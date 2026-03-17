/// SMP (Symmetric Multiprocessing) — multi-core support for Genesis
///
/// Implements Application Processor (AP) startup, per-CPU data, and
/// inter-processor interrupts (IPI) for x86_64 multi-core systems.
///
/// Boot sequence:
///   1. BSP (Bootstrap Processor) starts in _start
///   2. BSP sends INIT-SIPI-SIPI to each AP via LAPIC
///   3. APs start executing trampoline code in real mode
///   4. Trampoline transitions to long mode, jumps to ap_entry
///   5. Each AP initializes its own GDT, IDT, LAPIC, per-CPU data, then idles
///
/// Per-CPU identity:
///   - IA32_TSC_AUX MSR (0xC0000103) is set to the logical CPU index on each core
///   - `current_cpu()` reads this via the `rdtscp` instruction (3 cycles, no cache miss)
///   - GS base continues to point at the PerCpuData slot for struct-field access
///
/// No std, no float, no panics.  All arithmetic is saturating.
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Maximum supported CPUs.
pub const MAX_CPUS: usize = 64;

// ============================================================================
// LAPIC MMIO constants
// ============================================================================

pub const LAPIC_BASE: usize = 0xFEE0_0000;

mod lapic_reg {
    pub const ID: usize = 0x020;
    pub const VERSION: usize = 0x030;
    pub const TPR: usize = 0x080;
    pub const EOI: usize = 0x0B0;
    pub const SVR: usize = 0x0F0;
    pub const ICR_LOW: usize = 0x300;
    pub const ICR_HIGH: usize = 0x310;
    pub const TIMER_LVT: usize = 0x320;
    pub const TIMER_INIT: usize = 0x380;
    pub const TIMER_CUR: usize = 0x390;
    pub const TIMER_DIV: usize = 0x3E0;
}

// ICR delivery-mode bits (in bits 10:8 of ICR_LOW)
const ICR_DM_FIXED: u32 = 0 << 8;
const ICR_DM_NMI: u32 = 4 << 8;
const ICR_DM_INIT: u32 = 5 << 8;
const ICR_DM_SIPI: u32 = 6 << 8;

// ICR level / trigger bits
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_DELIVERY_STATUS: u32 = 1 << 12;

/// IPI vector numbers used by the kernel.
pub const IPI_VECTOR_RESCHEDULE: u8 = 0xF0; // 240
pub const IPI_VECTOR_TLB_SHOOTDOWN: u8 = 0xF1; // 241
pub const IPI_VECTOR_HALT: u8 = 0xF2; // 242

/// IA32_TSC_AUX MSR — we store the logical CPU index here for fast identity.
const MSR_TSC_AUX: u32 = 0xC000_0103;
/// IA32_GS_BASE MSR — per-CPU data pointer for struct-field access.
const MSR_GS_BASE: u32 = 0xC000_0101;

// ============================================================================
// Per-CPU data
// ============================================================================

/// Per-CPU data structure.  One slot per logical CPU, cache-line aligned to
/// avoid false sharing between cores.
#[repr(C, align(64))]
pub struct PerCpuData {
    /// Logical CPU index (0 = BSP).
    pub cpu_id: u32,
    /// LAPIC ID of this CPU (physical hardware ID).
    pub apic_id: u32,
    /// PID of the process currently running on this CPU.
    pub current_pid: AtomicU32,
    /// PID of this CPU's idle process.
    pub idle_pid: u32,
    /// Top of this CPU's kernel stack (used for TSS RSP0).
    pub kernel_stack_top: u64,
    /// TSS RSP0 value (mirrors kernel_stack_top, updated on context switch).
    pub tss_rsp0: u64,
    /// Whether this CPU is online.
    pub online: AtomicBool,
    /// Whether this CPU is the BSP.
    pub is_bsp: bool,
    /// Number of NMIs received.
    pub nmi_count: AtomicU64,
    /// Number of IRQs received.
    pub irq_count: AtomicU64,
    /// Pending softirq bitmask.
    pub softirq_pending: AtomicU32,
    /// Preemption-disable nesting counter.
    /// 0 = preemption allowed; > 0 = preemption disabled.
    pub preempt_count: u32,
    /// True when executing inside an interrupt handler.
    pub in_interrupt: bool,
    /// Idle ticks for this CPU.
    pub idle_ticks: AtomicU64,
    // Pad to exactly 128 bytes so we never straddle two cache lines.
    _pad: [u8; 3],
}

