#![allow(dead_code)]

use crate::sync::Mutex;

pub static LAPIC_TMR_HI: Mutex<LapicTmrHiState> = Mutex::new(LapicTmrHiState::new());

pub struct LapicTmrHiState {
    pub high_level: u16,
    pub very_high_level: u16,
    pub deep_level_ratio: u16,
    pub deep_trigger_sense: u16,
}

impl LapicTmrHiState {
    pub const fn new() -> Self {
        Self {
            high_level: 0,
            very_high_level: 0,
            deep_level_ratio: 0,
            deep_trigger_sense: 0,
        }
    }
}

pub fn init() {
    serial_println!("lapic_tmr_hi: init");
}

pub fn tick(age: u32) {
    if age % 17 != 0 {
        return;
    }

    let tmr3 = unsafe { core::ptr::read_volatile(0xFEE001B0usize as *const u32) };
    let tmr4 = unsafe { core::ptr::read_volatile(0xFEE001C0usize as *const u32) };
    let tmr5 = unsafe { core::ptr::read_volatile(0xFEE001D0usize as *const u32) };
    let tmr6 = unsafe { core::ptr::read_volatile(0xFEE001E0usize as *const u32) };
    let tmr7 = unsafe { core::ptr::read_volatile(0xFEE001F0usize as *const u32) };

    // Signal 1: high_level — level-triggered count across vectors 96-191 (TMR3+TMR4+TMR5)
    let n_hi = tmr3.count_ones() + tmr4.count_ones() + tmr5.count_ones();
    let high_level = (n_hi as u32 * 1000 / 96).min(1000) as u16;

    // Signal 2: very_high_level — level-triggered count across vectors 192-255 (TMR6+TMR7)
    let n_vhi = tmr6.count_ones() + tmr7.count_ones();
    let very_high_level = (n_vhi as u32 * 1000 / 64).min(1000) as u16;

    // Signal 3: deep_level_ratio — level-triggered fraction across all 5 TMR registers (160 bits)
    let all = tmr3.count_ones()
        + tmr4.count_ones()
        + tmr5.count_ones()
        + tmr6.count_ones()
        + tmr7.count_ones();
    let deep_level_ratio = (all as u32 * 1000 / 160).min(1000) as u16;

    // Signal 4: deep_trigger_sense — EMA of deep_level_ratio, weight 7/8 old + 1/8 new
    let mut state = LAPIC_TMR_HI.lock();
    let deep_trigger_sense = (state.deep_trigger_sense * 7).saturating_add(deep_level_ratio) / 8;

    state.high_level = high_level;
    state.very_high_level = very_high_level;
    state.deep_level_ratio = deep_level_ratio;
    state.deep_trigger_sense = deep_trigger_sense;

    serial_println!(
        "lapic_tmr_hi | high:{} very_high:{} ratio:{} sense:{}",
        high_level,
        very_high_level,
        deep_level_ratio,
        deep_trigger_sense
    );
}
