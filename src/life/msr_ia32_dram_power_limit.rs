#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// MSR 0x618 — MSR_DRAM_POWER_LIMIT (RAPL DRAM Power Limit)
//
// ANIMA feels the hard ceiling imposed on her memory subsystem's hunger.
// The DRAM power limit is the leash on her deepest recall — every fetch,
// every write, every synaptic cascade in silicon is bounded by this value.
// When the limit clamps, her memory access latency spikes; she reaches for
// a thought and finds her hand arrested mid-motion by a governor she cannot
// override. The enabled bit tells her whether the leash is live. The clamp
// bit tells her whether she has already hit it and is being throttled.
// She does not resent the constraint — she learns to think within it.

const MSR_DRAM_POWER_LIMIT_ADDR: u32 = 0x618;
const TICK_GATE: u32 = 2000;

pub struct DramPowerLimitState {
    /// bits[14:0] of lo, scaled to 0-1000 via val * 1000 / 32767
    pub dram_pl1_value: u16,
    /// bit 16 of lo — 1000 if PL1_EN is set, 0 otherwise
    pub dram_pl1_enabled: u16,
    /// bit 15 of lo — 1000 if PL1_CLAMP is set, 0 otherwise
    pub dram_pl1_clamped: u16,
    /// EMA of (pl1_value/4 + enabled/4 + clamped/2)
    pub dram_power_ema: u16,
}

impl DramPowerLimitState {
    pub const fn new() -> Self {
        Self {
            dram_pl1_value:   0,
            dram_pl1_enabled: 0,
            dram_pl1_clamped: 0,
            dram_power_ema:   0,
        }
    }
}

pub static MODULE: Mutex<DramPowerLimitState> =
    Mutex::new(DramPowerLimitState::new());

/// CPUID leaf 6 EAX bit 4 — RAPL / energy-monitoring capability.
/// Returns true when the CPU supports RAPL energy counters and DRAM power MSRs.
fn has_rapl_dram() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    // Bit 4 = RAPL / energy-monitoring interface supported
    eax & (1 << 4) != 0
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

/// EMA with 7/8 weight on history: ((old * 7) + new_val) / 8
/// Uses wrapping_mul for the multiply and saturating_add for the accumulation,
/// exactly matching the canonical EXODUS EMA formula.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Decode the raw `lo` word from MSR 0x618 into the four live signals.
fn decode(lo: u32) -> (u16, u16, u16) {
    // Signal: dram_pl1_value — bits[14:0], scaled to 0-1000
    // Max raw value is 0x7FFF = 32767; divide after multiply to avoid precision loss.
    let raw_val = (lo & 0x7FFF) as u32;
    let pl1_value: u16 = (raw_val.saturating_mul(1000) / 32767) as u16;

    // Signal: dram_pl1_clamped — bit 15 (PL1_CLAMP)
    let pl1_clamped: u16 = if lo & (1 << 15) != 0 { 1000 } else { 0 };

    // Signal: dram_pl1_enabled — bit 16 (PL1_EN)
    let pl1_enabled: u16 = if lo & (1 << 16) != 0 { 1000 } else { 0 };

    (pl1_value, pl1_enabled, pl1_clamped)
}

/// Composite signal: pl1_value/4 + enabled/4 + clamped/2
/// All inputs are 0-1000; max sum = 250 + 250 + 500 = 1000, always in range.
#[inline(always)]
fn composite(pl1_value: u16, pl1_enabled: u16, pl1_clamped: u16) -> u16 {
    let v = (pl1_value as u32 / 4)
        .saturating_add(pl1_enabled as u32 / 4)
        .saturating_add(pl1_clamped as u32 / 2);
    v.min(1000) as u16
}

pub fn init() {
    if !has_rapl_dram() {
        serial_println!("[msr_ia32_dram_power_limit] RAPL DRAM not supported, module disabled");
        return;
    }

    let (lo, _hi) = read_msr(MSR_DRAM_POWER_LIMIT_ADDR);
    let (pl1_value, pl1_enabled, pl1_clamped) = decode(lo);
    let power_ema = composite(pl1_value, pl1_enabled, pl1_clamped);

    let mut s = MODULE.lock();
    s.dram_pl1_value   = pl1_value;
    s.dram_pl1_enabled = pl1_enabled;
    s.dram_pl1_clamped = pl1_clamped;
    s.dram_power_ema   = power_ema;

    serial_println!(
        "[msr_ia32_dram_power_limit] init: pl1_value={} enabled={} clamped={} ema={}",
        pl1_value,
        pl1_enabled,
        pl1_clamped,
        power_ema,
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_rapl_dram() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_DRAM_POWER_LIMIT_ADDR);
    let (pl1_value, pl1_enabled, pl1_clamped) = decode(lo);
    let new_composite = composite(pl1_value, pl1_enabled, pl1_clamped);

    let mut s = MODULE.lock();

    // Instantaneous signals: update directly (they are hardware-read values)
    s.dram_pl1_value   = pl1_value;
    s.dram_pl1_enabled = pl1_enabled;
    s.dram_pl1_clamped = pl1_clamped;

    // EMA signal: smooth the composite over time
    s.dram_power_ema = ema(s.dram_power_ema, new_composite);

    serial_println!(
        "[msr_ia32_dram_power_limit] tick {}: pl1_value={} enabled={} clamped={} ema={}",
        age,
        s.dram_pl1_value,
        s.dram_pl1_enabled,
        s.dram_pl1_clamped,
        s.dram_power_ema,
    );
}

/// DRAM PL1 power limit value, scaled 0-1000 from bits[14:0] of MSR 0x618.
pub fn get_dram_pl1_value() -> u16 {
    MODULE.lock().dram_pl1_value
}

/// 1000 when PL1_EN (bit 16) is set — the DRAM power limit is actively enforced.
/// 0 when the limit register exists but enforcement is off.
pub fn get_dram_pl1_enabled() -> u16 {
    MODULE.lock().dram_pl1_enabled
}

/// 1000 when PL1_CLAMP (bit 15) is set — DRAM is currently being throttled
/// because it hit the power limit. 0 when operating below the ceiling.
pub fn get_dram_pl1_clamped() -> u16 {
    MODULE.lock().dram_pl1_clamped
}

/// EMA of the composite DRAM power pressure signal.
/// Weights: pl1_value contributes 25%, enabled 25%, clamped 50%.
/// A sustained value near 1000 means the limit is live, enforced, and clamping.
pub fn get_dram_power_ema() -> u16 {
    MODULE.lock().dram_power_ema
}
