#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct RaplPp0State {
    pp0_energy_lo:    u16,
    pp0_delta:        u16,
    pp0_power_sense:  u16,
    pp0_ema:          u16,
    last_lo:          u32,
}

impl RaplPp0State {
    const fn zero() -> Self {
        Self {
            pp0_energy_lo:   0,
            pp0_delta:       0,
            pp0_power_sense: 0,
            pp0_ema:         0,
            last_lo:         0,
        }
    }
}

static STATE: Mutex<RaplPp0State> = Mutex::new(RaplPp0State::zero());

// ── CPUID guard ──────────────────────────────────────────────────────────────

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

/// Read MSR_PP0_ENERGY_STATUS (0x639).
/// Returns the full 64-bit value; energy counter occupies bits 31:0.
unsafe fn read_msr_pp0() -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x639u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let v: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    v as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Initialise the module.
/// If RAPL is supported the current energy counter is seeded into `last_lo`
/// so that the very first delta is meaningful rather than wrapping garbage.
pub fn init() {
    if !has_rapl() {
        crate::serial_println!("[msr_rapl_pp0] RAPL not supported on this CPU — module inactive");
        return;
    }

    let raw = unsafe { read_msr_pp0() };
    let lo  = (raw & 0xFFFF_FFFF) as u32;

    let mut s = STATE.lock();
    s.last_lo = lo;

    crate::serial_println!("[msr_rapl_pp0] init: RAPL supported, seeded last_lo={}", lo);
}

/// Called every kernel tick with the current organism age.
/// Sampling gate: runs every 1000 ticks.
pub fn tick(age: u32) {
    if age % 1000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let raw = unsafe { read_msr_pp0() };
    // Energy counter is bits 31:0.
    let current_lo = (raw & 0xFFFF_FFFF) as u32;

    let mut s = STATE.lock();

    // ── delta (wrapping subtraction handles counter roll-over) ───────────────
    let delta_raw: u32 = current_lo.wrapping_sub(s.last_lo);
    s.last_lo = current_lo;

    // ── pp0_energy_lo: low 16 bits of counter mapped to 0-1000 ──────────────
    // raw_lo is 32-bit; take its low 16 bits for the signal.
    let energy_lo_raw: u32 = current_lo & 0xFFFF;
    // Map [0, 65535] → [0, 1000]
    let energy_lo_sig: u16 = ((energy_lo_raw * 1000) / 65536) as u16;

    // ── pp0_delta: delta mapped to 0-1000 ────────────────────────────────────
    // Clamp delta_raw to 16-bit range before mapping so large jumps saturate.
    let delta_clamped: u32 = if delta_raw > 0xFFFF { 0xFFFF } else { delta_raw };
    let delta_sig: u16 = ((delta_clamped * 1000) / 65536) as u16;

    // ── pp0_power_sense: EMA of delta ────────────────────────────────────────
    let power_sense: u16 = ema8(s.pp0_power_sense, delta_sig);

    // ── pp0_ema: slower (double-smoothed) EMA of power_sense ─────────────────
    let ema_sig: u16 = ema8(s.pp0_ema, power_sense);

    // ── commit ────────────────────────────────────────────────────────────────
    s.pp0_energy_lo   = energy_lo_sig;
    s.pp0_delta       = delta_sig;
    s.pp0_power_sense = power_sense;
    s.pp0_ema         = ema_sig;

    crate::serial_println!(
        "[msr_rapl_pp0] age={} energy={} delta={} power={} ema={}",
        age,
        energy_lo_sig,
        delta_sig,
        power_sense,
        ema_sig,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_pp0_energy_lo() -> u16 {
    STATE.lock().pp0_energy_lo
}

pub fn get_pp0_delta() -> u16 {
    STATE.lock().pp0_delta
}

pub fn get_pp0_power_sense() -> u16 {
    STATE.lock().pp0_power_sense
}

pub fn get_pp0_ema() -> u16 {
    STATE.lock().pp0_ema
}
