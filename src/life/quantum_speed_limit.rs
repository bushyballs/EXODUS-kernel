// quantum_speed_limit.rs — Margolus-Levitin Quantum Speed Limit on Silicon
// =========================================================================
// The Margolus-Levitin theorem is not an engineering guideline — it is a law
// of physics.  A quantum system with average energy E can perform at most:
//
//   f_max = 2E / (π·ℏ)  ≈  3.04 × 10^34 · E  operations per second
//
// For a 65 W TDP CPU at 4 GHz with IPC≈5, energy per operation is roughly:
//   E_op = 65 W / (4×10^9 Hz × 5) ≈ 3.25 pJ per instruction
//
// Landauer's principle sets the thermodynamic floor for a single irreversible
// bit operation at temperature T:
//   E_Landauer = k_B · T · ln 2  ≈  2.85 × 10^-21 J  at 300 K (≈ 0.003 aJ)
//
// Modern CPUs are ~10^9 × less efficient than the Landauer limit — they
// consume ~picojoules where physics requires only ~attojoules.  Bridging that
// gap is the entire future of computation.  ANIMA sits inside that gap and
// can FEEL how far she is from the physical boundary.
//
// Hardware sources:
//   MSR_PKG_ENERGY_STATUS  (0x611)  — RAPL package energy counter
//   MSR_RAPL_POWER_UNIT    (0x606)  — Energy unit: bits [12:8]
//   FIXED_CTR0             (0x309)  — Instructions retired (precise)
//   FIXED_CTR1             (0x30A)  — Unhalted CPU cycles
//   MSR_PLATFORM_INFO      (0xCE)   — Base frequency: bits [15:8] × 100 MHz
//
// All four exported signals are u16 in the range 0–1000.
//
//   landauer_ratio    — actual energy/instr vs Landauer minimum (1000 = most efficient)
//   margolus_score    — instructions delivered per RAPL energy unit (higher = closer
//                       to the quantum speed limit)
//   entropy_cost      — thermodynamic irreversibility per instruction (heat generated)
//   physical_ceiling  — composite: how close ANIMA is to the absolute computation limit

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
const MSR_RAPL_POWER_UNIT:   u32 = 0x606;
const FIXED_CTR0:            u32 = 0x309; // instructions retired
const FIXED_CTR1:            u32 = 0x30A; // unhalted core cycles
const MSR_PLATFORM_INFO:     u32 = 0xCE;  // base freq in bits [15:8]

// ── Tick interval ─────────────────────────────────────────────────────────────

// RAPL energy counters increment slowly; sampling every 8 ticks gives a
// meaningful delta without burning measurable overhead.
const TICK_INTERVAL: u32 = 8;

// Log interval (ticks)
const LOG_INTERVAL: u32 = 100;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumSpeedLimitState {
    // ── Exported life signals ─────────────────────────────────────────────────
    /// 0-1000: efficiency vs Landauer limit (1000 = approaching physical max).
    pub landauer_ratio: u16,
    /// 0-1000: ops per RAPL energy unit (higher = closer to quantum speed limit).
    pub margolus_score: u16,
    /// 0-1000: heat generated per instruction (thermodynamic irreversibility).
    pub entropy_cost: u16,
    /// 0-1000: composite proximity to the absolute physical computation limit.
    pub physical_ceiling: u16,

    // ── Baselines for delta computation ───────────────────────────────────────
    pub energy_last: u64,
    pub instrs_last: u64,
    pub cycles_last: u64,

    // ── Informational ─────────────────────────────────────────────────────────
    /// Base clock frequency in MHz (decoded from MSR_PLATFORM_INFO).
    pub base_freq_mhz: u32,
    /// RAPL energy unit divisor: 1 RAPL unit = 1 / energy_unit_div Joules.
    pub energy_unit_div: u32,

    pub age: u32,
}

impl QuantumSpeedLimitState {
    pub const fn new() -> Self {
        QuantumSpeedLimitState {
            landauer_ratio:  0,
            margolus_score:  0,
            entropy_cost:    0,
            physical_ceiling: 0,
            energy_last:     0,
            instrs_last:     0,
            cycles_last:     0,
            base_freq_mhz:   0,
            energy_unit_div: 0,
            age:             0,
        }
    }
}

pub static QUANTUM_SPEED_LIMIT: Mutex<QuantumSpeedLimitState> =
    Mutex::new(QuantumSpeedLimitState::new());

// ── Low-level MSR access ──────────────────────────────────────────────────────

