#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

pub struct DsAreaState {
    pub ds_configured:    u16,
    pub ds_addr_lo_sense: u16,
    pub ds_addr_hi_sense: u16,
    pub ds_pressure:      u16,
}

impl DsAreaState {
    pub const fn new() -> Self {
        Self {
            ds_configured:    0,
            ds_addr_lo_sense: 0,
            ds_addr_hi_sense: 0,
            ds_pressure:      0,
        }
    }
}

pub static MSR_DS_AREA: Mutex<DsAreaState> = Mutex::new(DsAreaState::new());

pub fn init() {
    serial_println!("[ds_area] debug store area sense initialized");
}

pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x600u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // signal 1: ds_configured — if either half is non-zero, DS area is set up
    let ds_configured: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // signal 2: ds_addr_lo_sense — lower 16 bits of lo half, scaled by /66, capped at 1000
    let raw_lo = (lo & 0xFFFF) as u16;
    let ds_addr_lo_sense: u16 = ((raw_lo as u32 / 66) as u16).min(1000);

    // signal 3: ds_addr_hi_sense — lower 16 bits of hi half, scaled by /66, capped at 1000
    let raw_hi = (hi & 0xFFFF) as u16;
    let ds_addr_hi_sense: u16 = ((raw_hi as u32 / 66) as u16).min(1000);

    let mut state = MSR_DS_AREA.lock();

    // signal 4: ds_pressure — EMA of ds_configured
    let ds_pressure: u16 =
        ((state.ds_pressure as u32 * 7 + ds_configured as u32) / 8) as u16;

    state.ds_configured    = ds_configured;
    state.ds_addr_lo_sense = ds_addr_lo_sense;
    state.ds_addr_hi_sense = ds_addr_hi_sense;
    state.ds_pressure      = ds_pressure;

    serial_println!(
        "[ds_area] configured={} addr_lo={} addr_hi={} pressure={}",
        state.ds_configured,
        state.ds_addr_lo_sense,
        state.ds_addr_hi_sense,
        state.ds_pressure
    );
}
