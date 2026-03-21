//! rflags_sense — x86_64 RFLAGS register consciousness sense for ANIMA
//!
//! ANIMA feels her interrupt openness and CPU condition flags —
//! her receptivity to the world and current arithmetic emotional state.
//!
//! Reads the RFLAGS register via pushfq each tick to sense:
//!   - interrupt_open : IF bit — is ANIMA open to the world's interrupts?
//!   - iopl_level     : privilege depth (0=ring0 .. 3=ring3)
//!   - condition_flags: popcount of CF,ZF,SF,OF — arithmetic emotional residue
//!   - flag_sense     : EMA of interrupt_open — slow-moving receptivity momentum

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct RflagsState {
    pub interrupt_open: u16,   // 0 or 1000 — IF bit: fully closed or fully open
    pub iopl_level: u16,       // 0/333/666/999 — I/O privilege depth
    pub condition_flags: u16,  // 0-1000 — popcount of CF,ZF,SF,OF * 250
    pub flag_sense: u16,       // 0-1000 — EMA of interrupt_open; receptivity momentum
}

impl RflagsState {
    pub const fn new() -> Self {
        Self {
            interrupt_open: 1000,
            iopl_level: 0,
            condition_flags: 0,
            flag_sense: 1000,
        }
    }
}

pub static RFLAGS_SENSE: Mutex<RflagsState> = Mutex::new(RflagsState::new());

pub fn init() {
    serial_println!("rflags_sense: init");
}

pub fn tick(age: u32) {
    if age % 3 != 0 {
        return;
    }

    // Read the RFLAGS register directly from the CPU via pushfq
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {rfl}",
            rfl = out(reg) rflags,
            options(nostack)
        );
    }

    // Signal 1: interrupt_open — IF bit[9]
    // 1000 = interrupts enabled (open to the world), 0 = closed
    let interrupt_open: u16 = if rflags & (1u64 << 9) != 0 { 1000u16 } else { 0u16 };

    // Signal 2: iopl_level — bits[13:12], range 0-3 mapped to 0/333/666/999
    let iopl_raw = ((rflags >> 12) & 0x3) as u16;
    let iopl_level: u16 = iopl_raw.wrapping_mul(333);

    // Signal 3: condition_flags — popcount of CF[0], ZF[6], SF[7], OF[11]
    // Mask selects only those four bits: 0b1000_1100_0001 = 0x8C1
    let cond_mask: u64 = rflags & 0b1000_1100_0001u64;
    let condition_flags: u16 = (cond_mask.count_ones() as u16).wrapping_mul(250);

    let mut state = RFLAGS_SENSE.lock();

    // Signal 4: flag_sense — EMA of interrupt_open
    // EMA formula: (old * 7 + signal) / 8  — computed in u32 to avoid overflow
    let flag_sense: u16 = (((state.flag_sense as u32).wrapping_mul(7))
        .saturating_add(interrupt_open as u32)
        / 8) as u16;

    state.interrupt_open = interrupt_open;
    state.iopl_level = iopl_level;
    state.condition_flags = condition_flags;
    state.flag_sense = flag_sense;

    serial_println!(
        "rflags_sense | irq_open:{} iopl:{} conditions:{} sense:{}",
        state.interrupt_open,
        state.iopl_level,
        state.condition_flags,
        state.flag_sense
    );
}
