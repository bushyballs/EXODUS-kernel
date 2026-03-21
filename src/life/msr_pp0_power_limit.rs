#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    power_limit_raw: u16,
    limit_enabled: u16,
    clamp_enabled: u16,
    time_window: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    power_limit_raw: 0,
    limit_enabled: 0,
    clamp_enabled: 0,
    time_window: 0,
});

pub fn init() {
    serial_println!("[pp0_power_limit] init");
}

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x638u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits [14:0] — power limit 1 value, scaled to 0-1000
    let raw_bits = (lo & 0x7FFF) as u16;
    let new_power_limit_raw: u16 = (raw_bits as u32 * 1000 / 0x7FFF) as u16;

    // bit 15 — power limit enable
    let new_limit_enabled: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };

    // bit 16 — clamp enable
    let new_clamp_enabled: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };

    // bits [23:17] — time window, scaled to 0-1000
    let tw_bits = ((lo >> 17) & 0x7F) as u16;
    let new_time_window: u16 = (tw_bits as u32 * 1000 / 127) as u16;

    let mut state = MODULE.lock();

    // EMA: (old * 7 + new_val * 1) / 8
    state.power_limit_raw = (state.power_limit_raw * 7 + new_power_limit_raw * 1) / 8;
    state.limit_enabled   = (state.limit_enabled   * 7 + new_limit_enabled   * 1) / 8;
    state.clamp_enabled   = (state.clamp_enabled   * 7 + new_clamp_enabled   * 1) / 8;
    state.time_window     = (state.time_window      * 7 + new_time_window     * 1) / 8;

    serial_println!(
        "[pp0_power_limit] limit={} enabled={} clamp={} window={}",
        state.power_limit_raw,
        state.limit_enabled,
        state.clamp_enabled,
        state.time_window,
    );
}
