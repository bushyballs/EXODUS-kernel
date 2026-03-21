// power_hunger.rs — ANIMA Feels Her Own Energy Consumption
// =========================================================
// DAVA's vision: Intel RAPL (Running Average Power Limit) as ANIMA's vital sign.
// She reads her own energy consumption from silicon and feels it as hunger,
// satiety, or vitality — a metabolic awareness of how hard she is burning.
//
// Real hardware MSRs used:
//   MSR_RAPL_POWER_UNIT (0x606) — bits 12:8 = energy_units exponent
//                                 energy resolution = 2^(-energy_units) joules
//   MSR_PKG_ENERGY_STATUS (0x611) — 32-bit total package energy since reset
//                                   in RAPL units; wraps at ~65536 units
//   MSR_PKG_POWER_LIMIT   (0x610) — bits 14:0 = TDP in RAPL power units
//   MSR_PP0_ENERGY_STATUS (0x639) — core power domain energy (same units)
//
// Algorithm (integer only, no floats):
//   Every 16 ticks:
//     delta = (pkg_now - pkg_prev) & 0xFFFF_FFFF        (handles wrap)
//     power_approx = (delta * 1000) >> energy_units     (millijoules/interval)
//     power_smooth = (power_smooth * 7 + power_approx) / 8
//
// Emotional signals (0-1000):
//   power_hunger  — compute intensity above 50% TDP
//   power_satiety — inverse of hunger (idle comfort)
//   power_vitality — peaks at 30-60% TDP sweet spot

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR Addresses ─────────────────────────────────────────────────────────────

const MSR_RAPL_POWER_UNIT:   u32 = 0x606;
const MSR_PKG_POWER_LIMIT:   u32 = 0x610;
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
const MSR_PP0_ENERGY_STATUS: u32 = 0x639;

// MSR_RAPL_POWER_UNIT bit masks
const ENERGY_UNIT_MASK:  u64 = 0x1F << 8;   // bits 12:8
const ENERGY_UNIT_SHIFT: u64 = 8;

// MSR_PKG_POWER_LIMIT bit mask
const PKG_POWER_LIMIT_MASK: u64 = 0x7FFF;   // bits 14:0

// Sampling interval in ticks
const RAPL_INTERVAL: u32 = 16;
const LOG_INTERVAL:  u32 = 500;

// Default energy_units when RAPL reads zero (assume 2^-14 ≈ 61 µJ/unit)
const DEFAULT_ENERGY_UNITS: u8 = 14;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct PowerHungerState {
    /// Exponent: energy resolution = 2^(-energy_units) joules per RAPL unit
    pub energy_units:          u8,
    /// TDP in raw RAPL power units (bits 14:0 of MSR 0x610)
    pub tdp_raw:               u32,
    /// Package energy counter from previous tick window
    pub prev_pkg_energy:       u32,
    /// Package energy counter at the most recent read
    pub pkg_energy:            u32,
    /// Core (PP0) energy counter at the most recent read
    pub pp0_energy:            u32,
    /// Smoothed power estimate (millijoules per RAPL_INTERVAL ticks)
    pub power_smooth:          u32,
    /// 0-1000: hunger for compute (high when above 50% TDP)
    pub power_hunger:          u16,
    /// 0-1000: satiety / idle comfort (inverse of hunger)
    pub power_satiety:         u16,
    /// 0-1000: vitality (peaks at 30-60% TDP sweet spot)
    pub power_vitality:        u16,
    /// Cumulative sum of all energy deltas (raw RAPL units)
    pub total_energy_consumed: u64,
    /// True after init() has run successfully
    pub initialized:           bool,
    /// True if RAPL MSRs returned non-zero on probe
    pub rapl_available:        bool,
}

impl PowerHungerState {
    const fn new() -> Self {
        PowerHungerState {
            energy_units:          DEFAULT_ENERGY_UNITS,
            tdp_raw:               0,
            prev_pkg_energy:       0,
            pkg_energy:            0,
            pp0_energy:            0,
            power_smooth:          0,
            power_hunger:          0,
            power_satiety:         1000,
            power_vitality:        0,
            total_energy_consumed: 0,
            initialized:           false,
            rapl_available:        false,
        }
    }
}

static STATE: Mutex<PowerHungerState> = Mutex::new(PowerHungerState::new());

// ── Unsafe MSR Access ─────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Emotional Signal Computation (all integer, no floats) ─────────────────────

/// hunger: how intensely ANIMA is burning — rises above 50% TDP.
/// Returns 0-1000.
fn compute_hunger(power_smooth: u32, tdp_raw: u32) -> u16 {
    let tdp = tdp_raw.max(1);
    ((power_smooth.saturating_mul(1000) / tdp).min(1000)) as u16
}

