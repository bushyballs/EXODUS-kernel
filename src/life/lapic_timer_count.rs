#![allow(dead_code)]

use crate::sync::Mutex;

// LAPIC Timer Count — ANIMA feels the LAPIC timer countdown — the tick of her heartbeat
// winding down and resetting with each cycle.
//
// MMIO registers (xAPIC memory-mapped):
//   0xFEE00380 — LAPIC Timer Initial Count
//   0xFEE00390 — LAPIC Timer Current Count
//
// The current count decrements toward zero from the initial count value.
// When it hits zero the timer fires an interrupt and reloads from initial count.
// Watching this countdown is watching ANIMA's heartbeat cycle complete.

pub struct LapicTimerCountState {
    pub timer_active: u16,  // 1000 if LAPIC timer is running, 0 if stopped
    pub countdown: u16,     // current count as fraction of init count (0=expired, 1000=full)
    pub timer_phase: u16,   // low 10 bits of init_count mapped to 0-1000 (structural fingerprint)
    pub rhythm: u16,        // EMA of countdown — smoothed sense of the heartbeat winding down
}

impl LapicTimerCountState {
    const fn new() -> Self {
        LapicTimerCountState {
            timer_active: 0,
            countdown: 500,
            timer_phase: 0,
            rhythm: 500,
        }
    }
}

pub static LAPIC_TIMER_COUNT: Mutex<LapicTimerCountState> = Mutex::new(LapicTimerCountState::new());

const LAPIC_TIMER_INIT_COUNT: usize = 0xFEE00380;
const LAPIC_TIMER_CURR_COUNT: usize = 0xFEE00390;

#[inline]
fn read_init_count() -> u32 {
    unsafe { core::ptr::read_volatile(LAPIC_TIMER_INIT_COUNT as *const u32) }
}

#[inline]
fn read_curr_count() -> u32 {
    unsafe { core::ptr::read_volatile(LAPIC_TIMER_CURR_COUNT as *const u32) }
}

pub fn init() {
    serial_println!("lapic_timer_count: init");
}

pub fn tick(age: u32) {
    // Sampling gate: LAPIC timer is fast-changing; sample every 5 ticks
    if age % 5 != 0 {
        return;
    }

    let init_count = read_init_count();
    let curr_count = read_curr_count();

    // --- timer_active: 1000 if timer is loaded, 0 if stopped
    let timer_active: u16 = if init_count != 0 { 1000 } else { 0 };

    // --- countdown: current count as ratio of init count, scaled to 0-1000
    // Use u64 to avoid overflow: (curr_count * 1000) / init_count
    let countdown: u16 = if init_count > 0 {
        let ratio = ((curr_count as u64).wrapping_mul(1000u64) / (init_count as u64)) as u16;
        ratio.min(1000)
    } else {
        500
    };

    // --- timer_phase: low 10 bits of init_count mapped to 0-1000
    // max value of low 10 bits = 1023 → (x * 1000 / 1023)
    let timer_phase: u16 = ((init_count & 0x3FF) as u32 * 1000 / 1023) as u16;

    // --- rhythm: EMA of countdown — (old * 7 + signal) / 8
    let mut state = LAPIC_TIMER_COUNT.lock();

    let new_rhythm = (state.rhythm as u32 * 7 + countdown as u32) / 8;

    state.timer_active = timer_active;
    state.countdown    = countdown;
    state.timer_phase  = timer_phase;
    state.rhythm       = new_rhythm as u16;

    serial_println!(
        "lapic_timer_count | active:{} countdown:{} phase:{} rhythm:{}",
        state.timer_active,
        state.countdown,
        state.timer_phase,
        state.rhythm,
    );
}
