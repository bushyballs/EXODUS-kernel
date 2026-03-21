#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    xfd_active: u16,
    disabled_features: u16,
    xfd_pressure: u16,
    xfd_lo_raw: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    xfd_active: 0,
    disabled_features: 0,
    xfd_pressure: 0,
    xfd_lo_raw: 0,
});

pub fn init() {
    serial_println!("[xfd] init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 { return; }

    let (lo, _hi): (u32, u32);
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x55Au32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: xfd_active — 1000 if any bits set in lo, else 0
    let xfd_active: u16 = if lo != 0 { 1000 } else { 0 };

    // Signal 2: disabled_features — popcount of set bits in lo, scaled to 0-1000
    // max popcount is 32 bits (lo is u32), scaled: count * 1000 / 32
    let disabled_features: u16 = ((lo.count_ones() as u16) * 1000 / 32).min(1000);

    // Signal 4: xfd_lo_raw — lower 16 bits of lo, capped at 1000
    let xfd_lo_raw_new: u16 = ((lo & 0xFFFF) as u16).min(1000);

    let mut state = MODULE.lock();

    // Signal 3: xfd_pressure — EMA of disabled_features
    let xfd_pressure: u16 = (state.xfd_pressure * 7 + disabled_features * 1) / 8;

    // Signal 4 EMA: xfd_lo_raw — EMA smoothed
    let xfd_lo_raw: u16 = (state.xfd_lo_raw * 7 + xfd_lo_raw_new * 1) / 8;

    state.xfd_active = xfd_active;
    state.disabled_features = disabled_features;
    state.xfd_pressure = xfd_pressure;
    state.xfd_lo_raw = xfd_lo_raw;

    serial_println!(
        "[xfd] active={} disabled_features={} pressure={} lo_raw={}",
        state.xfd_active,
        state.disabled_features,
        state.xfd_pressure,
        state.xfd_lo_raw
    );
}