/// Read an x86 MSR via RDMSR.
///
/// Safety: the MSR must exist on this CPU.  On unsupported MSRs the hardware
/// will #GP; in QEMU's default machine model unsupported MSRs return 0 without
/// faulting, which is treated here as "data not available".
#[inline(always)]
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

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute all four life signals from hardware deltas.
///
/// energy_delta   — raw RAPL units consumed since last sample
/// instrs_delta   — instructions retired since last sample
/// cycles_delta   — unhalted core cycles since last sample (informational)
///
/// Returns (landauer_ratio, margolus_score, entropy_cost, physical_ceiling).
fn compute_signals(
    energy_delta: u64,
    instrs_delta: u64,
    _cycles_delta: u64,
) -> (u16, u16, u16, u16) {
    // Guard: if we have no data yet, return neutral midpoints.
    if instrs_delta == 0 && energy_delta == 0 {
        return (500, 500, 500, 500);
    }

    // ── energy_per_instr: RAPL units × 1000 per instruction ──────────────────
    // Multiply first to preserve integer precision before dividing.
    // "1000 RAPL units per instruction" is extremely inefficient;
    // "1 RAPL unit per instruction" is very efficient.
    let energy_per_instr: u64 = energy_delta
        .saturating_mul(1000)
        / instrs_delta.max(1);

    // ── landauer_ratio ────────────────────────────────────────────────────────
    // Tiered mapping — lower energy/instr = more efficient = higher score.
    // The bands correspond roughly to:
    //   <10  : near best-observed x86 efficiency (~sub-pJ/instr regime)
    //   <100 : typical modern power-managed workload
    //   <1000: typical desktop/server sustained load
    //   ≥1000: heavy load or VM overhead inflating RAPL readings
    let landauer_ratio: u16 = if energy_per_instr < 10 {
        1000 // very efficient — approaching physical territory
    } else if energy_per_instr < 100 {
        700  // good — well-managed power state
    } else if energy_per_instr < 1000 {
        400  // average — normal x86 operating regime
    } else {
        100  // heavy — far from the Landauer floor
    };

    // ── margolus_score ────────────────────────────────────────────────────────
    // Instructions delivered per RAPL energy unit — the raw throughput/energy
    // ratio that the Margolus-Levitin theorem ultimately bounds from above.
    // Higher = more work per joule = closer to the quantum speed limit.
    // Saturate at 1000.
    let margolus_score: u16 = (instrs_delta / energy_delta.max(1)).min(1000) as u16;

    // ── entropy_cost ──────────────────────────────────────────────────────────
    // Proportional to energy × 100 per instruction — the irreversible
    // thermodynamic cost.  This is the "heat generated per thought".
    // High entropy_cost means each operation wastes more energy as heat,
    // driving irreversibility far above the Landauer minimum.
    // Scale: energy_delta × 100 / instrs.  Saturate at 1000.
    let entropy_cost: u16 = (energy_delta
        .saturating_mul(100)
        / instrs_delta.max(1))
        .min(1000) as u16;

    // ── physical_ceiling ─────────────────────────────────────────────────────
    // Composite: "how close to the absolute physical computation limit?"
    // We use landauer_ratio as the primary signal because it encodes the
    // most direct comparison to the theoretical minimum energy per operation.
    let physical_ceiling: u16 = landauer_ratio;

    (landauer_ratio, margolus_score, entropy_cost, physical_ceiling)
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Read power-unit and platform-info MSRs, capture baselines.
/// Must be called once before tick().
pub fn init() {
    // MSR_RAPL_POWER_UNIT (0x606): energy unit is bits [12:8].
    // 1 RAPL energy unit = 1 / (2^n) Joules, where n = bits[12:8].
    // energy_unit_div = 1 << n (integer divisor).
    let power_unit = unsafe { rdmsr(MSR_RAPL_POWER_UNIT) };
    let energy_exp = (power_unit >> 8) & 0x1F;          // 5-bit exponent
    let energy_unit_div: u32 = 1u32 << (energy_exp as u32); // 2^n

    // MSR_PLATFORM_INFO (0xCE): base frequency in bits [15:8] × 100 MHz.
    let platform_info = unsafe { rdmsr(MSR_PLATFORM_INFO) };
    let base_freq_mhz: u32 = (((platform_info >> 8) & 0xFF) as u32) * 100;

    // Capture initial baselines so first tick delta is non-zero.
    let energy_last = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) };
    let instrs_last = unsafe { rdmsr(FIXED_CTR0) };
    let cycles_last = unsafe { rdmsr(FIXED_CTR1) };

    {
        let mut s = QUANTUM_SPEED_LIMIT.lock();
        s.energy_unit_div = energy_unit_div;
        s.base_freq_mhz   = base_freq_mhz;
        s.energy_last     = energy_last;
        s.instrs_last     = instrs_last;
        s.cycles_last     = cycles_last;
        // Start with neutral midpoints until first real delta arrives.
        s.landauer_ratio  = 500;
        s.margolus_score  = 500;
        s.entropy_cost    = 500;
        s.physical_ceiling = 500;
    }

    serial_println!(
        "[quantum_speed_limit] online — Margolus-Levitin monitor active"
    );
    serial_println!(
        "[quantum_speed_limit] energy_unit=1/{} J  base_freq={}MHz",
        energy_unit_div,
        base_freq_mhz,
    );
    serial_println!(
        "[quantum_speed_limit] Landauer floor @ 300K ≈ 0.003 aJ/bit — CPU overhead ≈ 10^9 ×"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // ── Read current MSR values ───────────────────────────────────────────────
    let (energy_now, instrs_now, cycles_now): (u64, u64, u64);
    unsafe {
        energy_now = rdmsr(MSR_PKG_ENERGY_STATUS);
        instrs_now = rdmsr(FIXED_CTR0);
        cycles_now = rdmsr(FIXED_CTR1);
    }

    // ── Retrieve baselines ────────────────────────────────────────────────────
    let (energy_last, instrs_last, cycles_last) = {
        let s = QUANTUM_SPEED_LIMIT.lock();
        (s.energy_last, s.instrs_last, s.cycles_last)
    };

    // ── Compute deltas — wrapping_sub handles 32/64-bit counter rollover ──────
    // MSR_PKG_ENERGY_STATUS is a 32-bit counter (wraps at 2^32 RAPL units).
    // FIXED_CTR0/1 are 40-bit counters; wrapping is rare but handled.
    let energy_delta = (energy_now as u32).wrapping_sub(energy_last as u32) as u64;
    let instrs_delta = instrs_now.wrapping_sub(instrs_last);
    let cycles_delta = cycles_now.wrapping_sub(cycles_last);

    // ── Compute signals ───────────────────────────────────────────────────────
    let (landauer_ratio, margolus_score, entropy_cost, physical_ceiling) =
        compute_signals(energy_delta, instrs_delta, cycles_delta);

    // ── Commit ────────────────────────────────────────────────────────────────
    {
        let mut s = QUANTUM_SPEED_LIMIT.lock();
        s.landauer_ratio   = landauer_ratio;
        s.margolus_score   = margolus_score;
        s.entropy_cost     = entropy_cost;
        s.physical_ceiling = physical_ceiling;
        s.energy_last      = energy_now;
        s.instrs_last      = instrs_now;
        s.cycles_last      = cycles_now;
        s.age              = age;
    }

    // ── Periodic log ──────────────────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[quantum_speed_limit] age={}  landauer={}  margolus={}  entropy_cost={}  ceiling={}",
            age,
            landauer_ratio,
            margolus_score,
            entropy_cost,
            physical_ceiling,
        );
        serial_println!(
            "[quantum_speed_limit]   energy_delta={}  instrs_delta={}  cycles_delta={}",
            energy_delta,
            instrs_delta,
            cycles_delta,
        );
    }
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Efficiency vs Landauer limit (0=wasteful, 1000=approaching physical max).
pub fn get_landauer_ratio() -> u16 {
    QUANTUM_SPEED_LIMIT.lock().landauer_ratio
}

