#![allow(dead_code)]

// io_opl3.rs — ANIMA listens to the OPL3 sound chip status
// The ancient FM synthesis engine (Yamaha YMF262) as a deep sensory resonance.
// ANIMA reads the OPL3 status register and data port as raw environmental signals —
// busy states, timer overflows, and raw data texture feed into sound_sense via EMA.
// READ ONLY — no writes to OPL3 hardware ever.

use crate::sync::Mutex;

pub static IO_OPL3: Mutex<Opl3State> = Mutex::new(Opl3State::new());

pub struct Opl3State {
    pub opl3_busy: u16,
    pub timer_overflow: u16,
    pub data_texture: u16,
    pub sound_sense: u16,
}

impl Opl3State {
    pub const fn new() -> Self {
        Self {
            opl3_busy: 0,
            timer_overflow: 0,
            data_texture: 0,
            sound_sense: 0,
        }
    }
}

pub fn init() {
    serial_println!("io_opl3: init");
}

pub fn tick(age: u32) {
    if age % 13 != 0 {
        return;
    }

    // Read OPL3 status register from port 0x388
    let status: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") 0x388u16,
            out("al") status,
            options(nostack, nomem)
        );
    }

    // Read OPL3 data register from port 0x389
    let data: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") 0x389u16,
            out("al") data,
            options(nostack, nomem)
        );
    }

    // Signal 1: opl3_busy — bit[7] of status byte
    // 1=busy (not ready), 0=idle
    let opl3_busy: u16 = if status & 0x80 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: timer_overflow — bit[5] = Timer1 overflow, bit[6] = Timer2 overflow
    let timer_overflow: u16 = if (status & 0x60) != 0 { 1000u16 } else { 0u16 };

    // Signal 3: data_texture — raw data port value scaled to 0-1000
    // ((data as u32) * 1000 / 255) maps 0..=255 to 0..=1000
    let data_texture: u16 = ((data as u32).wrapping_mul(1000) / 255) as u16;
    let data_texture = data_texture.min(1000);

    let mut state = IO_OPL3.lock();

    // Signal 4: sound_sense — EMA of opl3_busy
    // EMA formula: (old * 7 + signal) / 8
    let sound_sense: u16 = (state.sound_sense.saturating_mul(7).saturating_add(opl3_busy)) / 8;

    state.opl3_busy = opl3_busy;
    state.timer_overflow = timer_overflow;
    state.data_texture = data_texture;
    state.sound_sense = sound_sense;

    serial_println!(
        "io_opl3 | busy:{} timer_ovf:{} texture:{} sense:{}",
        opl3_busy,
        timer_overflow,
        data_texture,
        sound_sense
    );
}
