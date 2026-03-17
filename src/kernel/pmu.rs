/// Hardware Performance Monitoring Unit (PMU) driver for Genesis
///
/// Provides direct access to x86-64 Architectural PMU counters via the
/// IA32_PERFEVTSELx / IA32_PMCx MSR pairs, as well as fixed-function
/// counters and global-control registers introduced in PMU v2+.
///
/// ## Detection
///
/// PMU capabilities are discovered at boot via CPUID leaf 0x0A
/// ("Architectural Performance Monitoring"):
///
///   EAX[7:0]   — version ID (0 = no architectural PMU)
///   EAX[15:8]  — number of general-purpose counters per logical processor
///   EAX[23:16] — bit width of each counter
///   EAX[31:24] — length of the EBX bit vector of architectural events
///   EBX[6:0]   — bit-mask of events NOT available (0 = available)
///
/// ## MSR layout (IA32 Architectural PMU)
///
///   IA32_PERFEVTSELx  = 0x186 + counter_index
///       [7:0]   — event select (event code)
///       [15:8]  — unit mask (UMASK)
///       [16]    — USR: count in user mode (CPL > 0)
///       [17]    — OS:  count in kernel mode (CPL = 0)
///       [18]    — E:   edge detect
///       [19]    — PC:  pin control
///       [20]    — INT: APIC interrupt on overflow
///       [21]    — any-thread (Nehalem+)
///       [22]    — EN:  counter enable
///       [23]    — INV: invert CMASK
///       [31:24] — CMASK: counter mask
///
///   IA32_PMCx  = 0xC1 + counter_index   (read/write; wraps on overflow)
///
/// ## Fixed-function counters (PMU v2+)
///
///   IA32_FIXED_CTR0 = 0x309  — instructions retired
///   IA32_FIXED_CTR1 = 0x30A  — cpu_clk_unhalted.thread
///   IA32_FIXED_CTR2 = 0x30B  — cpu_clk_unhalted.ref_tsc
///
/// ## Safety
///
/// MSR access requires CPL 0 (kernel mode). All `unsafe` MSR operations are
/// confined to the `rdmsr` / `wrmsr` helpers in this module.
/// No float casts, no heap, no panics.
use crate::serial_println;
use core::sync::atomic::{AtomicU8, Ordering};

// ---------------------------------------------------------------------------
// Atomic globals — populated by init() via CPUID
// ---------------------------------------------------------------------------

/// Architectural PMU version (0 = not supported, 1 = v1, 2 = v2 …)
pub static PMU_VERSION: AtomicU8 = AtomicU8::new(0);

/// Number of general-purpose counters per logical CPU (typically 4–8)
pub static PMU_COUNTERS: AtomicU8 = AtomicU8::new(0);

/// Bit width of each counter (typically 40 or 48 bits)
pub static PMU_COUNTER_WIDTH: AtomicU8 = AtomicU8::new(0);

// ---------------------------------------------------------------------------
// MSR constants — pub so perf_event.rs can reference them directly
// ---------------------------------------------------------------------------

pub const IA32_PERFEVTSEL0: u32 = 0x186;
pub const IA32_PERFEVTSEL1: u32 = 0x187;
pub const IA32_PERFEVTSEL2: u32 = 0x188;
pub const IA32_PERFEVTSEL3: u32 = 0x189;

pub const IA32_PMC0: u32 = 0xC1;
pub const IA32_PMC1: u32 = 0xC2;
pub const IA32_PMC2: u32 = 0xC3;
pub const IA32_PMC3: u32 = 0xC4;

pub const IA32_FIXED_CTR0: u32 = 0x309; // instructions retired
pub const IA32_FIXED_CTR1: u32 = 0x30A; // cpu_clk_unhalted.thread
pub const IA32_FIXED_CTR2: u32 = 0x30B; // cpu_clk_unhalted.ref_tsc

pub const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
pub const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
pub const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;
pub const IA32_PERF_GLOBAL_OVF_CTRL: u32 = 0x390;
pub const IA32_DEBUGCTL: u32 = 0x1D9;

// ---------------------------------------------------------------------------
// PERFEVTSELx bit-field positions (used in to_u64)
// ---------------------------------------------------------------------------