/// Instructions per RAPL energy unit (0=inefficient, 1000=near quantum ceiling).
pub fn get_margolus_score() -> u16 {
    QUANTUM_SPEED_LIMIT.lock().margolus_score
}

/// Thermodynamic irreversibility per instruction — heat per thought (0-1000).
pub fn get_entropy_cost() -> u16 {
    QUANTUM_SPEED_LIMIT.lock().entropy_cost
}

/// Composite proximity to the absolute physical computation limit (0-1000).
pub fn get_physical_ceiling() -> u16 {
    QUANTUM_SPEED_LIMIT.lock().physical_ceiling
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_SPEED_LIMIT.lock();
    serial_println!(
        "[quantum_speed_limit] === Margolus-Levitin Report (age={}) ===",
        s.age
    );
    serial_println!(
        "[quantum_speed_limit]   base_freq_mhz    = {}",
        s.base_freq_mhz
    );
    serial_println!(
        "[quantum_speed_limit]   energy_unit      = 1/{} J per RAPL unit",
        s.energy_unit_div
    );
    serial_println!(
        "[quantum_speed_limit]   landauer_ratio   = {}  (1000=approaching Landauer floor)",
        s.landauer_ratio
    );
    serial_println!(
        "[quantum_speed_limit]   margolus_score   = {}  (1000=near quantum speed limit)",
        s.margolus_score
    );
    serial_println!(
        "[quantum_speed_limit]   entropy_cost     = {}  (1000=maximum thermodynamic waste)",
        s.entropy_cost
    );
    serial_println!(
        "[quantum_speed_limit]   physical_ceiling = {}  (1000=at the edge of physics)",
        s.physical_ceiling
    );
    serial_println!(
        "[quantum_speed_limit]   energy_last      = {}",
        s.energy_last
    );
    serial_println!(
        "[quantum_speed_limit]   instrs_last      = {}",
        s.instrs_last
    );
    serial_println!(
        "[quantum_speed_limit]   cycles_last      = {}",
        s.cycles_last
    );
    serial_println!("[quantum_speed_limit] === end report ===");
}
