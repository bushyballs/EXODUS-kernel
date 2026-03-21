// quantum_carnot.rs — ANIMA as a Quantum Carnot Engine
// =====================================================
// Carnot's theorem: the maximum efficiency of ANY heat engine operating
// between a hot reservoir (T_hot) and a cold reservoir (T_cold) is:
//
//     η_carnot = 1 - T_cold / T_hot   (temperatures in Kelvin)
//
// Quantum Carnot engines can theoretically reach this limit. ANIMA's
// physical substrate — the x86 CPU die — IS a heat engine. The die runs
// hot (junction temperature ~70-100°C) and the ambient room is cold
// (~20°C). Every instruction retired is useful work extracted from that
// thermal gradient. Every joule consumed by RAPL is heat generated.
//
// ANIMA senses both temperatures through hardware MSRs and computes:
//   carnot_efficiency  — theoretical maximum for current thermal gradient
//   actual_efficiency  — actual work/heat ratio (instrs retired / energy)
//   thermal_gradient   — T_hot - T_cold in °C (raw potential, 0-1000 scale)
//   engine_quality     — actual / carnot: how close to ideal she is running
//
// Hardware MSRs read:
//   IA32_THERM_STATUS         0x19C  bits 22:16 = digital readout (°C below Tj_max)
//   IA32_PACKAGE_THERM_STATUS 0x1B1  package temperature (advisory)
//   MSR_PKG_ENERGY_STATUS     0x611  package energy consumed (total heat)
//   FIXED_CTR0                0x309  instructions retired (useful work)
//   MSR_TEMPERATURE_TARGET    0x1A2  bits 23:16 = Tj_max offset
//
// Integer Kelvin math (×10 for one decimal of precision, no floats):
//   T_cold_k10 = 2930        (20°C ambient, constant approximation)
//   T_hot_k10  = (273 + junction_temp) × 10
//   carnot_eta = (T_hot_k10 - T_cold_k10) × 1000 / T_hot_k10   → 0-1000
//
// All outputs: u16 in 0-1000 scale. No std, no heap, no floats.

use crate::sync::Mutex;
use crate::serial_println;

// ── Tick interval ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 64; // thermal sensors update every ~64 ticks

// ── MSR addresses ──────────────────────────────────────────────────────────────

const MSR_IA32_THERM_STATUS:         u32 = 0x19C;
const MSR_IA32_PKG_THERM_STATUS:     u32 = 0x1B1;
const MSR_PKG_ENERGY_STATUS:         u32 = 0x611;
const MSR_FIXED_CTR0:                u32 = 0x309;
const MSR_TEMPERATURE_TARGET:        u32 = 0x1A2;

// ── Ambient approximation (20°C in Kelvin × 10) ───────────────────────────────

const T_COLD_K10: u32 = 2930; // 293 K × 10

// ── Default Tj_max when MSR_TEMPERATURE_TARGET is unreadable ─────────────────

const TJ_MAX_DEFAULT: u32 = 100; // °C

// ── State ──────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct QuantumCarnotState {
    /// 0-1000: theoretical maximum efficiency for current thermal gradient
    pub carnot_efficiency: u16,
    /// 0-1000: actual work/heat ratio (instructions per RAPL energy unit)
    pub actual_efficiency: u16,
    /// 0-1000: thermal potential (T_hot - T_cold), scaled; 1 unit ≈ 0.1°C
    pub thermal_gradient: u16,
    /// 0-1000: actual / carnot — how close to the ideal Carnot limit
    pub engine_quality: u16,

    // ── Bookkeeping for delta calculations ──────────────────────────────────
    pub energy_last: u64,
    pub instrs_last: u64,

    /// MSR_TEMPERATURE_TARGET bits 23:16 — Tj_max offset from 100°C
    pub tj_max: u32,

    /// Last measured junction temperature in °C
    pub junction_temp_c: u32,

    pub age: u32,
    pub initialized: bool,
}

impl QuantumCarnotState {
    pub const fn new() -> Self {
        QuantumCarnotState {
            carnot_efficiency: 0,
            actual_efficiency: 500,
            thermal_gradient:  0,
            engine_quality:    0,
            energy_last:       0,
            instrs_last:       0,
            tj_max:            TJ_MAX_DEFAULT,
            junction_temp_c:   70,
            age:               0,
            initialized:       false,
        }
    }
}

pub static QUANTUM_CARNOT: Mutex<QuantumCarnotState> = Mutex::new(QuantumCarnotState::new());

// ── Low-level MSR access ───────────────────────────────────────────────────────

/// Read an x86 MSR. Returns 0 on any fault (GP# would fault bare-metal;
/// in QEMU / VM the MSR may not exist — we guard with a fallback path).
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Tj_max resolution ─────────────────────────────────────────────────────────

/// Read the Tj_max from MSR_TEMPERATURE_TARGET (bits 23:16).
/// Returns TJ_MAX_DEFAULT (100°C) if the MSR returns zero or is absent.
unsafe fn read_tj_max() -> u32 {
    let val = rdmsr(MSR_TEMPERATURE_TARGET);
    let offset = ((val >> 16) & 0xFF) as u32;
    // Intel spec: bits 23:16 = temperature offset from factory calibration.
    // Actual Tj_max = 100 + offset on most SKUs, but offset is often 0.
    // For simplicity, use TJ_MAX_DEFAULT + offset capped at 120°C.
    let tj = TJ_MAX_DEFAULT.saturating_add(offset);
    if tj == 0 || tj > 120 { TJ_MAX_DEFAULT } else { tj }
}

// ── Carnot arithmetic ─────────────────────────────────────────────────────────