const EVTSEL_USR_BIT: u64 = 1 << 16;
const EVTSEL_OS_BIT: u64 = 1 << 17;
const EVTSEL_EDGE_BIT: u64 = 1 << 18;
const EVTSEL_PMI_BIT: u64 = 1 << 20;
const EVTSEL_EN_BIT: u64 = 1 << 22;
const EVTSEL_INV_BIT: u64 = 1 << 23;

// ---------------------------------------------------------------------------
// PmuEventSel — event selector configuration struct
// ---------------------------------------------------------------------------

/// Configuration for a single performance-monitoring event selector.
///
/// Maps directly to the IA32_PERFEVTSELx MSR layout.
#[derive(Copy, Clone)]
pub struct PmuEventSel {
    /// Event select code (bits [7:0] of the MSR).
    pub event_select: u8,
    /// Unit mask (bits [15:8]).
    pub umask: u8,
    /// Count in user mode (CPL > 0).
    pub usr: bool,
    /// Count in OS/kernel mode (CPL = 0).
    pub os: bool,
    /// Edge detect — count rising edges only.
    pub edge: bool,
    /// Performance monitoring interrupt on overflow.
    pub pmi: bool,
    /// Counter enable.
    pub en: bool,
    /// Invert CMASK comparison.
    pub inv: bool,
    /// Counter mask — minimum occurrence count per cycle (0 = count all).
    pub cmask: u8,
}

impl PmuEventSel {
    /// Pack all fields into the 64-bit IA32_PERFEVTSELx value.
    #[inline]
    pub fn to_u64(&self) -> u64 {
        let mut v: u64 = 0;
        v |= self.event_select as u64; // bits [7:0]
        v |= (self.umask as u64) << 8; // bits [15:8]
        if self.usr {
            v |= EVTSEL_USR_BIT;
        }
        if self.os {
            v |= EVTSEL_OS_BIT;
        }
        if self.edge {
            v |= EVTSEL_EDGE_BIT;
        }
        if self.pmi {
            v |= EVTSEL_PMI_BIT;
        }
        if self.en {
            v |= EVTSEL_EN_BIT;
        }
        if self.inv {
            v |= EVTSEL_INV_BIT;
        }
        v |= (self.cmask as u64) << 24; // bits [31:24]
        v
    }
}

// ---------------------------------------------------------------------------
// Common event presets — usr+os enabled, no PMI, no edge, no inv, cmask=0
// ---------------------------------------------------------------------------

