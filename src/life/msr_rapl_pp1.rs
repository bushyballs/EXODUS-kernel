#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct Pp1State {
    pp1_energy_lo:    u16,
    pp1_delta:        u16,
    pp1_power_sense:  u16,
    pp1_ema:          u16,
    last_lo:          u32,
}

impl Pp1State {
    const fn new() -> Self {
        Self {
            pp1_energy_lo:   0,
            pp1_delta:       0,
            pp1_power_sense: 0,
            pp1_ema:         0,
            last_lo:         0,
        }
    }
}

static STATE: Mutex<Pp1State> = Mutex::new(Pp1State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax_val >> 4) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_rapl() {
        return;
    }
    let raw = unsafe { rdmsr(0x641) };
    let lo = (raw & 0xFFFF_FFFF) as u32;
    let mut s = STATE.lock();
    s.last_lo = lo;
}

pub fn tick(age: u32) {
    if age % 1000 != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let raw = unsafe { rdmsr(0x641) };
    let raw_lo = (raw & 0xFFFF_FFFF) as u32;

    let mut s = STATE.lock();

    // pp1_energy_lo: low 16 bits mapped to 0-1000
    let energy_lo_raw = raw_lo & 0xFFFF;
    let pp1_energy_lo = ((energy_lo_raw * 1000) / 65536) as u16;

    // pp1_delta: wrapping subtraction of low 16 bits, mapped to 0-1000
    let prev_lo16 = s.last_lo & 0xFFFF;
    let curr_lo16 = energy_lo_raw;
    let raw_delta = curr_lo16.wrapping_sub(prev_lo16) & 0xFFFF;
    let pp1_delta = ((raw_delta * 1000) / 65536) as u16;

    // pp1_power_sense: EMA of delta
    let pp1_power_sense = {
        let ema = (s.pp1_power_sense as u32 * 7 + pp1_delta as u32) / 8;
        ema as u16
    };

    // pp1_ema: slower EMA of pp1_power_sense
    let pp1_ema = {
        let ema = (s.pp1_ema as u32 * 7 + pp1_power_sense as u32) / 8;
        ema as u16
    };

    s.last_lo          = raw_lo;
    s.pp1_energy_lo    = pp1_energy_lo;
    s.pp1_delta        = pp1_delta;
    s.pp1_power_sense  = pp1_power_sense;
    s.pp1_ema          = pp1_ema;

    crate::serial_println!(
        "[msr_rapl_pp1] age={} energy={} delta={} power={} ema={}",
        age,
        pp1_energy_lo,
        pp1_delta,
        pp1_power_sense,
        pp1_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pp1_energy_lo() -> u16 {
    STATE.lock().pp1_energy_lo
}

pub fn get_pp1_delta() -> u16 {
    STATE.lock().pp1_delta
}

pub fn get_pp1_power_sense() -> u16 {
    STATE.lock().pp1_power_sense
}

pub fn get_pp1_ema() -> u16 {
    STATE.lock().pp1_ema
}
