#![allow(dead_code)]

use crate::sync::Mutex;

const LAPIC_LVT_LINT0: *const u32 = 0xFEE00350 as *const u32;
const LAPIC_LVT_LINT1: *const u32 = 0xFEE00360 as *const u32;

const SAMPLE_RATE: u32 = 77;
const OPENNESS_DELTA_THRESHOLD: u16 = 50;

pub static LAPIC_LINT: Mutex<LapicLintState> = Mutex::new(LapicLintState::new());

pub struct LapicLintState {
    pub lint0_open:     u16,
    pub lint1_open:     u16,
    pub nmi_delivery:   u16,
    pub extint_delivery: u16,
    pub openness:       u16,
    prev_openness:      u16,
}

impl LapicLintState {
    pub const fn new() -> Self {
        Self {
            lint0_open:      0,
            lint1_open:      0,
            nmi_delivery:    0,
            extint_delivery: 0,
            openness:        0,
            prev_openness:   0,
        }
    }

    pub fn tick(&mut self, age: u32) {
        if age % SAMPLE_RATE != 0 {
            return;
        }

        let lint0: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_LINT0) };
        let lint1: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_LINT1) };

        // bit[16] = mask; 0 = unmasked (open), 1 = masked (closed)
        let lint0_open_raw: u16 = if (lint0 >> 16) & 1 == 0 { 1000 } else { 0 };
        let lint1_open_raw: u16 = if (lint1 >> 16) & 1 == 0 { 1000 } else { 0 };

        // bits[10:8] = delivery mode
        let lint0_mode: u32 = (lint0 >> 8) & 0x7;
        let lint1_mode: u32 = (lint1 >> 8) & 0x7;

        // NMI = 0b100 = 4; ExtINT = 0b111 = 7
        let nmi_delivery_raw:   u16 = if lint1_mode == 0b100 { 1000 } else { 0 };
        let extint_delivery_raw: u16 = if lint0_mode == 0b111 { 1000 } else { 0 };

        // EMA smoothing: (old * 7 + new) / 8
        self.lint0_open      = self.lint0_open.wrapping_mul(7).saturating_add(lint0_open_raw) / 8;
        self.lint1_open      = self.lint1_open.wrapping_mul(7).saturating_add(lint1_open_raw) / 8;
        self.nmi_delivery    = self.nmi_delivery.wrapping_mul(7).saturating_add(nmi_delivery_raw) / 8;
        self.extint_delivery = self.extint_delivery.wrapping_mul(7).saturating_add(extint_delivery_raw) / 8;

        // openness = EMA of average of lint0_open and lint1_open
        let avg_open: u16 = self.lint0_open.saturating_add(self.lint1_open) / 2;
        self.openness = self.openness.wrapping_mul(7).saturating_add(avg_open) / 8;

        // Log when openness changes significantly
        let delta = if self.openness > self.prev_openness {
            self.openness.saturating_sub(self.prev_openness)
        } else {
            self.prev_openness.saturating_sub(self.openness)
        };

        if delta > OPENNESS_DELTA_THRESHOLD {
            serial_println!(
                "ANIMA: lint0_open={} lint1_open={} nmi={} extint={}",
                self.lint0_open,
                self.lint1_open,
                self.nmi_delivery,
                self.extint_delivery
            );
            self.prev_openness = self.openness;
        }
    }
}

pub fn init() {
    let mut state = LAPIC_LINT.lock();
    // Perform an initial read to seed state at boot
    let lint0: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_LINT0) };
    let lint1: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_LINT1) };

    state.lint0_open      = if (lint0 >> 16) & 1 == 0 { 1000 } else { 0 };
    state.lint1_open      = if (lint1 >> 16) & 1 == 0 { 1000 } else { 0 };

    let lint0_mode: u32 = (lint0 >> 8) & 0x7;
    let lint1_mode: u32 = (lint1 >> 8) & 0x7;

    state.nmi_delivery    = if lint1_mode == 0b100 { 1000 } else { 0 };
    state.extint_delivery = if lint0_mode == 0b111 { 1000 } else { 0 };

    state.openness      = state.lint0_open.saturating_add(state.lint1_open) / 2;
    state.prev_openness = state.openness;

    serial_println!(
        "ANIMA: lapic_lint init lint0_open={} lint1_open={} nmi={} extint={}",
        state.lint0_open,
        state.lint1_open,
        state.nmi_delivery,
        state.extint_delivery
    );
}

pub fn tick(age: u32) {
    LAPIC_LINT.lock().tick(age);
}