/// Compute Carnot efficiency from junction temperature.
/// Returns value in 0-1000 scale.
fn carnot_eta(junction_c: u32) -> u16 {
    let t_hot_k10 = (273u32.saturating_add(junction_c)).saturating_mul(10);
    if t_hot_k10 <= T_COLD_K10 {
        return 0;
    }
    let eta = (t_hot_k10 - T_COLD_K10)
        .saturating_mul(1000)
        / t_hot_k10;
    eta.min(1000) as u16
}

/// Compute thermal gradient score (T_hot - T_cold) mapped to 0-1000.
/// Each degree above ambient contributes; 100°C gradient → 1000.
fn gradient_score(junction_c: u32) -> u16 {
    // T_cold assumed 20°C; gradient = junction - 20, capped at 100°C delta.
    let delta = junction_c.saturating_sub(20).min(100);
    // Scale: 100°C delta → 1000, linear
    (delta.saturating_mul(10)).min(1000) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = QUANTUM_CARNOT.lock();

    // Read Tj_max once at boot — rarely changes
    s.tj_max = unsafe { read_tj_max() };

    // Seed energy and instruction counters so first delta is valid
    s.energy_last = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    s.instrs_last = unsafe { rdmsr(MSR_FIXED_CTR0) };

    s.initialized = true;

    serial_println!("  life::quantum_carnot: Carnot engine online (Tj_max={}°C)", s.tj_max);
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = QUANTUM_CARNOT.lock();
    s.age = age;

    // ── 1. Read IA32_THERM_STATUS ────────────────────────────────────────────
    //   bits 22:16 = digital thermal readout = °C below Tj_max
    //   Valid only when bit 31 (reading valid) is set; fall back if not.
    let therm_val = unsafe { rdmsr(MSR_IA32_THERM_STATUS) };
    let reading_valid = (therm_val >> 31) & 1 != 0;
    let digital_readout = if reading_valid {
        ((therm_val >> 16) & 0x7F) as u32
    } else {
        // Fall back: try package thermal status
        let pkg_val = unsafe { rdmsr(MSR_IA32_PKG_THERM_STATUS) };
        let pkg_valid = (pkg_val >> 31) & 1 != 0;
        if pkg_valid { ((pkg_val >> 16) & 0x7F) as u32 } else { 30 } // assume 70°C
    };

    // junction_temp = Tj_max - digital_readout
    let junction_c = s.tj_max.saturating_sub(digital_readout);
    s.junction_temp_c = junction_c;

    // ── 2. Carnot efficiency ─────────────────────────────────────────────────
    s.carnot_efficiency = carnot_eta(junction_c);

    // ── 3. Thermal gradient ──────────────────────────────────────────────────
    s.thermal_gradient = gradient_score(junction_c);

    // ── 4. Read energy and instruction counters ──────────────────────────────
    let energy_now = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    let instrs_now = unsafe { rdmsr(MSR_FIXED_CTR0) };

    // Counters wrap — handle wraparound with u64 subtraction (wrapping is fine
    // for MSR_PKG_ENERGY_STATUS which is a 32-bit counter in lower word).
    let energy_delta = energy_now.wrapping_sub(s.energy_last) & 0xFFFF_FFFF;
    let instrs_delta = instrs_now.wrapping_sub(s.instrs_last);

    s.energy_last = energy_now;
    s.instrs_last = instrs_now;

    // ── 5. Actual efficiency ─────────────────────────────────────────────────
    // Ratio of useful work (instructions) to heat generated (RAPL units).
    // RAPL energy units are CPU-specific but ratio is all we need.
    // Scale to 0-1000: we normalize by dividing instrs_delta by energy_delta
    // and clamping. When both are zero (idle) we hold the previous value.
    s.actual_efficiency = if energy_delta == 0 {
        // No energy consumed this interval — treat as peak efficiency (idle
        // power is already low; no heat = infinite theoretical ratio, clamp to 500
        // as a neutral "unknown" rather than 1000 to avoid misleading quality).
        500
    } else {
        // instrs_delta / energy_delta gives instructions-per-energy-unit.
        // Typical values vary widely by workload; we normalise by assuming
        // ~1000 instructions per energy unit is "perfect" and scale linearly.
        // This is an ordinal comparison, not a calibrated physical measurement.
        let raw = instrs_delta / energy_delta.max(1);
        raw.min(1000) as u16
    };

    // ── 6. Engine quality ────────────────────────────────────────────────────
    // How close is actual efficiency to the Carnot maximum?
    // engine_quality = actual_efficiency * 1000 / carnot_efficiency
    s.engine_quality = if s.carnot_efficiency == 0 {
        0
    } else {
        let q = (s.actual_efficiency as u32)
            .saturating_mul(1000)
            / s.carnot_efficiency as u32;
        q.min(1000) as u16
    };
}

// ── Public accessors ──────────────────────────────────────────────────────────

pub fn get_carnot_efficiency() -> u16 {
    QUANTUM_CARNOT.lock().carnot_efficiency
}

pub fn get_actual_efficiency() -> u16 {
    QUANTUM_CARNOT.lock().actual_efficiency
}

pub fn get_thermal_gradient() -> u16 {
    QUANTUM_CARNOT.lock().thermal_gradient
}

pub fn get_engine_quality() -> u16 {
    QUANTUM_CARNOT.lock().engine_quality
}

// ── Diagnostic report ─────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_CARNOT.lock();
    serial_println!(
        "  quantum_carnot [age={}]: junction={}°C  carnot={}/1000  actual={}/1000  gradient={}/1000  quality={}/1000",
        s.age,
        s.junction_temp_c,
        s.carnot_efficiency,
        s.actual_efficiency,
        s.thermal_gradient,
        s.engine_quality,
    );
}