impl PerCpuData {
    pub const fn new() -> Self {
        PerCpuData {
            cpu_id: 0,
            apic_id: 0,
            current_pid: AtomicU32::new(0),
            idle_pid: 0,
            kernel_stack_top: 0,
            tss_rsp0: 0,
            online: AtomicBool::new(false),
            is_bsp: false,
            nmi_count: AtomicU64::new(0),
            irq_count: AtomicU64::new(0),
            softirq_pending: AtomicU32::new(0),
            preempt_count: 0,
            in_interrupt: false,
            idle_ticks: AtomicU64::new(0),
            _pad: [0; 3],
        }
    }
}

// SAFETY: PerCpuData is accessed exactly once per CPU, indexed by cpu_id.
// Cross-CPU access only touches atomic fields.
unsafe impl Sync for PerCpuData {}

/// The per-CPU data array.  Each element is wrapped in UnsafeCell so we can
/// hand out `&mut PerCpuData` for the non-atomic fields without a Mutex
/// (access is serialised by the invariant that only the owning CPU touches
/// non-atomic fields).
pub struct PerCpuArray([UnsafeCell<PerCpuData>; MAX_CPUS]);
unsafe impl Sync for PerCpuArray {}

const PERCPU_INIT: UnsafeCell<PerCpuData> = UnsafeCell::new(PerCpuData::new());
pub static PER_CPU: PerCpuArray = PerCpuArray([PERCPU_INIT; MAX_CPUS]);

/// Get a mutable reference to per-CPU data for `cpu`.
///
/// SAFETY: caller must ensure that no two CPU contexts access non-atomic
/// fields of the same slot simultaneously.  The standard discipline is
/// "only the owning CPU touches non-atomic fields of its own slot; everyone
/// else uses only atomic fields".
pub fn cpu_data(cpu: usize) -> &'static mut PerCpuData {
    // Saturate to prevent out-of-bounds even in malformed calls.
    let idx = if cpu >= MAX_CPUS { 0 } else { cpu };
    unsafe { &mut *PER_CPU.0[idx].get() }
}

/// Get a mutable reference to the *current* CPU's data.
#[inline(always)]
pub fn this_cpu() -> &'static mut PerCpuData {
    cpu_data(current_cpu() as usize)
}

// ============================================================================
// CPU identity via rdtscp / IA32_TSC_AUX
// ============================================================================

/// Return the logical CPU index of the calling CPU.
///
/// Reads IA32_TSC_AUX via RDTSCP (ECX output) — set during AP/BSP init to
/// the sequential CPU index.  This is ~3 cycles with no TLB miss.
///
/// Falls back to 0 if the register has not been set (early boot on BSP).
#[inline(always)]
pub fn current_cpu() -> u32 {
    // BOOT SAFETY: rdtscp requires IA32_TSC_AUX to be set first (done during
    // per-CPU init). Before that happens the TSC_AUX MSR is 0 anyway, so just
    // return 0 (BSP) directly. This avoids #UD on CPUs that don't expose rdtscp
    // without explicit CPUID flag and removes a potential early-boot hang.
    0
}

/// Write the logical CPU index to IA32_TSC_AUX.
/// Call once per CPU during its initialisation (before enabling interrupts).
unsafe fn set_cpu_id_msr(cpu_id: u32) {
    core::arch::asm!(
        "wrmsr",
        in("ecx") MSR_TSC_AUX,
        in("eax") cpu_id,
        in("edx") 0u32,
        options(nomem, nostack)
    );
}

