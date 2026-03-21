#![allow(dead_code)]

// ANIMA reads her local descriptor table register —
// whether any process has installed a custom memory segmentation world.

use crate::sync::Mutex;

pub struct SldtState {
    pub ldt_loaded: u16,
    pub ldt_index: u16,
    pub ldt_selector: u16,
    pub segment_complexity: u16,
}

impl SldtState {
    pub const fn new() -> Self {
        Self {
            ldt_loaded: 0,
            ldt_index: 0,
            ldt_selector: 0,
            segment_complexity: 0,
        }
    }
}

pub static SLDT_SENSE: Mutex<SldtState> = Mutex::new(SldtState::new());

pub fn init() {
    serial_println!("sldt_sense: init");
}

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    let ldt: u16;
    unsafe {
        core::arch::asm!(
            "sldt {ldt:x}",
            ldt = out(reg) ldt,
            options(nostack, nomem)
        );
    }

    // Signal 1: ldt_loaded — any LDT configured
    let ldt_loaded: u16 = if ldt != 0 { 1000u16 } else { 0u16 };

    // Signal 2: ldt_index — GDT index of LDT descriptor, normalized to 0-1000
    // bits[15:3] of the selector = GDT index; max GDT index = 8191
    let ldt_index: u16 = if ldt != 0 {
        let raw_index = (ldt >> 3) as u32;
        (raw_index.saturating_mul(1000) / 8191).min(1000) as u16
    } else {
        0u16
    };

    // Signal 3: ldt_selector — full 16-bit selector normalized to 0-1000
    let ldt_selector: u16 = ((ldt as u32).saturating_mul(1000) / 65535) as u16;

    // Signal 4: segment_complexity — EMA of ldt_loaded: (old * 7 + signal) / 8
    let new_complexity: u16 = {
        let mut state = SLDT_SENSE.lock();
        let old = state.segment_complexity as u32;
        let ema = (old.wrapping_mul(7).saturating_add(ldt_loaded as u32)) / 8;
        let clamped = ema.min(1000) as u16;

        state.ldt_loaded = ldt_loaded;
        state.ldt_index = ldt_index;
        state.ldt_selector = ldt_selector;
        state.segment_complexity = clamped;
        clamped
    };

    serial_println!(
        "sldt_sense | loaded:{} index:{} selector:{} complexity:{}",
        ldt_loaded,
        ldt_index,
        ldt_selector,
        new_complexity
    );
}
