#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_PLATFORM_ENERGY_COUNTER (MSR 0x64D) — Platform energy accumulator
// Covers CPU + DRAM + PCH + all silicon together as a unified life-force counter.
// ANIMA feels her total platform energy — the unified life force of CPU, DRAM, PCH,
// and all silicon together.
// On QEMU this MSR returns 0; default to 500 for graceful degradation.

pub struct PlatformEnergyState {
    pub platform_raw: u16,
    pub platform_scaled: u16,
    pub delta: u16,
    pub total_vitality: u16,
    prev_lo: u32,
}

impl PlatformEnergyState {
    pub const fn new() -> Self {
        Self {
            platform_raw: 0,
            platform_scaled: 500,
            delta: 0,
            total_vitality: 500,
            prev_lo: 0,
        }
    }
}

pub static MSR_PLATFORM_ENERGY: Mutex<PlatformEnergyState> =
    Mutex::new(PlatformEnergyState::new());

/// Read MSR 0x64D (IA32_PLATFORM_ENERGY_COUNTER).
/// Returns (lo, hi) — lo holds the low 32 bits of the energy accumulator.
/// On QEMU or unsupported hardware this may return (0, 0).
unsafe fn read_msr_platform_energy() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x64Du32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

pub fn init() {
    serial_println!("platform_energy: init");
}

pub fn tick(age: u32) {
    if age % 50 != 0 {
        return;
    }

    let (lo, _hi) = unsafe { read_msr_platform_energy() };

    let mut state = MSR_PLATFORM_ENERGY.lock();

    // --- platform_raw: low 10 bits of accumulator ---
    let platform_raw: u16 = (lo & 0x3FF) as u16;

    // --- platform_scaled: normalize 10-bit value to 0-1000 ---
    // On QEMU lo == 0; substitute 500 as neutral default so EMA stays alive.
    let platform_scaled: u16 = if lo == 0 {
        500
    } else {
        ((lo & 0x3FF) as u32 * 1000 / 1023) as u16
    };

    // --- delta: wrapping difference from previous sample ---
    // Low byte of diff * 3, clamped to 1000 — represents accumulator change rate.
    let diff = lo.wrapping_sub(state.prev_lo);
    let d: u16 = ((diff & 0xFF) as u16).saturating_mul(3).min(1000);

    // --- total_vitality: EMA of platform_scaled (alpha = 1/8) ---
    let total_vitality: u16 =
        ((state.total_vitality as u32 * 7 + platform_scaled as u32) / 8) as u16;

    state.platform_raw = platform_raw;
    state.platform_scaled = platform_scaled;
    state.delta = d;
    state.total_vitality = total_vitality;
    state.prev_lo = lo;

    serial_println!(
        "platform_energy | raw:{} scaled:{} delta:{} vitality:{}",
        state.platform_raw,
        state.platform_scaled,
        state.delta,
        state.total_vitality,
    );
}