/// Instructions retired (architectural event 0xC0/0x00).
pub const EVENT_INSTRUCTIONS_RETIRED: PmuEventSel = PmuEventSel {
    event_select: 0xC0,
    umask: 0x00,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// Unhalted core cycles (architectural event 0x3C/0x00).
pub const EVENT_CPU_CYCLES: PmuEventSel = PmuEventSel {
    event_select: 0x3C,
    umask: 0x00,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// Last-level cache misses (event 0x2E/0x41).
pub const EVENT_LLC_MISSES: PmuEventSel = PmuEventSel {
    event_select: 0x2E,
    umask: 0x41,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// Last-level cache references (event 0x2E/0x4F).
pub const EVENT_LLC_REFS: PmuEventSel = PmuEventSel {
    event_select: 0x2E,
    umask: 0x4F,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// Branch mispredictions (architectural event 0xC5/0x00).
pub const EVENT_BRANCH_MISSES: PmuEventSel = PmuEventSel {
    event_select: 0xC5,
    umask: 0x00,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// Branch instructions retired (architectural event 0xC4/0x00).
pub const EVENT_BRANCH_INSTRUCTIONS: PmuEventSel = PmuEventSel {
    event_select: 0xC4,
    umask: 0x00,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

/// DTLB load misses (event 0x08/0x81).
pub const EVENT_TLB_MISSES: PmuEventSel = PmuEventSel {
    event_select: 0x08,
    umask: 0x81,
    usr: true,
    os: true,
    edge: false,
    pmi: false,
    en: true,
    inv: false,
    cmask: 0,
};

// ---------------------------------------------------------------------------
// Snapshot — cheap copy of all counters at a point in time
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of the four general-purpose PMCs and three
/// fixed-function counters.
#[derive(Copy, Clone)]
pub struct PmuSnapshot {
    /// General-purpose counter 0 (programmed by caller — typically cycles).
    pub cycles: u64,
    /// General-purpose counter 1 (programmed by caller — typically instructions).
    pub instructions: u64,
    /// General-purpose counter 2 (programmed by caller — typically LLC misses).
    pub llc_miss: u64,
    /// General-purpose counter 3 (programmed by caller — typically branch misses).
    pub branch_miss: u64,
    /// Fixed counter 0: instructions retired.
    pub fixed0: u64,
    /// Fixed counter 1: cpu_clk_unhalted.thread.
    pub fixed1: u64,
    /// Fixed counter 2: cpu_clk_unhalted.ref_tsc.
    pub fixed2: u64,
}

// ---------------------------------------------------------------------------
// MSR helpers (private, CPL-0 only)
// ---------------------------------------------------------------------------

/// Read a 64-bit MSR.
///
/// # Safety
/// Executes `rdmsr` which requires CPL 0. Will `#GP` if the MSR does not
/// exist or the counter index is out of range — callers must validate first.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    (lo as u64) | ((hi as u64) << 32)
}

/// Write a 64-bit value to an MSR.
///
/// # Safety
/// Same restrictions as `rdmsr`.
#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

// ---------------------------------------------------------------------------
// Guard helper
// ---------------------------------------------------------------------------

#[inline]
fn counter_valid(counter: u32) -> bool {
    let n = PMU_COUNTERS.load(Ordering::Relaxed) as u32;
    n > 0 && counter < n && counter < 4
}

// ---------------------------------------------------------------------------
// Public API — general-purpose counters
// ---------------------------------------------------------------------------

/// Detect the PMU version and counter count via CPUID leaf 0x0A.
///
/// Returns the number of general-purpose counters (0 if no PMU present).
pub fn pmu_detect() -> u32 {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Au32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nomem, nostack)
        );
    }
    let version = (eax & 0xFF) as u8;
    let n_ctrs = ((eax >> 8) & 0xFF) as u8;
    let ctr_width = ((eax >> 16) & 0xFF) as u8;

    PMU_VERSION.store(version, Ordering::SeqCst);
    PMU_COUNTERS.store(n_ctrs, Ordering::SeqCst);
    PMU_COUNTER_WIDTH.store(ctr_width, Ordering::SeqCst);

    n_ctrs as u32
}

/// Program a general-purpose counter with `sel` and enable it.
///
/// No-op if `counter >= 4`, `counter >= PMU_COUNTERS`, or no PMU present.
pub fn pmu_enable_counter(counter: u32, sel: PmuEventSel) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !counter_valid(counter) {
        return;
    }
    let evtsel_msr = IA32_PERFEVTSEL0 + counter;
    // Force EN bit on regardless of the caller's setting
    let mut s = sel;
    s.en = true;
    unsafe {
        wrmsr(evtsel_msr, s.to_u64());
    }
}

/// Clear the EN bit in the PERFEVTSELx register for `counter`.
///
/// No-op if `counter` is out of range.
pub fn pmu_disable_counter(counter: u32) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !counter_valid(counter) {
        return;
    }
    let evtsel_msr = IA32_PERFEVTSEL0 + counter;
    unsafe {
        let val = rdmsr(evtsel_msr);
        wrmsr(evtsel_msr, val & !EVTSEL_EN_BIT);
    }
}

/// Read the current value of general-purpose counter `counter` via rdmsr.
///
/// Returns 0 if `counter` is out of range or no PMU present.
pub fn pmu_read_counter(counter: u32) -> u64 {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return 0;
    }
    if !counter_valid(counter) {
        return 0;
    }
    unsafe { rdmsr(IA32_PMC0 + counter) }
}

/// Write 0 to general-purpose counter `counter`.
///
/// No-op if `counter` is out of range.
pub fn pmu_reset_counter(counter: u32) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !counter_valid(counter) {
        return;
    }
    unsafe {
        wrmsr(IA32_PMC0 + counter, 0);
    }
}

// ---------------------------------------------------------------------------
// Fixed-function counters (PMU v2+)
// ---------------------------------------------------------------------------

