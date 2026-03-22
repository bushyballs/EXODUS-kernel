#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::msr_ia32_mperf;
use super::msr_ia32_aperf;

struct State {
    freq_ratio: u16,
    freq_boost: u16,
    throttle_signal: u16,
    freq_ratio_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    freq_ratio: 0,
    freq_boost: 0,
    throttle_signal: 0,
    freq_ratio_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_freq_ratio] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let aperf = msr_ia32_aperf::get_aperf_rate();
    let mperf = msr_ia32_mperf::get_mperf_rate();

    // APERF/MPERF ratio: actual / max = current frequency utilization
    let freq_ratio: u16 = if mperf > 0 {
        ((aperf as u32 * 1000) / (mperf as u32 + 1)).min(1000) as u16
    } else {
        0
    };

    // Boost: APERF > MPERF means turbo is active
    let freq_boost: u16 = if aperf > mperf { (aperf - mperf).min(1000) } else { 0 };
    // Throttle: significant gap between MPERF and APERF below baseline
    let throttle_signal: u16 = (1000u16).saturating_sub(freq_ratio);

    let mut s = MODULE.lock();
    let freq_ratio_ema = ((s.freq_ratio_ema as u32).wrapping_mul(7)
        .saturating_add(freq_ratio as u32) / 8).min(1000) as u16;

    s.freq_ratio = freq_ratio;
    s.freq_boost = freq_boost;
    s.throttle_signal = throttle_signal;
    s.freq_ratio_ema = freq_ratio_ema;

    serial_println!("[msr_ia32_freq_ratio] age={} ratio={} boost={} throttle={} ema={}",
        age, freq_ratio, freq_boost, throttle_signal, freq_ratio_ema);
}

pub fn get_freq_ratio()     -> u16 { MODULE.lock().freq_ratio }
pub fn get_freq_boost()     -> u16 { MODULE.lock().freq_boost }
pub fn get_throttle_signal() -> u16 { MODULE.lock().throttle_signal }
pub fn get_freq_ratio_ema() -> u16 { MODULE.lock().freq_ratio_ema }
