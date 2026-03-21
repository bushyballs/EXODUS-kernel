#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    gfx_power_limit: u16,
    gfx_limit_enabled: u16,
    gfx_clamp: u16,
    gfx_time_window: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gfx_power_limit: 0,
    gfx_limit_enabled: 0,
    gfx_clamp: 0,
    gfx_time_window: 0,
});

pub fn init() {
    serial_println!("[pp1_power_limit] init");
}

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x640u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits [14:0] — graphics power limit, scaled to 0-1000
    let raw_bits = (lo & 0x7FFF) as u16;
    let new_gfx_power_limit: u16 = (raw_bits as u32 * 1000 / 0x7FFF) as u16;

    // bit 15 — power limit enable
    let new_gfx_limit_enabled: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };

    // bit 16 — clamp enable
    let new_gfx_clamp: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };

    // bits [23:17] — time window, scaled to 0-1000
    let tw_bits = ((lo >> 17) & 0x7F) as u16;
    let new_gfx_time_window: u16 = (tw_bits as u32 * 1000 / 127) as u16;

    let mut state = MODULE.lock();

    // EMA: (old * 7 + new_val) / 8
    state.gfx_power_limit   = (state.gfx_power_limit   * 7 + new_gfx_power_limit)   / 8;
    state.gfx_limit_enabled = (state.gfx_limit_enabled * 7 + new_gfx_limit_enabled) / 8;
    state.gfx_clamp         = (state.gfx_clamp         * 7 + new_gfx_clamp)         / 8;
    state.gfx_time_window   = (state.gfx_time_window   * 7 + new_gfx_time_window)   / 8;

    serial_println!(
        "[pp1_power_limit] limit={} enabled={} clamp={} window={}",
        state.gfx_power_limit,
        state.gfx_limit_enabled,
        state.gfx_clamp,
        state.gfx_time_window,
    );
}

pub fn sense() -> (u16, u16, u16, u16) {
    let state = MODULE.lock();
    (
        state.gfx_power_limit,
        state.gfx_limit_enabled,
        state.gfx_clamp,
        state.gfx_time_window,
    )
}