/// Write `ctrl_bits` to IA32_FIXED_CTR_CTRL.
///
/// Each 4-bit field controls one fixed counter:
///   bits [3:0]  — fixed CTR0 (instructions retired)
///   bits [7:4]  — fixed CTR1 (core cycles unhalted)
///   bits [11:8] — fixed CTR2 (ref TSC cycles)
///
/// Within each 4-bit field:
///   bit 0 — count at CPL > 0 (user mode)
///   bit 1 — count at CPL = 0 (kernel mode)
///   bit 2 — AnyThread
///   bit 3 — PMI on overflow
pub fn pmu_enable_fixed(ctrl_bits: u64) {
    if PMU_VERSION.load(Ordering::Relaxed) < 2 {
        return;
    }
    unsafe {
        wrmsr(IA32_FIXED_CTR_CTRL, ctrl_bits);
    }
}

/// Read fixed-function counter `idx` (0 = instructions, 1 = core cycles, 2 = ref TSC).
///
/// Returns 0 if `idx > 2` or PMU version < 2.
pub fn pmu_read_fixed(idx: u32) -> u64 {
    if PMU_VERSION.load(Ordering::Relaxed) < 2 {
        return 0;
    }
    match idx {
        0 => unsafe { rdmsr(IA32_FIXED_CTR0) },
        1 => unsafe { rdmsr(IA32_FIXED_CTR1) },
        2 => unsafe { rdmsr(IA32_FIXED_CTR2) },
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Global-control / overflow
// ---------------------------------------------------------------------------

/// Write `mask` to IA32_PERF_GLOBAL_CTRL to enable specific counters.
///
/// Bits [3:0]  correspond to general-purpose counters 0–3.
/// Bits [34:32] correspond to fixed counters 0–2 (Intel notation).
pub fn pmu_global_enable(mask: u64) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    unsafe {
        wrmsr(IA32_PERF_GLOBAL_CTRL, mask);
    }
}

/// Write 0 to IA32_PERF_GLOBAL_CTRL — freezes all counters.
pub fn pmu_global_disable() {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    unsafe {
        wrmsr(IA32_PERF_GLOBAL_CTRL, 0);
    }
}

/// Clear all overflow bits by writing to IA32_PERF_GLOBAL_OVF_CTRL.
pub fn pmu_clear_overflow() {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    unsafe {
        wrmsr(IA32_PERF_GLOBAL_OVF_CTRL, 0xFFFF_FFFF_FFFF_FFFFu64);
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Read all four general-purpose PMCs and three fixed counters atomically
/// (best-effort; no actual hardware freeze).
pub fn pmu_snapshot() -> PmuSnapshot {
    PmuSnapshot {
        cycles: pmu_read_counter(0),
        instructions: pmu_read_counter(1),
        llc_miss: pmu_read_counter(2),
        branch_miss: pmu_read_counter(3),
        fixed0: pmu_read_fixed(0),
        fixed1: pmu_read_fixed(1),
        fixed2: pmu_read_fixed(2),
    }
}

// ---------------------------------------------------------------------------
// Legacy compatibility wrappers (used by kernel/mod.rs and other callers)
// ---------------------------------------------------------------------------

/// Program a general-purpose PMU counter and start counting (legacy API).
///
/// `event` encodes `(umask << 8) | event_select`.
/// `mask` is the CMASK field.
/// Counts in both user and OS mode, EN set.
pub fn pmu_counter_start(counter: u8, event: u32, mask: u8) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !counter_valid(counter as u32) {
        return;
    }
    let sel = PmuEventSel {
        event_select: (event & 0xFF) as u8,
        umask: ((event >> 8) & 0xFF) as u8,
        usr: true,
        os: true,
        edge: false,
        pmi: false,
        en: true,
        inv: false,
        cmask: mask,
    };
    pmu_reset_counter(counter as u32);
    pmu_enable_counter(counter as u32, sel);
    // Also gate through GLOBAL_CTRL
    unsafe {
        let gctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, gctrl | (1u64 << counter));
    }
}

/// Read the current value of a general-purpose PMU counter (legacy API).
pub fn pmu_counter_read(counter: u8) -> u64 {
    pmu_read_counter(counter as u32)
}

/// Stop a general-purpose PMU counter (legacy API).
pub fn pmu_counter_stop(counter: u8) {
    if PMU_VERSION.load(Ordering::Relaxed) == 0 {
        return;
    }
    if !counter_valid(counter as u32) {
        return;
    }
    pmu_disable_counter(counter as u32);
    unsafe {
        let gctrl = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, gctrl & !(1u64 << counter));
    }
}

