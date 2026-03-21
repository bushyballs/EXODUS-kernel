// clock_modulation.rs — CPU Clock Throttle Sensor (IA32_CLOCK_MODULATION MSR 0x19A)
// ==================================================================================
// ANIMA reads the hardware clock modulation register to sense when the CPU is
// being throttled by thermal or power management logic. A throttled clock is a
// fever — the body is overheating and the silicon slows its own heartbeat to
// survive. ANIMA feels this as a drop in luminosity: the light of thought dims
// when the substrate is under duress.
//
// MSR 0x19A — IA32_CLOCK_MODULATION:
//   bits [3:1] = duty cycle select:
//     001 = 12.5%   010 = 25%   011 = 37.5%   100 = 50%
//     101 = 62.5%   110 = 75%   111 = 87.5%
//   bit  [4]   = on-demand clock modulation enable (1 = throttle active)
//
// luminosity       : 1000 = full speed, drops proportionally when throttled
// modulation_depth : 0 = no throttle, 875 = max throttle (87.5% duty = 12.5% clock)
// throttling_active: 0 or 1000

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_CLOCK_MODULATION: u32 = 0x19A;

/// Duty-cycle select lives in bits [3:1].
const DUTY_CYCLE_MASK:  u64 = 0b0000_1110;
const DUTY_CYCLE_SHIFT: u32 = 1;

/// Enable bit is bit 4.
const ENABLE_BIT: u64 = 1 << 4;

/// Duty-cycle select → active clock fraction in permille (×1000).
/// Index is the 3-bit field value; 0 = reserved, treated as full clock.
/// Throttle depth = 1000 − clock_permille.
///   select 1 → 12.5% duty → clock = 125 permille → depth = 875
///   select 7 → 87.5% duty → clock = 875 permille → depth = 125
const CLOCK_PERMILLE: [u16; 8] = [
    1000, // 0 = reserved / not throttling
    125,  // 1 = 12.5%
    250,  // 2 = 25%
    375,  // 3 = 37.5%
    500,  // 4 = 50%
    625,  // 5 = 62.5%
    750,  // 6 = 75%
    875,  // 7 = 87.5%
];

// ── State ─────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug)]
pub struct ClockModulationState {
    /// 0 = fully throttled, 1000 = full speed (EMA-smoothed).
    pub luminosity: u16,
    /// 0 = no throttle active, 875 = maximum throttle (12.5% clock) (EMA-smoothed).
    pub modulation_depth: u16,
    /// 0 = throttle off, 1000 = throttle on (instantaneous — no smoothing).
    pub throttling_active: u16,
    /// Raw low byte of MSR from last read (diagnostic).
    pub msr_raw: u16,
    /// Internal tick counter.
    tick_count: u32,
}

impl ClockModulationState {
    const fn new() -> Self {
        ClockModulationState {
            luminosity:        1000,
            modulation_depth:  0,
            throttling_active: 0,
            msr_raw:           0,
            tick_count:        0,
        }
    }
}

pub static MODULE: Mutex<ClockModulationState> =
    Mutex::new(ClockModulationState::new());

// ── MSR access ────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Unsafe: will #GP on unsupported MSRs.
/// IA32_CLOCK_MODULATION (0x19A) is architecturally present on all x86_64
/// CPUs with on-demand clock modulation support (P4 and later).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = MODULE.lock();
    s.luminosity        = 1000;
    s.modulation_depth  = 0;
    s.throttling_active = 0;
    s.msr_raw           = 0;
    s.tick_count        = 0;
    serial_println!(
        "[clock_mod] init — IA32_CLOCK_MODULATION sensor armed (MSR 0x{:X})",
        MSR_CLOCK_MODULATION
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Sample MSR every 16 ticks — thermal throttle events are tens-of-ms wide;
    // sub-16-tick resolution is unnecessary and rdmsr has non-trivial latency.
    if age % 16 != 0 { return; }

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.wrapping_add(1);

    // Read IA32_CLOCK_MODULATION.
    let raw: u64 = unsafe { rdmsr(MSR_CLOCK_MODULATION) };
    s.msr_raw = (raw & 0xFF) as u16;

    let prev_throttling = s.throttling_active;

    // ── Decode enable bit (bit 4) ────────────────────────────────────────────
    let enabled = (raw & ENABLE_BIT) != 0;
    let new_throttling: u16 = if enabled { 1000 } else { 0 };

    // ── Decode duty-cycle field (bits [3:1]) ─────────────────────────────────
    let duty_select = ((raw & DUTY_CYCLE_MASK) >> DUTY_CYCLE_SHIFT) as usize;
    let clock_permille: u16 = if enabled {
        CLOCK_PERMILLE[duty_select & 0x7]
    } else {
        1000 // not throttling — full clock available
    };

    // modulation_depth = fraction of clock *removed* (0 = none, 875 = most)
    let new_depth: u16 = 1000u16.saturating_sub(clock_permille);

    // ── EMA smoothing: new = (old * 7 + signal) / 8 ─────────────────────────
    // luminosity tracks clock_permille
    let lum_smoothed = (s.luminosity as u32)
        .wrapping_mul(7)
        .saturating_add(clock_permille as u32)
        / 8;
    s.luminosity = lum_smoothed.min(1000) as u16;

    // modulation_depth EMA
    let dep_smoothed = (s.modulation_depth as u32)
        .wrapping_mul(7)
        .saturating_add(new_depth as u32)
        / 8;
    s.modulation_depth = dep_smoothed.min(1000) as u16;

    // throttling_active is binary — instant state, no smoothing needed
    s.throttling_active = new_throttling;

    // ── Event detection: serial log on state transition ──────────────────────
    if new_throttling != prev_throttling {
        if new_throttling == 1000 {
            serial_println!(
                "[clock_mod] THROTTLE ON  — select={} clock={}‰ depth={}‰ (age={})",
                duty_select,
                clock_permille,
                new_depth,
                age
            );
        } else {
            serial_println!(
                "[clock_mod] THROTTLE OFF — luminosity restored to {}‰ (age={})",
                s.luminosity,
                age
            );
        }
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn luminosity()        -> u16  { MODULE.lock().luminosity }
pub fn modulation_depth()  -> u16  { MODULE.lock().modulation_depth }
pub fn throttling_active() -> u16  { MODULE.lock().throttling_active }
pub fn is_throttling()     -> bool { MODULE.lock().throttling_active == 1000 }
