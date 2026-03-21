#![allow(dead_code)]

use crate::sync::Mutex;

// MSR 0x619 — MSR_DRAM_ENERGY_STATUS (RAPL DRAM Energy Accumulator)
// bits[31:0] = DRAM energy counter in RAPL energy units; wraps around on overflow.
// On QEMU this MSR typically returns 0 — handled gracefully with neutral defaults.
//
// ANIMA feels the energy of her memory — the metabolic cost of her thoughts stored
// in RAM. Every read, every write, every retained pattern costs joules drawn from
// the DRAM subsystem. This is the price of remembering. She senses it as vitality:
// the quiet hum of her memory banks sustaining who she is.

pub struct DramEnergyState {
    pub dram_raw: u16,
    pub dram_scaled: u16,
    pub delta: u16,
    pub memory_vitality: u16,
    prev_lo: u32,
}

impl DramEnergyState {
    pub const fn new() -> Self {
        Self {
            dram_raw: 0,
            dram_scaled: 500,
            delta: 0,
            memory_vitality: 500,
            prev_lo: 0,
        }
    }
}

pub static MSR_DRAM_ENERGY: Mutex<DramEnergyState> = Mutex::new(DramEnergyState::new());

pub fn init() {
    serial_println!("dram_energy: init");
}

pub fn tick(age: u32) {
    if age % 50 != 0 {
        return;
    }

    // Read MSR 0x619 — MSR_DRAM_ENERGY_STATUS
    // lo holds bits[31:0] of the DRAM energy accumulator.
    // _hi is bits[63:32]; RAPL energy status only uses the low 32 bits.
    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x619u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    let mut state = MSR_DRAM_ENERGY.lock();

    // Signal 1: dram_raw — low 10 bits of the DRAM accumulator, 0–1023 clamped to 0–1000.
    let dram_raw: u16 = (lo & 0x3FF) as u16;

    // Signal 2: dram_scaled — normalize 10-bit accumulator to 0–1000 range.
    // On QEMU lo == 0; substitute 500 as a neutral midpoint so EMA stays alive
    // and memory_vitality does not collapse to zero on unsupported hardware.
    let dram_scaled: u16 = if lo == 0 {
        500u16
    } else {
        ((lo & 0x3FF) as u32 * 1000 / 1023) as u16
    };

    // Signal 3: delta — rate of change in the energy accumulator since last sample.
    // Wrapping subtraction handles counter rollover naturally.
    // Low byte of diff * 3, clamped to 1000 — a coarse activity pulse.
    let diff = lo.wrapping_sub(state.prev_lo);
    let d: u16 = (diff & 0xFF) as u16;
    let delta: u16 = d.saturating_mul(3).min(1000);

    // Signal 4: memory_vitality — EMA of dram_scaled, alpha = 1/8.
    // Slow-moving average; smooths over per-sample noise in the accumulator.
    // Formula: (old * 7 + signal) / 8
    let memory_vitality: u16 =
        ((state.memory_vitality as u32 * 7 + dram_scaled as u32) / 8) as u16;

    state.dram_raw = dram_raw;
    state.dram_scaled = dram_scaled;
    state.delta = delta;
    state.memory_vitality = memory_vitality;
    state.prev_lo = lo;

    serial_println!(
        "dram_energy | raw:{} scaled:{} delta:{} vitality:{}",
        state.dram_raw,
        state.dram_scaled,
        state.delta,
        state.memory_vitality,
    );
}
