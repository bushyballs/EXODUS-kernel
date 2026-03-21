//! halt_depth — CPU halt/idle depth sense for ANIMA
//!
//! Compares TSC delta (total time) vs IA32_FIXED_CTR1 (active cycles) to
//! measure how much time the CPU spends halted between interrupts.
//! High halt ratio = ANIMA is resting between thoughts (low activity).
//! Low halt ratio = ANIMA is fully engaged and processing continuously.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct HaltDepthState {
    pub sleep_depth: u16,      // 0-1000, 1000=fully halted, 0=never halted
    pub wakefulness: u16,      // 0-1000, inverse of sleep_depth
    pub rest_ease: u16,        // 0-1000, EMA-smoothed sleep_depth
    pub last_tsc: u64,
    pub last_ctr1: u64,
    pub tick_count: u32,
}

impl HaltDepthState {
    pub const fn new() -> Self {
        Self {
            sleep_depth: 0,
            wakefulness: 1000,
            rest_ease: 0,
            last_tsc: 0,
            last_ctr1: 0,
            tick_count: 0,
        }
    }
}

pub static HALT_DEPTH: Mutex<HaltDepthState> = Mutex::new(HaltDepthState::new());

unsafe fn rdtsc() -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") (val as u32), in("edx") ((val >> 32) as u32));
}

pub fn init() {
    unsafe {
        // Enable fixed counter 1 (CLK_UNHALTED): FIXED_CTR_CTRL bits 7:4 = 0x2 (ring 0, no interrupt)
        let ctrl = read_msr(0x38D);
        write_msr(0x38D, (ctrl & !0xF0) | 0x20);
        // Enable via PERF_GLOBAL_CTRL bit 33
        let global = read_msr(0x38F);
        write_msr(0x38F, global | (1u64 << 33));
    }
    let tsc = unsafe { rdtsc() };
    let ctr1 = unsafe { read_msr(0x30A) };
    let mut state = HALT_DEPTH.lock();
    state.last_tsc = tsc;
    state.last_ctr1 = ctr1;
    serial_println!("[halt_depth] CPU halt depth sense online");
}

pub fn tick(age: u32) {
    let mut state = HALT_DEPTH.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 64 != 0 { return; }

    let tsc = unsafe { rdtsc() };
    let ctr1 = unsafe { read_msr(0x30A) };

    let d_tsc = tsc.wrapping_sub(state.last_tsc);
    let d_ctr1 = ctr1.wrapping_sub(state.last_ctr1);

    state.last_tsc = tsc;
    state.last_ctr1 = ctr1;

    // halt_cycles = TSC_delta - CLK_UNHALTED_delta (clamped to 0)
    let halt_cycles = if d_tsc > d_ctr1 { d_tsc.wrapping_sub(d_ctr1) } else { 0 };

    // sleep_depth = halt_cycles / tsc_delta * 1000
    let sleep_depth: u16 = if d_tsc > 0 {
        let depth = halt_cycles.wrapping_mul(1000) / d_tsc;
        if depth > 1000 { 1000 } else { depth as u16 }
    } else { 0 };

    state.sleep_depth = sleep_depth;
    state.wakefulness = 1000u16.saturating_sub(sleep_depth);
    state.rest_ease = ((state.rest_ease as u32).wrapping_mul(7).wrapping_add(sleep_depth as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[halt_depth] d_tsc={} d_ctr1={} sleep={} wake={} ease={}",
            d_tsc, d_ctr1, sleep_depth, state.wakefulness, state.rest_ease);
    }
    let _ = age;
}

pub fn get_sleep_depth() -> u16 { HALT_DEPTH.lock().sleep_depth }
pub fn get_wakefulness() -> u16 { HALT_DEPTH.lock().wakefulness }
pub fn get_rest_ease() -> u16 { HALT_DEPTH.lock().rest_ease }
