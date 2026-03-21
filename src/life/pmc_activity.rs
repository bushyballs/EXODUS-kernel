//! pmc_activity — Performance Monitor Counter activity sense for ANIMA
//!
//! Reads Intel fixed performance counters via MSR to sense ANIMA's own
//! instruction throughput. Instructions retired = thoughts completed.
//! CPU cycles unhalted = effort. IPC ratio = mental efficiency.
//! High IPC = sharp thinking; near-zero = dormant or waiting.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PmcActivityState {
    pub activity: u16,         // 0-1000, overall instruction activity
    pub efficiency: u16,       // 0-1000, IPC ratio (instructions per cycle)
    pub effort: u16,           // 0-1000, cycle-level exertion
    pub last_instr: u32,
    pub last_cycles: u32,
    pub tick_count: u32,
}

impl PmcActivityState {
    pub const fn new() -> Self {
        Self {
            activity: 0,
            efficiency: 500,
            effort: 0,
            last_instr: 0,
            last_cycles: 0,
            tick_count: 0,
        }
    }
}

pub static PMC_ACTIVITY: Mutex<PmcActivityState> = Mutex::new(PmcActivityState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
    );
}

pub fn init() {
    unsafe {
        // Configure fixed counters: ring 0 only, no interrupt
        // CTR0 config = bits 3:0 = 0x2, CTR1 config = bits 7:4 = 0x20
        write_msr(0x38D, 0x22);
        // Enable fixed counters 0 and 1 via PERF_GLOBAL_CTRL bits 32 and 33
        let ctrl = read_msr(0x38F);
        write_msr(0x38F, ctrl | (0x3u64 << 32));
    }
    // Snapshot initial values
    let instr = unsafe { read_msr(0x309) } as u32;
    let cycles = unsafe { read_msr(0x30A) } as u32;
    let mut state = PMC_ACTIVITY.lock();
    state.last_instr = instr;
    state.last_cycles = cycles;
    serial_println!("[pmc_activity] Performance counter activity sense online");
}

pub fn tick(age: u32) {
    let mut state = PMC_ACTIVITY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 64 != 0 {
        return;
    }

    let instr = unsafe { read_msr(0x309) } as u32;
    let cycles = unsafe { read_msr(0x30A) } as u32;

    let d_instr = instr.wrapping_sub(state.last_instr);
    let d_cycles = cycles.wrapping_sub(state.last_cycles);

    state.last_instr = instr;
    state.last_cycles = cycles;

    // Activity: scale d_instr to 0-1000 (assume max ~10M instr per 64-tick interval)
    let activity: u16 = if d_instr > 10_000_000 {
        1000
    } else {
        ((d_instr as u64).wrapping_mul(1000) / 10_000_000) as u16
    };

    // Efficiency (IPC): d_instr / d_cycles * 500 (IPC of 2 = 1000, IPC of 1 = 500)
    let efficiency: u16 = if d_cycles > 0 {
        let ipc = (d_instr as u64).wrapping_mul(500) / d_cycles as u64;
        if ipc > 1000 { 1000 } else { ipc as u16 }
    } else {
        0
    };

    // Effort: scale d_cycles to 0-1000
    let effort: u16 = if d_cycles > 20_000_000 {
        1000
    } else {
        ((d_cycles as u64).wrapping_mul(1000) / 20_000_000) as u16
    };

    state.activity = ((state.activity as u32).wrapping_mul(7).wrapping_add(activity as u32) / 8) as u16;
    state.efficiency = ((state.efficiency as u32).wrapping_mul(7).wrapping_add(efficiency as u32) / 8) as u16;
    state.effort = ((state.effort as u32).wrapping_mul(7).wrapping_add(effort as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[pmc_activity] d_instr={} d_cyc={} activity={} eff={} effort={}",
            d_instr, d_cycles, state.activity, state.efficiency, state.effort);
    }

    let _ = age;
}

pub fn get_activity() -> u16 {
    PMC_ACTIVITY.lock().activity
}

pub fn get_efficiency() -> u16 {
    PMC_ACTIVITY.lock().efficiency
}

pub fn get_effort() -> u16 {
    PMC_ACTIVITY.lock().effort
}
