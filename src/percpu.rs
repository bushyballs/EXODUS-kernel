/*
 * Genesis OS — Per-CPU Variables
 *
 * Each CPU has its own data structure stored in a static array.
 * CPU ID is determined via APIC ID, stored in GS base for fast access.
 */

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const MAX_CPUS: usize = 256;

#[repr(C, align(64))] // Cache line aligned to avoid false sharing
pub struct PerCpu {
    pub cpu_id: u32,
    pub apic_id: u32,
    pub online: AtomicBool,
    pub ticks: AtomicU64,
    pub idle_task: u64,     // Pointer to idle task for this CPU
    pub current_task: u64,  // Pointer to currently running task
    pub kernel_stack: u64,  // Top of kernel stack
    pub user_stack: u64,    // Top of user stack
    pub preempt_count: u32, // Preemption disable counter
    pub in_interrupt: bool,
}

impl PerCpu {
    pub const fn new() -> Self {
        PerCpu {
            cpu_id: 0,
            apic_id: 0,
            online: AtomicBool::new(false),
            ticks: AtomicU64::new(0),
            idle_task: 0,
            current_task: 0,
            kernel_stack: 0,
            user_stack: 0,
            preempt_count: 0,
            in_interrupt: false,
        }
    }
}

const PERCPU_INIT: PerCpu = PerCpu::new();
static mut CPU_DATA: [PerCpu; MAX_CPUS] = [PERCPU_INIT; MAX_CPUS];
static mut NEXT_CPU_ID: u32 = 0;

/// Initialize per-CPU data for BSP
pub unsafe fn init_bsp() {
    let apic_id = crate::apic::read_apic_id();

    CPU_DATA[0].cpu_id = 0;
    CPU_DATA[0].apic_id = apic_id;
    CPU_DATA[0].online.store(true, Ordering::SeqCst);

    // Store per-CPU pointer in GS base for fast access
    set_gs_base(&CPU_DATA[0] as *const _ as u64);

    NEXT_CPU_ID = 1;
}

/// Initialize per-CPU data for an application processor
pub unsafe fn init_ap() -> u32 {
    let cpu_id = NEXT_CPU_ID;
    NEXT_CPU_ID += 1;

    if cpu_id as usize >= MAX_CPUS {
        return cpu_id;
    }

    let apic_id = crate::apic::read_apic_id();

    CPU_DATA[cpu_id as usize].cpu_id = cpu_id;
    CPU_DATA[cpu_id as usize].apic_id = apic_id;
    CPU_DATA[cpu_id as usize]
        .online
        .store(true, Ordering::SeqCst);

    // Store per-CPU pointer in GS base
    set_gs_base(&CPU_DATA[cpu_id as usize] as *const _ as u64);

    cpu_id
}

/// Get current CPU ID (fast path via GS register)
#[inline(always)]
pub fn cpu_id() -> usize {
    unsafe {
        let cpu_data = current_cpu();
        (*cpu_data).cpu_id as usize
    }
}

/// Get pointer to current CPU's data structure
#[inline(always)]
pub fn current_cpu() -> *mut PerCpu {
    let ptr: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, gs:0",
            out(reg) ptr,
            options(nostack, nomem, preserves_flags)
        );
    }
    ptr as *mut PerCpu
}

/// Set GS base to point to per-CPU data
unsafe fn set_gs_base(addr: u64) {
    // Use WRMSR to set IA32_GS_BASE (MSR 0xC0000101)
    let low = (addr & 0xFFFFFFFF) as u32;
    let high = (addr >> 32) as u32;

    core::arch::asm!(
        "wrmsr",
        in("ecx") 0xC0000101u32,
        in("eax") low,
        in("edx") high,
        options(nostack, nomem)
    );

    // Also write to GS directly for compatibility
    core::arch::asm!(
        "mov gs:0, {}",
        in(reg) addr,
        options(nostack)
    );
}

/// Get per-CPU data for specific CPU
pub fn cpu_data(cpu_id: usize) -> Option<&'static PerCpu> {
    if cpu_id < MAX_CPUS {
        unsafe { Some(&CPU_DATA[cpu_id]) }
    } else {
        None
    }
}

/// Get mutable per-CPU data for specific CPU
pub fn cpu_data_mut(cpu_id: usize) -> Option<&'static mut PerCpu> {
    if cpu_id < MAX_CPUS {
        unsafe { Some(&mut CPU_DATA[cpu_id]) }
    } else {
        None
    }
}

/// Check if a CPU is online
pub fn is_online(cpu_id: usize) -> bool {
    if let Some(cpu) = cpu_data(cpu_id) {
        cpu.online.load(Ordering::SeqCst)
    } else {
        false
    }
}

/// Disable preemption (increment counter)
pub fn preempt_disable() {
    unsafe {
        let cpu = current_cpu();
        (*cpu).preempt_count += 1;
    }
}

/// Enable preemption (decrement counter, reschedule if zero)
pub fn preempt_enable() {
    unsafe {
        let cpu = current_cpu();
        if (*cpu).preempt_count > 0 {
            (*cpu).preempt_count -= 1;
            if (*cpu).preempt_count == 0 && !(*cpu).in_interrupt {
                crate::scheduler::reschedule();
            }
        }
    }
}

/// Check if preemption is allowed
pub fn can_preempt() -> bool {
    unsafe {
        let cpu = current_cpu();
        (*cpu).preempt_count == 0 && !(*cpu).in_interrupt
    }
}

/// Get total tick count for current CPU
pub fn ticks() -> u64 {
    unsafe {
        let cpu = current_cpu();
        (*cpu).ticks.load(Ordering::Relaxed)
    }
}

/// Get number of online CPUs
pub fn online_count() -> usize {
    let mut count = 0;
    for i in 0..MAX_CPUS {
        if is_online(i) {
            count += 1;
        }
    }
    count
}

/// Execute a function on all online CPUs
pub fn for_each_online<F>(mut f: F)
where
    F: FnMut(usize),
{
    for cpu_id in 0..MAX_CPUS {
        if is_online(cpu_id) {
            f(cpu_id);
        }
    }
}