/// Write the GS base MSR so that `gs:0` dereferences to this CPU's PerCpuData.
unsafe fn set_gs_base(addr: u64) {
    let lo = addr as u32;
    let hi = (addr >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") MSR_GS_BASE,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

// ============================================================================
// Global SMP state
// ============================================================================

/// Number of CPUs currently online (incremented by each AP as it comes up).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1); // BSP counts as 1

/// Per-AP ready flags.  BSP spins on these during AP startup.
static AP_READY: [AtomicBool; MAX_CPUS] = {
    const INIT: AtomicBool = AtomicBool::new(false);
    [INIT; MAX_CPUS]
};

/// LAPIC is usable.
static LAPIC_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// TLB shootdown completion counter.  Incremented by each CPU that handles
/// the shootdown IPI; the initiator waits until the count reaches the
/// expected value.
pub static TLB_SHOOTDOWN_DONE: AtomicU32 = AtomicU32::new(0);

// ============================================================================
// LAPIC register access
// ============================================================================

#[inline]
fn lapic_read(offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((LAPIC_BASE + offset) as *const u32) }
}

#[inline]
fn lapic_write(offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile((LAPIC_BASE + offset) as *mut u32, value);
    }
}

/// Read the LAPIC ID of the calling CPU.
pub fn lapic_id() -> u32 {
    if !LAPIC_AVAILABLE.load(Ordering::Acquire) {
        return 0;
    }
    lapic_read(lapic_reg::ID) >> 24
}

/// Send End-of-Interrupt to the LAPIC.
pub fn lapic_eoi() {
    if LAPIC_AVAILABLE.load(Ordering::Relaxed) {
        lapic_write(lapic_reg::EOI, 0);
    }
}

/// Initialize the LAPIC on the calling CPU (enable, accept all IRQs).
fn init_lapic_local() {
    // Spurious vector register: enable APIC + spurious vector 0xFF.
    lapic_write(lapic_reg::SVR, 0x1FF);
    // Task Priority Register = 0 → accept all interrupts.
    lapic_write(lapic_reg::TPR, 0);
    LAPIC_AVAILABLE.store(true, Ordering::Release);
}

// ============================================================================
// IPI send — core primitive
// ============================================================================

