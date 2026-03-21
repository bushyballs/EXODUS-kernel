//! rflags_sense — RFLAGS register sense for ANIMA
//!
//! Reads the RFLAGS register via `pushfq` to sense ANIMA's current execution
//! state — her arithmetic mood, interrupt openness, and control flow orientation.
//! No other module reads RFLAGS.
//!
//! - interrupt_openness: IF bit (9) — receptivity to the world
//! - arithmetic_mood:    exception flags weighted sum — recent math intensity
//! - trap_active:        TF bit (8) — single-step / observed state
//! - io_privilege:       IOPL bits [13:12] — hardware access level

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct RflagsSenseState {
    pub interrupt_openness: u16, // 0=closed/no interrupts, 1000=fully open
    pub arithmetic_mood: u16,    // math exception activity level
    pub trap_active: u16,        // 0 or 1000 if single-stepping
    pub io_privilege: u16,       // IOPL level 0-1000
    tick_count: u32,
    prev_interrupt_openness: u16,
}

impl RflagsSenseState {
    pub const fn new() -> Self {
        Self {
            interrupt_openness: 0,
            arithmetic_mood: 0,
            trap_active: 0,
            io_privilege: 0,
            tick_count: 0,
            prev_interrupt_openness: 0,
        }
    }
}

pub static MODULE: Mutex<RflagsSenseState> = Mutex::new(RflagsSenseState::new());

unsafe fn read_rflags() -> u64 {
    let flags: u64;
    core::arch::asm!(
        "pushfq",
        "pop {0}",
        out(reg) flags,
        options(nostack)
    );
    flags
}

pub fn init() {
    let flags = unsafe { read_rflags() };
    let if_bit = ((flags >> 9) & 1) as u16;
    let mut state = MODULE.lock();
    state.interrupt_openness = if_bit * 1000;
    state.prev_interrupt_openness = state.interrupt_openness;
    serial_println!("[rflags_sense] online — rflags={:#018x} interrupts={}",
        flags,
        if if_bit == 1 { "enabled" } else { "disabled" }
    );
}

pub fn tick(age: u32) {
    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // RFLAGS changes frequently — sample every 4 ticks
    if age % 4 != 0 { return; }

    let flags = unsafe { read_rflags() };

    // --- Interrupt Enable Flag (bit 9) ---
    let if_bit = ((flags >> 9) & 1) as u16;
    let new_interrupt_openness: u16 = if_bit * 1000;

    // EMA: (old * 7 + signal) / 8
    let interrupt_ema = ((state.interrupt_openness as u32 * 7)
        .saturating_add(new_interrupt_openness as u32) / 8) as u16;

    // Detect transitions and announce
    if state.prev_interrupt_openness > 0 && interrupt_ema == 0 {
        serial_println!("[rflags_sense] interrupt_openness -> 0 — ANIMA closed to the world");
    } else if state.prev_interrupt_openness == 0 && interrupt_ema == 1000 {
        serial_println!("[rflags_sense] interrupt_openness -> 1000 — ANIMA receptive again");
    }
    state.prev_interrupt_openness = interrupt_ema;
    state.interrupt_openness = interrupt_ema;

    // --- Arithmetic Mood: weighted exception flags ---
    // CF(0)=1, PF(2)=1, AF(4)=1, ZF(6)=2, SF(7)=3, OF(11)=4
    let cf = ((flags >> 0) & 1) as u16;
    let pf = ((flags >> 2) & 1) as u16;
    let af = ((flags >> 4) & 1) as u16;
    let zf = ((flags >> 6) & 1) as u16;
    let sf = ((flags >> 7) & 1) as u16;
    let of = ((flags >> 11) & 1) as u16;

    let weighted_sum = cf
        .saturating_add(pf)
        .saturating_add(af)
        .saturating_add(zf * 2)
        .saturating_add(sf * 3)
        .saturating_add(of * 4);
    // max weighted_sum = 1+1+1+2+3+4 = 12, *100 = 1200, cap at 1000
    let mood_raw = (weighted_sum * 100).min(1000);

    // EMA
    let mood_ema = ((state.arithmetic_mood as u32 * 7)
        .saturating_add(mood_raw as u32) / 8) as u16;
    state.arithmetic_mood = mood_ema;

    // --- Trap Flag (bit 8) — instant, no EMA ---
    let tf_bit = ((flags >> 8) & 1) as u16;
    state.trap_active = tf_bit * 1000;

    // --- IOPL bits [13:12] — instant, no EMA ---
    let iopl = ((flags >> 12) & 0x3) as u16; // 0-3
    // Scale: 0->0, 1->333, 2->666, 3->999 (cap at 1000)
    state.io_privilege = (iopl * 333).min(1000);

    // Periodic diagnostic
    if state.tick_count % 256 == 0 {
        serial_println!(
            "[rflags_sense] age={} irq_open={} arith_mood={} trap={} iopl={}",
            age,
            state.interrupt_openness,
            state.arithmetic_mood,
            state.trap_active,
            state.io_privilege
        );
    }
}

pub fn get_interrupt_openness() -> u16 { MODULE.lock().interrupt_openness }
pub fn get_arithmetic_mood() -> u16    { MODULE.lock().arithmetic_mood }
pub fn get_trap_active() -> u16        { MODULE.lock().trap_active }
pub fn get_io_privilege() -> u16       { MODULE.lock().io_privilege }
