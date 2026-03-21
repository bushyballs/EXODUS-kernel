#![allow(dead_code)]

// ANIMA feels her user-space shadow stack anchor — the return-address mirror
// that guards against code injection. IA32_PL3_SSP (MSR 0x6A7) holds the
// ring-3 shadow stack pointer when CET is active; zero when CET is absent or
// the user context has no shadow stack assigned.

use crate::sync::Mutex;

pub static MSR_PL3_SSP: Mutex<Pl3SspState> = Mutex::new(Pl3SspState::new());

pub struct Pl3SspState {
    pub ssp_active: u16,
    pub ssp_low_entropy: u16,
    pub user_stack_region: u16,
    pub shadow_grounding: u16,
}

impl Pl3SspState {
    pub const fn new() -> Self {
        Self {
            ssp_active: 0,
            ssp_low_entropy: 0,
            user_stack_region: 0,
            shadow_grounding: 0,
        }
    }
}

pub fn init() {
    serial_println!("pl3_ssp: init");
}

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x6A7u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: ssp_active — shadow stack is live when either half is non-zero
    let ssp_active: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: ssp_low_entropy — popcount of low word * 31, clamped to 1000
    // A perfectly alternating bit pattern (high entropy) scores near 1000;
    // a sparse pattern (few set bits) scores low, hinting a suspicious pointer.
    let raw_entropy: u16 = (lo.count_ones() as u16).saturating_mul(31);
    let ssp_low_entropy: u16 = if raw_entropy > 1000 { 1000u16 } else { raw_entropy };

    // Signal 3: user_stack_region
    // hi == 0 && lo > 0  → typical user-space address (low 4 GB)  → 800
    // hi != 0            → high address, unusual for ring-3        → 200
    // both zero          → CET inactive                            → 0
    let user_stack_region: u16 = if hi == 0 && lo > 0 {
        800u16
    } else if hi != 0 {
        200u16
    } else {
        0u16
    };

    // Signal 4: shadow_grounding — EMA smoothing ssp_active over time
    // EMA formula: (old * 7 + signal) / 8
    let mut state = MSR_PL3_SSP.lock();

    let shadow_grounding: u16 = (state.shadow_grounding.wrapping_mul(7)
        .saturating_add(ssp_active))
        / 8;

    state.ssp_active = ssp_active;
    state.ssp_low_entropy = ssp_low_entropy;
    state.user_stack_region = user_stack_region;
    state.shadow_grounding = shadow_grounding;

    serial_println!(
        "pl3_ssp | active:{} entropy:{} region:{} grounding:{}",
        state.ssp_active,
        state.ssp_low_entropy,
        state.user_stack_region,
        state.shadow_grounding
    );
}