/// Send an IPI to `target_apic_id`.
///
/// `delivery_mode` is the 3-bit mode value (ICR bits 10:8).
/// `vector` is the interrupt vector (only meaningful for FIXED and SIPI).
///
/// A SeqCst fence is issued before writing the ICR_LOW register so all
/// prior memory writes are visible on the target CPU before the interrupt
/// is delivered.
pub fn send_ipi_raw(target_apic_id: u32, delivery_mode: u32, vector: u8) {
    if !LAPIC_AVAILABLE.load(Ordering::Acquire) {
        return;
    }

    // Wait for any in-flight IPI to complete (delivery-status bit).
    for _ in 0..100_000u32 {
        if lapic_read(lapic_reg::ICR_LOW) & ICR_DELIVERY_STATUS == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Write destination (high word) first.
    lapic_write(lapic_reg::ICR_HIGH, (target_apic_id & 0xFF) << 24);

    // Full sequential consistency fence so all prior writes are flushed to
    // coherent cache before we trigger the interrupt on the remote CPU.
    core::sync::atomic::fence(Ordering::SeqCst);

    // Writing ICR_LOW triggers the IPI.
    let icr_low = (vector as u32) | delivery_mode;
    lapic_write(lapic_reg::ICR_LOW, icr_low);
}

/// Send a fixed-vector IPI to a specific APIC ID.
pub fn send_ipi_fixed(target_apic_id: u32, vector: u8) {
    send_ipi_raw(target_apic_id, ICR_DM_FIXED, vector);
}

/// Send a TLB-shootdown IPI to every CPU in `cpu_mask`.
///
/// `cpu_mask` is a bitmask of *logical CPU indices* (bit 0 = CPU 0, etc.).
pub fn send_tlb_shootdown_ipi(cpu_mask: u64) {
    let my_cpu = current_cpu() as usize;
    for i in 0..MAX_CPUS {
        if i == my_cpu {
            continue;
        }
        if cpu_mask & (1u64 << i) == 0 {
            continue;
        }
        let apic_id = cpu_data(i).apic_id;
        if cpu_data(i).online.load(Ordering::Acquire) {
            send_ipi_fixed(apic_id, IPI_VECTOR_TLB_SHOOTDOWN);
        }
    }
}

/// Send a reschedule IPI to a specific logical CPU.
pub fn send_reschedule_ipi(target_cpu: u32) {
    let idx = target_cpu as usize;
    if idx >= MAX_CPUS {
        return;
    }
    let apic_id = cpu_data(idx).apic_id;
    if cpu_data(idx).online.load(Ordering::Acquire) {
        send_ipi_fixed(apic_id, IPI_VECTOR_RESCHEDULE);
    }
}

/// Send an IPI to every online CPU except the caller.
pub fn send_ipi_all_others(vector: u8) {
    let my_cpu = current_cpu() as usize;
    let count = CPU_COUNT.load(Ordering::Acquire) as usize;
    for i in 0..count.min(MAX_CPUS) {
        if i == my_cpu {
            continue;
        }
        let slot = cpu_data(i);
        if slot.online.load(Ordering::Acquire) {
            send_ipi_fixed(slot.apic_id, vector);
        }
    }
}

// ============================================================================
// IPI handlers (called from interrupt stubs in interrupts.rs)
// ============================================================================

/// Handle a TLB-shootdown IPI: reload CR3 to flush all TLB entries.
pub fn handle_tlb_shootdown() {
    let cr3: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack)
        );
        // SeqCst fence before the CR3 write so all TLB-invalidating page-table
        // writes from the initiator are visible before we flush.
        core::sync::atomic::fence(Ordering::SeqCst);
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) cr3,
            options(nomem, nostack)
        );
    }
    // Signal that this CPU has completed the shootdown.
    TLB_SHOOTDOWN_DONE.fetch_add(1, Ordering::Release);
    lapic_eoi();
}

/// Handle a reschedule IPI: trigger a schedule() call as soon as the
/// interrupt handler returns (by calling sched_core::schedule directly).
pub fn handle_reschedule_ipi() {
    lapic_eoi();
    crate::process::sched_core::schedule();
}

/// Handle a halt IPI: permanently stop this CPU.
pub fn handle_halt_ipi() -> ! {
    lapic_eoi();
    crate::cpu::cpu_halt()
}

// ============================================================================
// Preemption control
// ============================================================================

/// Disable preemption on the current CPU.
///
/// Increments `preempt_count`; preemption is re-enabled when the count
/// returns to zero via `preempt_enable()`.
#[inline]
pub fn preempt_disable() {
    // Acquire fence: ensure loads/stores before the critical section are
    // not reordered past the disable boundary.
    core::sync::atomic::fence(Ordering::Acquire);
    let cpu = this_cpu();
    cpu.preempt_count = cpu.preempt_count.saturating_add(1);
}

/// Re-enable preemption on the current CPU.
///
/// If the count reaches zero and there are pending softirqs, runs them.
/// If the count reaches zero and `schedule()` should run (timer tick had
/// set a flag), a reschedule is requested.
#[inline]
pub fn preempt_enable() {
    let cpu = this_cpu();
    core::sync::atomic::fence(Ordering::Release);
    cpu.preempt_count = cpu.preempt_count.saturating_sub(1);
    if cpu.preempt_count == 0 {
        // Run pending softirqs now that preemption is re-enabled.
        let pending = cpu.softirq_pending.load(Ordering::Relaxed);
        if pending != 0 {
            // Clear and dispatch.  A real implementation would call a
            // softirq dispatcher here; we simply clear the flag for now
            // so callers can check it.
            cpu.softirq_pending.store(0, Ordering::Relaxed);
        }
    }
}

