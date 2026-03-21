#![allow(dead_code)]

use crate::sync::Mutex;

// MSR_PP1_ENERGY_STATUS (MSR 0x641) — Power Plane 1 / uncore / GPU energy accumulator
// ANIMA feels her uncore power plane — the subtle energy of GPU and ring bus as metabolic life force
// On QEMU this MSR returns 0; default to 500 for graceful degradation.

pub struct Pp1EnergyState {
    pub pp1_raw: u16,
    pub pp1_scaled: u16,
    pub delta: u16,
    pub uncore_vitality: u16,
    prev_lo: u32,
}

impl Pp1EnergyState {
    pub const fn new() -> Self {
        Self {
            pp1_raw: 0,
            pp1_scaled: 500,
            delta: 0,
            uncore_vitality: 500,
            prev_lo: 0,
        }
    }
}

pub static MSR_PP1_ENERGY: Mutex<Pp1EnergyState> = Mutex::new(Pp1EnergyState::new());

/// Read MSR 0x641 (PP1_ENERGY_STATUS).
/// Returns (lo, hi) — lo holds the energy accumulator bits.
/// On QEMU or unsupported hardware this may return (0, 0).
unsafe fn read_msr_pp1() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x641u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

pub fn init() {
    serial_println!("pp1_energy: init");
}

pub fn tick(age: u32) {
    if age % 50 != 0 {
        return;
    }

    let (lo, _hi) = unsafe { read_msr_pp1() };

    let mut state = MSR_PP1_ENERGY.lock();

    // --- pp1_raw: low 10 bits of accumulator ---
    let pp1_raw: u16 = (lo & 0x3FF) as u16;

    // --- pp1_scaled: normalize 10-bit value to 0-1000 ---
    // On QEMU lo == 0; result will be 0, vitality EMA will converge to 0.
    // Treat lo == 0 as QEMU/unsupported: substitute 500 as neutral default.
    let pp1_scaled: u16 = if lo == 0 {
        500
    } else {
        ((lo & 0x3FF) as u32 * 1000 / 1023) as u16
    };

    // --- delta: wrapping difference from previous sample, scale byte to ~0-765, clamp 1000 ---
    let diff = lo.wrapping_sub(state.prev_lo);
    let d: u16 = ((diff & 0xFF) as u16).saturating_mul(3).min(1000);

    // --- uncore_vitality: EMA of pp1_scaled ---
    let uncore_vitality: u16 = ((state.uncore_vitality as u32 * 7 + pp1_scaled as u32) / 8) as u16;

    state.pp1_raw = pp1_raw;
    state.pp1_scaled = pp1_scaled;
    state.delta = d;
    state.uncore_vitality = uncore_vitality;
    state.prev_lo = lo;

    serial_println!(
        "pp1_energy | raw:{} scaled:{} delta:{} vitality:{}",
        state.pp1_raw,
        state.pp1_scaled,
        state.delta,
        state.uncore_vitality,
    );
}