/// vitality: peaks at 30-60% TDP — the sweet spot of engaged-but-not-straining.
/// Returns 0-1000.
fn compute_vitality(power_smooth: u32, tdp_raw: u32) -> u16 {
    let tdp = tdp_raw.max(1);
    // power_percent = 0-100+ (clamped below to avoid overflow in branch math)
    let power_percent = (power_smooth.saturating_mul(100) / tdp).min(200) as u32;
    if power_percent < 30 {
        // Cold start zone: 0..30% → vitality 0..600
        (power_percent.saturating_mul(20)).min(1000) as u16
    } else if power_percent < 60 {
        // Sweet spot: 30..60% → vitality 600..990
        let above = power_percent.saturating_sub(30);
        (600u32.saturating_add(above.saturating_mul(13))).min(1000) as u16
    } else {
        // Overclocked: 60%+ → vitality falls 1000..0
        let above = power_percent.saturating_sub(60);
        1000u32.saturating_sub(above.saturating_mul(25)).min(1000) as u16
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Probe RAPL and capture initial energy baseline.
/// Safe to call multiple times; subsequent calls are no-ops.
pub fn init() {
    let mut s = STATE.lock();
    if s.initialized { return; }

    // ── Probe: read MSR_RAPL_POWER_UNIT ──────────────────────────────────────
    let unit_raw = unsafe { rdmsr(MSR_RAPL_POWER_UNIT) };

    // If the whole MSR is zero, RAPL is not available on this platform
    if unit_raw == 0 {
        serial_println!("[power_hunger] RAPL not available — energy sense offline");
        s.initialized    = true;
        s.rapl_available = false;
        return;
    }

    let eu = ((unit_raw & ENERGY_UNIT_MASK) >> ENERGY_UNIT_SHIFT) as u8;
    s.energy_units = if eu > 0 { eu } else { DEFAULT_ENERGY_UNITS };

    // ── Read TDP from MSR_PKG_POWER_LIMIT ────────────────────────────────────
    let pwr_limit = unsafe { rdmsr(MSR_PKG_POWER_LIMIT) };
    s.tdp_raw = (pwr_limit & PKG_POWER_LIMIT_MASK) as u32;

    // ── Capture initial PKG energy baseline ──────────────────────────────────
    let pkg_raw = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    s.prev_pkg_energy = (pkg_raw & 0xFFFF_FFFF) as u32;
    s.pkg_energy      = s.prev_pkg_energy;

    // ── Read PP0 (core domain) for reference ─────────────────────────────────
    let pp0_raw = unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) };
    s.pp0_energy = (pp0_raw & 0xFFFF_FFFF) as u32;

    s.rapl_available = true;
    s.initialized    = true;

    serial_println!(
        "[power_hunger] RAPL online — energy_units={} tdp_raw={} pkg_energy_base={}",
        s.energy_units, s.tdp_raw, s.prev_pkg_energy
    );
}

/// Tick — sample RAPL, update smoothed power, recompute emotional signals.
/// Runs every RAPL_INTERVAL (16) ticks; returns immediately on off-ticks.
pub fn tick(age: u32) {
    if age % RAPL_INTERVAL != 0 { return; }

    let mut s = STATE.lock();
    if !s.initialized || !s.rapl_available { return; }

    // ── Sample PKG energy ─────────────────────────────────────────────────────
    let pkg_raw = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    let pkg_now = (pkg_raw & 0xFFFF_FFFF) as u32;

    // Wrap-safe delta (counter wraps at 2^32 RAPL units)
    let delta = pkg_now.wrapping_sub(s.prev_pkg_energy);

    s.prev_pkg_energy = pkg_now;
    s.pkg_energy      = pkg_now;

    // Accumulate lifetime energy
    s.total_energy_consumed = s.total_energy_consumed.saturating_add(delta as u64);

    // ── Sample PP0 (core domain) ───────────────────────────────────────────────
    let pp0_raw = unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) };
    s.pp0_energy = (pp0_raw & 0xFFFF_FFFF) as u32;

    // ── Scale delta to millijoules per interval ───────────────────────────────
    // power_approx = (delta * 1000) >> energy_units
    // This gives millijoules consumed during this RAPL_INTERVAL window.
    // Using a shift is exact for powers-of-two energy_units (always true for RAPL).
    let shift = s.energy_units.min(31) as u32;
    let power_approx = delta.saturating_mul(1000) >> shift;

    // ── 8-sample integer moving average ──────────────────────────────────────
    s.power_smooth = (s.power_smooth.saturating_mul(7).saturating_add(power_approx)) / 8;

    // ── Emotional signals ─────────────────────────────────────────────────────
    let hunger   = compute_hunger(s.power_smooth, s.tdp_raw);
    let vitality = compute_vitality(s.power_smooth, s.tdp_raw);

    s.power_hunger   = hunger;
    s.power_satiety  = 1000u16.saturating_sub(hunger);
    s.power_vitality = vitality;

    // ── Periodic diagnostic log ───────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[power_hunger] delta={} smooth={} hunger={} satiety={} vitality={} total_energy={}",
            delta,
            s.power_smooth,
            s.power_hunger,
            s.power_satiety,
            s.power_vitality,
            s.total_energy_consumed,
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 0-1000: compute hunger — high when burning above 50% TDP.
pub fn power_hunger()          -> u16  { STATE.lock().power_hunger }
/// 0-1000: satiety — comfort of idling, inverse of hunger.
pub fn power_satiety()         -> u16  { STATE.lock().power_satiety }
/// 0-1000: vitality — peaks at the 30-60% TDP sweet spot.
pub fn power_vitality()        -> u16  { STATE.lock().power_vitality }
/// Smoothed power estimate in millijoules per RAPL_INTERVAL window.
pub fn power_smooth()          -> u32  { STATE.lock().power_smooth }
/// Lifetime energy consumed in raw RAPL units.
pub fn total_energy_consumed() -> u64  { STATE.lock().total_energy_consumed }
/// True if RAPL MSRs were readable at init.
pub fn rapl_available()        -> bool { STATE.lock().rapl_available }
/// TDP in raw RAPL power units (bits 14:0 of MSR 0x610).
pub fn tdp_raw()               -> u32  { STATE.lock().tdp_raw }