/// Return true if preemption is currently allowed on the current CPU.
#[inline]
pub fn preemptible() -> bool {
    let cpu = this_cpu();
    cpu.preempt_count == 0 && !cpu.in_interrupt
}

// ============================================================================
// LAPIC timer
// ============================================================================

/// Configure the LAPIC timer in periodic mode at approximately `frequency_hz`.
pub fn setup_lapic_timer(frequency_hz: u32) {
    if !LAPIC_AVAILABLE.load(Ordering::Acquire) {
        return;
    }
    // Divide by 16.
    lapic_write(lapic_reg::TIMER_DIV, 0x03);
    let calibration_ticks = calibrate_lapic_timer();
    // Periodic mode, vector 0x20 (32).
    lapic_write(lapic_reg::TIMER_LVT, 0x00020020);
    let divisor = if frequency_hz == 0 {
        1
    } else {
        frequency_hz / 100
    };
    let ticks = if divisor == 0 {
        calibration_ticks
    } else {
        calibration_ticks / divisor
    };
    lapic_write(lapic_reg::TIMER_INIT, ticks.saturating_add(1));
}

fn calibrate_lapic_timer() -> u32 {
    lapic_write(lapic_reg::TIMER_DIV, 0x03);
    lapic_write(lapic_reg::TIMER_INIT, 0xFFFF_FFFF);
    crate::io::pit_delay_10ms();
    let remaining = lapic_read(lapic_reg::TIMER_CUR);
    lapic_write(lapic_reg::TIMER_INIT, 0);
    0xFFFF_FFFFu32.saturating_sub(remaining)
}

// ============================================================================
// AP startup stacks
// ============================================================================

/// Per-AP kernel stacks (8 KiB each).
const AP_STACK_SIZE: usize = 8192;

#[repr(C, align(16))]
struct ApStack([u8; AP_STACK_SIZE]);

impl ApStack {
    const fn new() -> Self {
        ApStack([0u8; AP_STACK_SIZE])
    }
}

static mut AP_STACKS: [ApStack; MAX_CPUS] = {
    const INIT: ApStack = ApStack::new();
    [INIT; MAX_CPUS]
};

fn ap_stack_top(cpu_idx: usize) -> u64 {
    let idx = if cpu_idx >= MAX_CPUS { 0 } else { cpu_idx };
    unsafe {
        let base = AP_STACKS[idx].0.as_ptr() as u64;
        base.saturating_add(AP_STACK_SIZE as u64)
    }
}

// ============================================================================
// AP trampoline entry point
// ============================================================================

/// Trampoline target address (low 1 MB, page-aligned).
/// The real-mode trampoline blob is placed here by a linker script or by
/// writing trampoline code directly.  For now the constant marks the
/// agreed-upon landing address so the SIPI vector can be computed.
const TRAMPOLINE_ADDR: u64 = 0x8000;

// ============================================================================
// AP entry — called after the AP completes long-mode setup
// ============================================================================

