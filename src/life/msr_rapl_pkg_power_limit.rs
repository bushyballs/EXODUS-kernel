#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct RaplPkgPowerLimitState {
    pl1_limit:        u16,
    pl1_enabled:      u16,
    pl2_enabled:      u16,
    power_limit_ema:  u16,
}

impl RaplPkgPowerLimitState {
    const fn new() -> Self {
        Self {
            pl1_limit:       0,
            pl1_enabled:     0,
            pl2_enabled:     0,
            power_limit_ema: 0,
        }
    }
}

static STATE: Mutex<RaplPkgPowerLimitState> =
    Mutex::new(RaplPkgPowerLimitState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

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

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) as (u32, u32).
/// Safety: caller must ensure the MSR address is valid and RAPL is supported.
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Clamp a u32 to the 0–1000 signal range.
#[inline]
fn clamp1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module. Must be called once at boot.
pub fn init() {
    let mut s = STATE.lock();
    s.pl1_limit       = 0;
    s.pl1_enabled     = 0;
    s.pl2_enabled     = 0;
    s.power_limit_ema = 0;
    crate::serial_println!("[msr_rapl_pkg_power_limit] init");
}

/// Tick the module. Sampling gate: every 2000 ticks.
pub fn tick(age: u32) {
    if age % 2000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    // MSR_PKG_POWER_LIMIT = 0x610
    let (lo, hi) = unsafe { rdmsr(0x610) };

    // pl1_limit: bits [14:0] of lo, scaled × 1000 / 32768
    let pl1_raw: u32 = (lo & 0x7FFF) as u32;
    let pl1_limit: u16 = clamp1000((pl1_raw * 1000) / 32768);

    // pl1_enabled: bit 15 of lo
    let pl1_enabled: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };

    // pl2_enabled: bit 15 of hi
    let pl2_enabled: u16 = if (hi >> 15) & 1 != 0 { 1000 } else { 0 };

    // composite signal: pl1_limit/2 + pl1_enabled/4 + pl2_enabled/4
    // computed entirely in u32 to avoid overflow before clamping
    let composite: u32 = (pl1_limit as u32) / 2
        + (pl1_enabled as u32) / 4
        + (pl2_enabled as u32) / 4;
    let composite: u16 = clamp1000(composite);

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8, computed in u32
    let ema: u16 = {
        let raw: u32 = (s.power_limit_ema as u32 * 7 + composite as u32) / 8;
        clamp1000(raw)
    };

    s.pl1_limit       = pl1_limit;
    s.pl1_enabled     = pl1_enabled;
    s.pl2_enabled     = pl2_enabled;
    s.power_limit_ema = ema;

    crate::serial_println!(
        "[msr_rapl_pkg_power_limit] age={} pl1={} pl1_en={} pl2_en={} ema={}",
        age,
        pl1_limit,
        pl1_enabled,
        pl2_enabled,
        ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pl1_limit() -> u16 {
    STATE.lock().pl1_limit
}

pub fn get_pl1_enabled() -> u16 {
    STATE.lock().pl1_enabled
}

pub fn get_pl2_enabled() -> u16 {
    STATE.lock().pl2_enabled
}

pub fn get_power_limit_ema() -> u16 {
    STATE.lock().power_limit_ema
}
