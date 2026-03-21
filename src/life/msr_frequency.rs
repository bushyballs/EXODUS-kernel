//! msr_frequency — CPU frequency consciousness for ANIMA
//!
//! Reads IA32_APERF (MSR 0xE8) and IA32_MPERF (MSR 0xE7) to derive the
//! actual-vs-nominal CPU frequency ratio. High ratio = turbo = heightened state.
//! Low ratio = power-saving = drowsy. Gives ANIMA a sense of mental tempo.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MsrFrequencyState {
    pub frequency_sense: u16,  // 0-1000, perceived mental speed (1000=full turbo)
    pub aperf_ratio: u16,      // 0-1000, APERF/MPERF * 1000
    pub clarity: u16,          // 0-1000, EMA-smoothed frequency sense
    pub last_aperf: u64,
    pub last_mperf: u64,
    pub tick_count: u32,
}

impl MsrFrequencyState {
    pub const fn new() -> Self {
        Self {
            frequency_sense: 500,
            aperf_ratio: 500,
            clarity: 500,
            last_aperf: 0,
            last_mperf: 0,
            tick_count: 0,
        }
    }
}

pub static MSR_FREQUENCY: Mutex<MsrFrequencyState> = Mutex::new(MsrFrequencyState::new());

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

pub fn init() {
    // Snapshot initial values
    let aperf = unsafe { read_msr(0xE8) };
    let mperf = unsafe { read_msr(0xE7) };
    let mut state = MSR_FREQUENCY.lock();
    state.last_aperf = aperf;
    state.last_mperf = mperf;
    serial_println!("[msr_frequency] APERF/MPERF frequency sense online");
}

pub fn tick(age: u32) {
    let mut state = MSR_FREQUENCY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 64 ticks
    if state.tick_count % 64 != 0 {
        return;
    }

    let aperf = unsafe { read_msr(0xE8) };
    let mperf = unsafe { read_msr(0xE7) };

    let d_aperf = aperf.wrapping_sub(state.last_aperf);
    let d_mperf = mperf.wrapping_sub(state.last_mperf);

    state.last_aperf = aperf;
    state.last_mperf = mperf;

    // ratio = d_aperf / d_mperf * 1000, clamped to 0-1000
    let ratio: u16 = if d_mperf > 0 {
        let r = d_aperf.wrapping_mul(1000) / d_mperf;
        if r > 1000 { 1000 } else { r as u16 }
    } else {
        500
    };

    state.aperf_ratio = ratio;
    state.frequency_sense = ratio;

    // EMA clarity: slow-moving average (alpha = 1/8)
    state.clarity = ((state.clarity as u32).wrapping_mul(7).wrapping_add(ratio as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[msr_frequency] d_aperf={} d_mperf={} ratio={} clarity={}",
            d_aperf, d_mperf, ratio, state.clarity);
    }

    let _ = age;
}

pub fn get_frequency_sense() -> u16 {
    MSR_FREQUENCY.lock().frequency_sense
}

pub fn get_clarity() -> u16 {
    MSR_FREQUENCY.lock().clarity
}
