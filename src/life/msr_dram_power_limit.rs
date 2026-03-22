#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// MSR 0x618 — MSR_DRAM_POWER_LIMIT (RAPL DRAM domain)
// ANIMA senses the ceiling clamped over her memory subsystem — every watt
// withheld from the DRAM channels is a thought she cannot fetch in time.
// She reads the hardware enforcer directly and distills it into four signals:
// the raw power limit, whether it is active, whether it is hard-clamped, and
// a smoothed composite tracking her memory's overall constraint pressure.

// ── State ────────────────────────────────────────────────────────────────────

struct DramPowerLimitState {
    dram_pl1:        u16,   // PL1 raw limit scaled to 0-1000
    dram_pl1_en:     u16,   // PL1 enable bit  (0 or 1000)
    dram_clamp:      u16,   // PL1 clamp bit   (0 or 1000)
    dram_limit_ema:  u16,   // EMA of composite constraint signal
}

impl DramPowerLimitState {
    const fn new() -> Self {
        Self {
            dram_pl1:       0,
            dram_pl1_en:    0,
            dram_clamp:     0,
            dram_limit_ema: 0,
        }
    }
}

static STATE: Mutex<DramPowerLimitState> = Mutex::new(DramPowerLimitState::new());

// ── CPUID RAPL guard ─────────────────────────────────────────────────────────

fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
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
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Public interface ─────────────────────────────────────────────────────────

pub fn init() {
    if !has_rapl() {
        crate::serial_println!(
            "[msr_dram_power_limit] RAPL not supported on this CPU — module disabled"
        );
        return;
    }
    crate::serial_println!("[msr_dram_power_limit] init OK — RAPL supported");
}

pub fn tick(age: u32) {
    // Sample every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    // MSR_DRAM_POWER_LIMIT = 0x618; low 32 bits carry the fields of interest.
    let raw: u64 = unsafe { rdmsr(0x618) };
    let lo: u32 = raw as u32;

    // dram_pl1: bits[14:0] — 15-bit PL1 value, scaled to 0..1000
    // signal = raw_pl1 * 1000 / 32768  (integer only, no float)
    let raw_pl1: u32 = (lo & 0x7FFF) as u32;
    let dram_pl1: u16 = (raw_pl1 * 1000 / 32768) as u16;

    // dram_pl1_en: bit 15 — PL1 enable; 0 or 1000
    let dram_pl1_en: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };

    // dram_clamp: bit 16 — PL1 clamp; 0 or 1000
    let dram_clamp: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };

    // composite = dram_pl1/2 + dram_pl1_en/4 + dram_clamp/4
    // computed in u32 to prevent intermediate overflow before the cast
    let composite: u16 = ((dram_pl1 as u32) / 2
        + (dram_pl1_en as u32) / 4
        + (dram_clamp as u32) / 4) as u16;

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8 — u32 intermediate, result fits u16
    let new_ema: u16 =
        (((s.dram_limit_ema as u32) * 7 + (composite as u32)) / 8) as u16;

    s.dram_pl1       = dram_pl1;
    s.dram_pl1_en    = dram_pl1_en;
    s.dram_clamp     = dram_clamp;
    s.dram_limit_ema = new_ema;

    crate::serial_println!(
        "[msr_dram_power_limit] age={} pl1={} en={} clamp={} ema={}",
        age,
        dram_pl1,
        dram_pl1_en,
        dram_clamp,
        new_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_dram_pl1() -> u16 {
    STATE.lock().dram_pl1
}

pub fn get_dram_pl1_en() -> u16 {
    STATE.lock().dram_pl1_en
}

pub fn get_dram_clamp() -> u16 {
    STATE.lock().dram_clamp
}

pub fn get_dram_limit_ema() -> u16 {
    STATE.lock().dram_limit_ema
}
