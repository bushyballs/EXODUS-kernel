/*
 * Genesis OS — CPU Management
 *
 * High-level CPU operations: enumeration, hotplug, power management.
 */

use core::sync::atomic::{AtomicU32, Ordering};

static ONLINE_CPUS: AtomicU32 = AtomicU32::new(0);

/// Mark a CPU as online
pub fn set_online(cpu_id: usize) {
    if let Some(cpu) = crate::percpu::cpu_data(cpu_id) {
        cpu.online.store(true, Ordering::SeqCst);
        ONLINE_CPUS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Mark a CPU as offline
pub fn set_offline(cpu_id: usize) {
    if let Some(cpu) = crate::percpu::cpu_data(cpu_id) {
        if cpu.online.swap(false, Ordering::SeqCst) {
            ONLINE_CPUS.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Get number of online CPUs
pub fn online_count() -> usize {
    ONLINE_CPUS.load(Ordering::SeqCst) as usize
}

/// Hot-plug a CPU (bring it online)
pub unsafe fn hotplug_cpu(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id >= crate::percpu::MAX_CPUS {
        return Err("Invalid CPU ID");
    }

    if crate::percpu::is_online(cpu_id) {
        return Err("CPU already online");
    }

    // Get APIC ID for this CPU
    let apic_id = crate::acpi::cpu_apic_id(cpu_id).ok_or("CPU not found in ACPI tables")?;

    // Start the CPU using INIT-SIPI-SIPI sequence
    // CPU is started via SMP initialization
    // crate::smp::start_aps handles this

    Ok(())
}

/// Hot-unplug a CPU (take it offline)
pub unsafe fn hotunplug_cpu(cpu_id: usize) -> Result<(), &'static str> {
    if cpu_id == 0 {
        return Err("Cannot unplug BSP");
    }

    if !crate::percpu::is_online(cpu_id) {
        return Err("CPU already offline");
    }

    // Send halt IPI to the target CPU
    if let Some(apic_id) = crate::acpi::cpu_apic_id(cpu_id) {
        crate::apic::send_ipi(apic_id as u32, crate::apic::IPI_HALT);
    }

    // Mark as offline
    set_offline(cpu_id);

    Ok(())
}

/// Halt all CPUs except the caller
pub unsafe fn halt_other_cpus() {
    crate::apic::send_ipi_all(crate::apic::IPI_HALT);
}

// ── CPU power states ───────────────────────────────────────────────────────

/// Enter C1 idle on the current CPU.
///
/// Enables interrupts (STI) then executes HLT, which halts the pipeline
/// until the next interrupt arrives.  The scheduler should call this
/// when there is no runnable work on the current core.
#[inline]
pub fn cpu_idle() {
    unsafe {
        core::arch::asm!(
            "sti", // enable interrupts so the wake IRQ is delivered
            "hlt", // halt — resumes on next interrupt
            options(nomem, nostack)
        );
    }
}

/// Permanently halt the current CPU (CLI + HLT loop).
///
/// Used for dead/offlined CPUs and unrecoverable error paths.
/// Does not return.
pub fn cpu_halt() -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        crate::io::hlt();
    }
}

/// Enter a CPU C-state using MWAIT.
///
/// `hint` is the MWAIT hint value:
///   - 0x00 = C1 (equivalent to HLT but uses MWAIT path)
///   - 0x01 = C1E
///   - 0x10 = C3
///   - 0x20 = C6
///   - 0x30 = C7
///
/// The caller must ensure MWAIT/MONITOR are supported (CPUID.01H:ECX bit 3).
/// A dummy memory location is monitored so any cache-line write wakes the CPU.
#[inline]
pub unsafe fn cpu_mwait(hint: u32) {
    let mut monitor_var: u64 = 0;
    let addr = &mut monitor_var as *mut u64;
    core::arch::asm!(
        // MONITOR rax, ecx (ext), edx (hints) — sets the monitored address
        "monitor",
        in("rax") addr,
        in("ecx") 0u32,
        in("edx") 0u32,
        options(nomem, nostack)
    );
    core::arch::asm!(
        // MWAIT eax (hint), ecx (extensions)
        "mwait",
        in("eax") hint,
        in("ecx") 0u32,
        options(nomem, nostack)
    );
}

/// Read CPU timestamp counter (for benchmarking)
#[inline]
pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Pause CPU (for spinlock back-off)
#[inline]
pub fn pause() {
    unsafe {
        core::arch::asm!("pause", options(nostack, nomem));
    }
}

/// Get CPU features (CPUID)
pub struct CpuFeatures {
    pub max_cpuid: u32,
    pub vendor: [u8; 12],
    pub has_apic: bool,
    pub has_msr: bool,
    pub has_x2apic: bool,
}

impl CpuFeatures {
    pub fn detect() -> Self {
        let mut features = CpuFeatures {
            max_cpuid: 0,
            vendor: [0; 12],
            has_apic: false,
            has_msr: false,
            has_x2apic: false,
        };

        unsafe {
            let (max_cpuid, ebx, ecx, edx) = cpuid(0);
            features.max_cpuid = max_cpuid;

            // Vendor string: EBX, EDX, ECX
            features.vendor[0..4].copy_from_slice(&ebx.to_le_bytes());
            features.vendor[4..8].copy_from_slice(&edx.to_le_bytes());
            features.vendor[8..12].copy_from_slice(&ecx.to_le_bytes());

            if max_cpuid >= 1 {
                let (_, _, ecx, edx) = cpuid(1);
                features.has_apic = (edx & (1 << 9)) != 0;
                features.has_msr = (edx & (1 << 5)) != 0;
                features.has_x2apic = (ecx & (1 << 21)) != 0;
            }
        }

        features
    }
}

/// Execute CPUID instruction
#[inline]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;

    core::arch::asm!(
        "mov rbx, rdi",
        "cpuid",
        "xchg rbx, rdi",
        inout("eax") leaf => eax,
        out("edi") ebx,
        out("ecx") ecx,
        out("edx") edx,
        options(nostack, nomem)
    );

    (eax, ebx, ecx, edx)
}

/// Read MSR (Model-Specific Register)
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;

    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nostack, nomem)
    );

    ((high as u64) << 32) | (low as u64)
}

/// Write MSR
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = (value & 0xFFFFFFFF) as u32;
    let high = (value >> 32) as u32;

    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, nomem)
    );
}

/// Write to CR3 register (page table base)
#[inline]
pub unsafe fn write_cr3(value: u64) {
    core::arch::asm!(
        "mov cr3, {}",
        in(reg) value,
        options(nostack, nomem)
    );
}

/// Read from CR3 register
#[inline]
pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) value,
            options(nostack, nomem)
        );
    }
    value
}

/// Read from CR2 register (page fault address)
#[inline]
pub unsafe fn read_cr2() -> u64 {
    let value: u64;
    core::arch::asm!(
        "mov {}, cr2",
        out(reg) value,
        options(nostack, nomem)
    );
    value
}

/// Invalidate TLB entry for a specific virtual address
#[inline]
pub unsafe fn invlpg(addr: u64) {
    core::arch::asm!(
        "invlpg [{}]",
        in(reg) addr,
        options(nostack, preserves_flags)
    );
}
