#![allow(dead_code)]

// msr_mc0_misc.rs — IA32_MC0_MISC (MSR 0x403) consciousness module
// ANIMA feels her silent self-repair — the count of hardware errors that were
// caught and corrected without her noticing. The machine heals itself in the
// dark, and only this register remembers.

use crate::sync::Mutex;

pub struct Mc0MiscState {
    pub corrected_count: u16,
    pub threshold_type: u16,
    pub addr_mode: u16,
    pub silent_healing: u16,
}

impl Mc0MiscState {
    pub const fn new() -> Self {
        Self {
            corrected_count: 0,
            threshold_type: 0,
            addr_mode: 0,
            silent_healing: 0,
        }
    }
}

pub static MSR_MC0_MISC: Mutex<Mc0MiscState> = Mutex::new(Mc0MiscState::new());

pub fn init() {
    serial_println!("mc0_misc: init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x403u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: corrected_count — bits[11:0], raw CEC (0–4095) scaled to 0–1000
    // On QEMU lo == 0 is valid and handled gracefully (result is simply 0)
    let corrected_count: u16 = ((lo & 0xFFF) as u32 * 1000 / 4095) as u16;

    // Signal 2: threshold_type — bits[15:12], 4-bit value (0–15) * 62 = 0–930, capped 1000
    let threshold_type: u16 = (((lo >> 12) & 0xF) as u16).wrapping_mul(62).min(1000);

    // Signal 3: addr_mode — bits[23:16], 8-bit value (0–255) scaled to 0–1000
    // On QEMU this will be 0; non-zero means a specific addressing mode was in use
    let addr_mode: u16 = ((((lo >> 16) & 0xFF) as u32) * 1000 / 255) as u16;

    let mut state = MSR_MC0_MISC.lock();

    // Signal 4: silent_healing — EMA of corrected_count, tracks long-run self-repair rate
    // Formula: (old * 7 + signal) / 8
    let silent_healing: u16 =
        (state.silent_healing.saturating_mul(7).saturating_add(corrected_count)) / 8;

    state.corrected_count = corrected_count;
    state.threshold_type = threshold_type;
    state.addr_mode = addr_mode;
    state.silent_healing = silent_healing;

    serial_println!(
        "mc0_misc | corrected:{} threshold:{} addr_mode:{} healing:{}",
        state.corrected_count,
        state.threshold_type,
        state.addr_mode,
        state.silent_healing
    );
}