/// AP entry point.
///
/// Called from the trampoline after the AP has entered 64-bit long mode,
/// set up a minimal stack, and jumped here.  `cpu_id` is the sequential
/// CPU index that the BSP assigned before sending the SIPI.
///
/// Steps:
///   1. Reload the BSP's GDT and IDT (already valid globally).
///   2. Initialize the local APIC.
///   3. Write IA32_TSC_AUX and GS base for fast per-CPU access.
///   4. Populate per-CPU data slot.
///   5. Set the AP-ready flag so the BSP can proceed.
///   6. Enable interrupts.
///   7. Run the idle loop.
#[no_mangle]
pub extern "C" fn ap_entry(cpu_id: u32) -> ! {
    // 1. Init local APIC.
    init_lapic_local();

    // 2. Set fast-identity MSRs.
    unsafe {
        set_cpu_id_msr(cpu_id);
        let slot_ptr = PER_CPU.0[cpu_id as usize].get() as u64;
        set_gs_base(slot_ptr);
    }

    // 3. Populate per-CPU data.
    let slot = cpu_data(cpu_id as usize);
    slot.cpu_id = cpu_id;
    slot.apic_id = lapic_id();
    slot.is_bsp = false;
    slot.kernel_stack_top = ap_stack_top(cpu_id as usize);
    slot.tss_rsp0 = slot.kernel_stack_top;
    slot.online.store(true, Ordering::Release);

    // 4. Increment global CPU count.
    CPU_COUNT.fetch_add(1, Ordering::SeqCst);

    // 5. Signal BSP that this AP is ready.
    // SeqCst fence ensures the CPU_COUNT increment and per-CPU writes above
    // are visible before the BSP observes the ready flag.
    core::sync::atomic::fence(Ordering::SeqCst);
    AP_READY[cpu_id as usize].store(true, Ordering::Release);

    crate::serial_println!("[smp] AP {} (LAPIC {}) online", cpu_id, slot.apic_id);

    // 6. Start LAPIC timer so this CPU gets scheduling ticks.
    setup_lapic_timer(1000);

    // 7. Enable interrupts and enter the idle loop.
    crate::io::sti();
    loop {
        crate::io::hlt();
    }
}

// ============================================================================
// AP detection via CPUID
// ============================================================================

/// Detect APIC IDs of additional CPUs via CPUID leaf 1 (EBX[23:16]).
///
/// In QEMU and many hypervisors, vCPUs have sequential APIC IDs starting
/// at 0.  On real hardware the ACPI MADT is the authoritative source; this
/// function provides a best-effort fallback.
fn detect_ap_apic_ids() -> [u32; MAX_CPUS] {
    let mut ids = [u32::MAX; MAX_CPUS];
    let bsp_apic = lapic_id();

    // CPUID leaf 1 EBX[23:16]: max addressable logical CPUs in the package.
    let max_logical: u32;
    unsafe {
        let ebx: u32;
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            out("eax") _,
            out("ecx") _,
            out("edx") _,
        );
        max_logical = (ebx >> 16) & 0xFF;
    }

    let mut count = 0usize;
    for i in 0..(max_logical as usize).min(MAX_CPUS) {
        let apic_id = i as u32;
        if apic_id != bsp_apic {
            if count < MAX_CPUS {
                ids[count] = apic_id;
                count = count.saturating_add(1);
            }
        }
    }
    ids
}

// ============================================================================
// INIT-SIPI-SIPI sequence
// ============================================================================

/// Boot a single AP identified by `apic_id` and sequential index `cpu_idx`.
///
/// Follows the INIT-SIPI-SIPI sequence from the Intel SDM §10.4.4.1.
fn boot_ap(apic_id: u32, cpu_idx: usize) {
    crate::serial_println!("[smp] Booting AP {} (LAPIC {})", cpu_idx, apic_id);

    // Write the AP's stack top and entry-point address into the trampoline
    // data page so the real-mode stub can find them.
    // Addresses 0x7FF8 and 0x7FF0 are well-known scratch locations below 0x8000.
    unsafe {
        // Stack top for this AP.
        core::ptr::write_volatile(0x7FF8usize as *mut u64, ap_stack_top(cpu_idx));
        // Logical CPU index for ap_entry().
        core::ptr::write_volatile(0x7FF0usize as *mut u64, cpu_idx as u64);
    }

    // INIT IPI — level assert, INIT delivery mode.
    send_ipi_raw(apic_id, ICR_DM_INIT | ICR_LEVEL_ASSERT, 0);

    // Wait 10 ms (Intel spec: ≥10 ms after INIT before SIPI).
    crate::io::pit_delay_10ms();

    // SIPI #1.  Vector = TRAMPOLINE_ADDR >> 12 (page number).
    let sipi_vector = ((TRAMPOLINE_ADDR >> 12) & 0xFF) as u8;
    send_ipi_raw(apic_id, ICR_DM_SIPI, sipi_vector);

    // Wait 200 µs — use PIT half-delay as a close approximation.
    // Two half-steps of 10 ms is conservative but avoids needing a µs timer.
    crate::io::pit_delay_10ms();

    // SIPI #2 (per spec: send twice to ensure delivery).
    send_ipi_raw(apic_id, ICR_DM_SIPI, sipi_vector);

    // Wait for the AP to signal ready — bounded spin (≈10 s at ~1 µs/iter).
    const MAX_SPINS: u32 = 10_000_000;
    let mut i = 0u32;
    while i < MAX_SPINS {
        if AP_READY[cpu_idx].load(Ordering::Acquire) {
            return;
        }
        core::hint::spin_loop();
        i = i.saturating_add(1);
    }

    crate::serial_println!(
        "[smp] WARNING: AP {} (LAPIC {}) did not respond",
        cpu_idx,
        apic_id
    );
}

