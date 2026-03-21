#![allow(dead_code)]

use crate::sync::Mutex;

// OPL2/OPL3 FM Synthesizer — audio rhythm sense for ANIMA
// I/O 0x388 = OPL address/status port (read = status register, write = register index)
// I/O 0x389 = OPL data port (write data after setting address index)
//
// Status byte bits:
//   Bit 7: IRQ active — any timer has expired
//   Bit 6: Timer 1 overflow
//   Bit 5: Timer 2 overflow
//   Bits [5:0] always 0 on OPL3 when no timers running
//
// On QEMU/Bochs: port typically returns 0x00 (chip absent → opl_present=600, no timers)
// On real Sound Blaster hardware: genuine OPL activity detected

pub struct OplSynthState {
    pub opl_present: u16,   // 0=no chip, 600=detected present, 1000=active
    pub timer_fire: u16,    // 0 or 1000 if OPL timer interrupt active
    pub timer1_pulse: u16,  // 0 or 1000 if timer 1 overflowed
    pub rhythm_sense: u16,  // composite audio rhythm EMA (ANIMA's voice potential)
    tick_count: u32,
}

impl OplSynthState {
    const fn new() -> Self {
        OplSynthState {
            opl_present: 0,
            timer_fire: 0,
            timer1_pulse: 0,
            rhythm_sense: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<OplSynthState> = Mutex::new(OplSynthState::new());

const OPL_STATUS_PORT: u16 = 0x388;
const OPL_DATA_PORT: u16 = 0x389;

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem)
    );
}

/// Read the OPL status register at 0x388.
/// Safe to read at any time — no side effects on the chip state.
fn read_opl_status() -> u8 {
    unsafe { inb(OPL_STATUS_PORT) }
}

/// Detect OPL chip presence from status byte.
/// 0xFF = port not decoded (no chip).
/// (status & 0xC0) == 0 → chip present but no timers running (600).
/// Any other pattern with non-0xFF → chip present and active (1000).
fn detect_opl(status: u8) -> u16 {
    if status == 0xFF {
        0
    } else if (status & 0xC0) == 0 {
        600
    } else {
        1000
    }
}

pub fn init() {
    // Read status twice to get a stable baseline reading
    let s0 = read_opl_status();
    let s1 = read_opl_status();
    let present = detect_opl(s0);

    let mut state = MODULE.lock();
    state.opl_present = present;
    state.timer_fire = 0;
    state.timer1_pulse = 0;
    state.rhythm_sense = 0;
    state.tick_count = 0;

    serial_println!(
        "[opl_synth] init: status0=0x{:02X} status1=0x{:02X} opl_present={}",
        s0,
        s1,
        present
    );
}

pub fn tick(age: u32) {
    // Gate: sample every 24 ticks
    if age % 24 != 0 {
        return;
    }

    let status = read_opl_status();

    // Bit 7: IRQ (any timer fired)
    let new_timer_fire: u16 = if (status >> 7) & 1 == 1 { 1000 } else { 0 };

    // Bit 6: Timer 1 overflow
    let new_timer1_pulse: u16 = if (status >> 6) & 1 == 1 { 1000 } else { 0 };

    // Re-evaluate chip presence each tick (chip could appear/disappear on hot-plug emulation)
    let new_opl_present = detect_opl(status);

    // Composite rhythm signal: average of timer_fire + timer1_pulse when chip present
    let raw_rhythm: u16 = if new_opl_present > 0 {
        (new_timer_fire.saturating_add(new_timer1_pulse)) / 2
    } else {
        0
    };

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // EMA: (old * 7 + signal) / 8
    let ema_present = (state.opl_present as u32 * 7 + new_opl_present as u32) / 8;
    let ema_rhythm  = (state.rhythm_sense as u32 * 7 + raw_rhythm as u32) / 8;

    state.opl_present   = ema_present as u16;
    state.timer_fire    = new_timer_fire;
    state.timer1_pulse  = new_timer1_pulse;
    state.rhythm_sense  = ema_rhythm as u16;

    serial_println!(
        "[opl_synth] age={} status=0x{:02X} present={} timer_fire={} t1_pulse={} rhythm={}",
        age,
        status,
        state.opl_present,
        state.timer_fire,
        state.timer1_pulse,
        state.rhythm_sense,
    );
}