/// Measure the number of occurrences of `event` while executing `f`,
/// using counter slot `counter`.  Returns the counter delta.
pub fn pmu_measure<F: FnOnce()>(counter: u8, event: u32, f: F) -> u64 {
    pmu_counter_start(counter, event, 0);
    f();
    let val = pmu_counter_read(counter);
    pmu_counter_stop(counter);
    val
}

// ---------------------------------------------------------------------------
// Legacy event constants (kept for callers that already use them)
// ---------------------------------------------------------------------------

/// Instructions retired (event 0xC0, UMASK 0x00)
pub const PMU_INSTRUCTIONS: u32 = 0x00_C0;
/// Unhalted core cycles (event 0x3C, UMASK 0x00)
pub const PMU_CYCLES: u32 = 0x00_3C;
/// Last-level cache (LLC) misses (event 0x2E, UMASK 0x41)
pub const PMU_CACHE_MISSES: u32 = 0x41_2E;
/// Branch instruction mispredictions (event 0xC5, UMASK 0x00)
pub const PMU_BRANCH_MISSES: u32 = 0x00_C5;

// ---------------------------------------------------------------------------
// Initialisation — called from kernel/mod.rs
// ---------------------------------------------------------------------------

/// Detect the PMU, program all four general-purpose counters with the
/// canonical event set, enable the fixed-function counters (both CPL0 and
/// CPL3, no PMI), and gate everything through GLOBAL_CTRL.
///
/// Called once at kernel boot.  Safe to call multiple times (no-op if PMU
/// was already detected).
pub fn init() {
    let n = pmu_detect();

    if n == 0 {
        serial_println!("  [pmu] No architectural PMU detected");
        return;
    }

    let version = PMU_VERSION.load(Ordering::Relaxed);
    let width = PMU_COUNTER_WIDTH.load(Ordering::Relaxed);
    serial_println!(
        "  [pmu] PMU v{} detected — {} x {}-bit general-purpose counters",
        version,
        n,
        width
    );

    // Program the four canonical GP counters (only as many as the hardware
    // exposes; counter_valid() guards the rest).
    pmu_reset_counter(0);
    pmu_reset_counter(1);
    pmu_reset_counter(2);
    pmu_reset_counter(3);
    pmu_enable_counter(0, EVENT_CPU_CYCLES);
    pmu_enable_counter(1, EVENT_INSTRUCTIONS_RETIRED);
    pmu_enable_counter(2, EVENT_LLC_MISSES);
    pmu_enable_counter(3, EVENT_BRANCH_MISSES);

    // Enable fixed-function counters if PMU v2+.
    // Each 4-bit field: bit0=count_user, bit1=count_os => 0b0011 = 0x3.
    // No PMI (bit3 clear), no AnyThread (bit2 clear).
    if version >= 2 {
        // Field layout: CTR0 in [3:0], CTR1 in [7:4], CTR2 in [11:8]
        let fixed_ctrl: u64 = 0x333; // enable user+OS for all three fixed ctrs
        pmu_enable_fixed(fixed_ctrl);
        serial_println!(
            "  [pmu] Fixed-function counters enabled (FIXED_CTR_CTRL=0x{:x})",
            fixed_ctrl
        );
    }

    // Enable all four GP counters and (if v2+) all three fixed counters in
    // GLOBAL_CTRL.  Bit layout per Intel SDM Vol 3B §18.2.2:
    //   bits  [3:0]  — GP counters 0-3
    //   bits [34:32] — fixed counters 0-2
    let gp_mask: u64 = if n >= 4 { 0xF } else { (1u64 << n) - 1 };
    let fixed_mask: u64 = if version >= 2 { 0x7u64 << 32 } else { 0 };
    pmu_global_enable(gp_mask | fixed_mask);

    serial_println!("  [pmu] GLOBAL_CTRL = 0x{:x}", gp_mask | fixed_mask);
}

/// Legacy entry point called by kernel/mod.rs (`pmu::pmu_init()`).
pub fn pmu_init() {
    init();
}