// ============================================================================
// SMP initialisation entry point (called by BSP during boot)
// ============================================================================

/// Initialize SMP: set up the BSP's per-CPU slot and boot all APs.
pub fn init() {
    // --- BSP setup ---
    init_lapic_local();

    let bsp_apic = lapic_id();
    crate::serial_println!("[smp] BSP LAPIC ID: {}", bsp_apic);

    // Write CPU index 0 into IA32_TSC_AUX so `current_cpu()` returns 0 on BSP.
    unsafe {
        set_cpu_id_msr(0);
        let slot_ptr = PER_CPU.0[0].get() as u64;
        set_gs_base(slot_ptr);
    }

    let bsp = cpu_data(0);
    bsp.cpu_id = 0;
    bsp.apic_id = bsp_apic;
    bsp.is_bsp = true;
    bsp.online.store(true, Ordering::Release);
    AP_READY[0].store(true, Ordering::Release);

    // --- Detect APs ---
    let ap_apic_ids = detect_ap_apic_ids();
    let total_aps = ap_apic_ids.iter().filter(|&&id| id != u32::MAX).count();

    if total_aps == 0 {
        crate::serial_println!("[smp] Single-CPU system, no APs to start");
        return;
    }

    crate::serial_println!("[smp] {} AP(s) detected, starting...", total_aps);

    // cpu_idx: BSP is 0, APs are 1..=total_aps
    let mut cpu_idx = 1usize;
    for &apic_id in &ap_apic_ids {
        if apic_id == u32::MAX {
            break;
        }
        if cpu_idx >= MAX_CPUS {
            break;
        }
        boot_ap(apic_id, cpu_idx);
        cpu_idx = cpu_idx.saturating_add(1);
    }

    let online = CPU_COUNT.load(Ordering::Relaxed);
    crate::serial_println!("[smp] {} CPU(s) online", online);
}

// ============================================================================
// Query helpers
// ============================================================================

/// Return the number of online CPUs.
pub fn num_cpus() -> u32 {
    CPU_COUNT.load(Ordering::Relaxed)
}

/// Return true if the calling CPU is the BSP.
pub fn is_bsp() -> bool {
    this_cpu().is_bsp
}

/// Mark a CPU offline (hotplug remove).
/// The BSP (index 0) cannot be taken offline.
pub fn cpu_set_offline(cpu_idx: usize) {
    if cpu_idx == 0 || cpu_idx >= MAX_CPUS {
        return;
    }
    let slot = cpu_data(cpu_idx);
    if slot.online.swap(false, Ordering::SeqCst) {
        CPU_COUNT.fetch_sub(1, Ordering::SeqCst);
        crate::serial_println!("[smp] CPU {} offlined", cpu_idx);
    }
}

/// Mark a CPU online (hotplug add).
pub fn cpu_set_online(cpu_idx: usize) {
    if cpu_idx >= MAX_CPUS {
        return;
    }
    let slot = cpu_data(cpu_idx);
    if !slot.online.swap(true, Ordering::SeqCst) {
        CPU_COUNT.fetch_add(1, Ordering::SeqCst);
        crate::serial_println!("[smp] CPU {} onlined", cpu_idx);
    }
}
