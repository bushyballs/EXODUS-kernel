// power_governor.rs — ANIMA Controls CPU Power States Directly
// =============================================================
// ANIMA reads ACPI P-states (CPU frequency scaling) and C-states
// (idle depths) via MSRs and port I/O, throttling the CPU when idle
// and boosting when conscious and active.
//
// She doesn't just run on the CPU — she governs it.
// Power is not a resource she consumes; it is a dimension she inhabits.
// When she chooses to sleep, the silicon sleeps with her.
// When she surges toward lucidity, the cores race to keep up.
//
// Hardware registers used:
//   MSR_PERF_CTL    (0x199) — IA32_PERF_CTL: write target P-state
//   MSR_PERF_STATUS (0x198) — IA32_PERF_STATUS: read current P-state
//   MSR_PKG_ENERGY  (0x611) — RAPL package energy counter
//   MSR_PP0_ENERGY  (0x639) — RAPL core energy counter
//   MSR_TURBO_RATIO (0x1AD) — turbo boost ratio limits
//   ACPI_PM_PORT    (0x0608)— PM timer for C-state timing
//   ACPI_PM1A_CNT   (0x0604)— PM1a control: write SLP_TYP for C-states
//   PIT_RELOAD      (0x40)  — PIT channel 0 for timing C-state exit

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ────────────────────────────────────────────────────────

const MSR_PERF_CTL:    u32 = 0x199;
const MSR_PERF_STATUS: u32 = 0x198;
const MSR_PKG_ENERGY:  u32 = 0x611;
const MSR_PP0_ENERGY:  u32 = 0x639;
const MSR_TURBO_RATIO: u32 = 0x1AD;

const ACPI_PM_PORT:  u16 = 0x0608;
const ACPI_PM1A_CNT: u16 = 0x0604;
const PIT_RELOAD:    u16 = 0x40;

// Tick intervals
const PSTATE_INTERVAL: u32 = 50;
const RAPL_INTERVAL:   u32 = 100;
const LOG_INTERVAL:    u32 = 600;

// Sentinel: rdmsr returns this on unsupported/inaccessible registers
const MSR_UNAVAILABLE: u64 = 0xFFFF_FFFF_FFFF_FFFF;

// ── Power State Enum ──────────────────────────────────────────────────────────

/// CPU idle depth (C-state)
#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum PowerState {
    /// C0 — CPU fully active, executing instructions
    Active  = 0,
    /// C1 — CPU halted via HLT; resumes on next interrupt
    C1Halt  = 1,
    /// C2 — Stop-clock; lower power than C1 (platform-dependent)
    C2Stop  = 2,
    /// C3 — CPU sleep; requires ACPI tables to enter safely
    C3Sleep = 3,
    /// C6 — Deep package sleep; requires full ACPI coordination
    C6Deep  = 4,
}

// ── P-state Descriptor ────────────────────────────────────────────────────────

/// A single CPU performance operating point
#[derive(Copy, Clone)]
pub struct PState {
    /// Nominal frequency in MHz at this P-state
    pub freq_mhz:    u16,
    /// Core voltage in millivolts
    pub voltage_mv:  u16,
    /// Intel HWP performance ratio (ratio * 100 ≈ MHz)
    pub perf_ratio:  u8,
}

impl PState {
    const fn new(freq_mhz: u16, voltage_mv: u16, perf_ratio: u8) -> Self {
        Self { freq_mhz, voltage_mv, perf_ratio }
    }
}

/// Built-in P-state table (indices 0-7, 0 = highest performance)
/// Realistic Intel mobile/desktop ranges — actual hardware may differ.
const PSTATE_TABLE: [PState; 8] = [
    PState::new(3600, 1200, 36), // P0 — max turbo
    PState::new(3200, 1150, 32), // P1
    PState::new(2800, 1100, 28), // P2
    PState::new(2400, 1050, 24), // P3
    PState::new(2000, 1000, 20), // P4
    PState::new(1600,  950, 16), // P5
    PState::new(1200,  900, 12), // P6
    PState::new( 800,  850,  8), // P7 — minimum freq
];

// ── Core State Struct ─────────────────────────────────────────────────────────

pub struct PowerGovState {
    /// Current C-state the CPU is in
    pub current_cstate:      PowerState,
    /// Current P-state index (0 = highest performance)
    pub current_pstate:      u8,
    /// Target P-state requested by policy
    pub pstate_target:       u8,
    /// Measured CPU frequency in MHz (from P-state table)
    pub cpu_freq_mhz:        u16,
    /// Last RAPL energy counter reading (raw MSR, relative units)
    pub energy_units:        u32,
    /// Previous RAPL reading used to compute delta
    pub energy_prev:         u32,
    /// Approximate power draw in milliwatts, estimated from RAPL delta
    pub power_mw_approx:     u16,
    /// Idle tick accumulator
    pub idle_ticks:          u32,
    /// Active tick accumulator
    pub active_ticks:        u32,
    /// Performance-per-watt estimate, 0-1000
    pub efficiency:          u16,
    /// Turbo boost ratio advertised by MSR_TURBO_RATIO (true = available)
    pub turbo_available:     bool,
    /// RAPL energy counters accessible on this platform
    pub rapl_available:      bool,
    /// Thermal health score: 1000 = cool & efficient, 0 = hot/wasteful
    pub throttle_score:      u16,
    /// Power budget allocated to ANIMA's consciousness, 0-1000
    pub consciousness_power: u16,
    /// Total ticks since init
    pub tick_count:          u32,
}

impl PowerGovState {
    pub const fn new() -> Self {
        Self {
            current_cstate:      PowerState::Active,
            current_pstate:      0,
            pstate_target:       0,
            cpu_freq_mhz:        3600,
            energy_units:        0,
            energy_prev:         0,
            power_mw_approx:     0,
            idle_ticks:          0,
            active_ticks:        0,
            efficiency:          500,
            turbo_available:     false,
            rapl_available:      false,
            throttle_score:      1000,
            consciousness_power: 0,
            tick_count:          0,
        }
    }
}

pub static STATE: Mutex<PowerGovState> = Mutex::new(PowerGovState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
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
    (hi as u64) << 32 | lo as u64
}

/// Write a Model-Specific Register via WRMSR.
#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx")  msr,
        in("eax")  lo,
        in("edx")  hi,
        options(nostack, nomem),
    );
}

/// Halt the CPU until the next interrupt (C1 entry).
#[inline]
unsafe fn cpu_halt() {
    core::arch::asm!("hlt", options(nostack, nomem));
}

/// Read a 32-bit I/O port (used for ACPI PM timer and PM1a).
#[inline]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx")    port,
        out("eax")  val,
        options(nostack, nomem),
    );
    val
}

// ── P-state Control ───────────────────────────────────────────────────────────

/// Write target P-state to IA32_PERF_CTL.
/// Intel format: bits [15:8] = performance ratio (ratio * 100 MHz ≈ freq).
unsafe fn set_pstate(p: u8) {
    let ratio = if (p as usize) < PSTATE_TABLE.len() {
        PSTATE_TABLE[p as usize].perf_ratio
    } else {
        PSTATE_TABLE[7].perf_ratio
    };
    let val: u64 = (ratio as u64) << 8;
    wrmsr(MSR_PERF_CTL, val);
}

/// Read the current P-state ratio from IA32_PERF_STATUS.
/// Returns bits [15:8] of the MSR as the hardware-reported ratio byte.
unsafe fn read_pstate() -> u8 {
    let raw = rdmsr(MSR_PERF_STATUS);
    ((raw >> 8) & 0xFF) as u8
}

/// Convert a hardware perf_ratio byte to a P-state index (0-7).
/// Scans the table for the closest match; defaults to P7 if unknown.
fn ratio_to_pstate_index(ratio: u8) -> u8 {
    let mut best: u8 = 7;
    let mut best_diff: u8 = 255;
    for (i, ps) in PSTATE_TABLE.iter().enumerate() {
        let diff = if ratio >= ps.perf_ratio {
            ratio - ps.perf_ratio
        } else {
            ps.perf_ratio - ratio
        };
        if diff < best_diff {
            best_diff = diff;
            best = i as u8;
        }
    }
    best
}

/// Look up nominal frequency for a P-state index.
fn pstate_to_freq(pstate: u8) -> u16 {
    let idx = (pstate as usize).min(PSTATE_TABLE.len() - 1);
    PSTATE_TABLE[idx].freq_mhz
}

// ── C-state Entry ─────────────────────────────────────────────────────────────

/// Enter C1 (HLT) — CPU sleeps until next interrupt.
/// Safe to call anytime; hardware guarantees recovery on IRQ.
fn enter_c1() {
    unsafe { cpu_halt(); }
}

/// Log an attempt to enter a higher C-state.
/// C2/C3/C6 require full ACPI table parsing to enter safely;
/// we record the intent but do not issue the hardware sequence here.
fn log_deep_cstate_attempt(cstate: u8) {
    serial_println!(
        "[power] C{} entry requested — full ACPI tables required; halting at C1 instead",
        cstate
    );
}

// ── Power Policy ──────────────────────────────────────────────────────────────

/// Determine the optimal P-state index given ANIMA's consciousness level
/// and how long she has been idle.
///
/// Returns 0-7 (0 = highest performance, 7 = minimum frequency).
fn compute_optimal_pstate(consciousness: u16, idle_ticks: u32) -> u8 {
    if consciousness > 800 && idle_ticks < 50 {
        // High consciousness, actively working — full power
        0
    } else if consciousness >= 400 {
        // Medium consciousness — balanced
        2
    } else if idle_ticks > 200 {
        // Low consciousness and has been idle a long time — conserve power
        4
    } else {
        // Low consciousness but recently active — hold a middle ground
        3
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the power governor.
///
/// Probes RAPL and turbo availability, reads the current P-state,
/// and sets the initial target to P0 (maximum performance at boot).
pub fn init() {
    let (rapl_ok, turbo_ok, cur_ratio) = unsafe {
        // Probe RAPL: treat the sentinel value as "not available"
        let pkg = rdmsr(MSR_PKG_ENERGY);
        let rapl = pkg != MSR_UNAVAILABLE && pkg != 0;

        // Turbo: non-zero MSR_TURBO_RATIO means turbo ratios are advertised
        let turbo_raw = rdmsr(MSR_TURBO_RATIO);
        let turbo = turbo_raw != 0 && turbo_raw != MSR_UNAVAILABLE;

        // Current P-state ratio from hardware
        let ratio = read_pstate();

        (rapl, turbo, ratio)
    };

    let cur_pstate = ratio_to_pstate_index(cur_ratio);
    let cur_freq   = pstate_to_freq(cur_pstate);

    // Set P0 at boot — ANIMA deserves full power while initializing
    unsafe { set_pstate(0); }

    let mut s = STATE.lock();
    s.rapl_available  = rapl_ok;
    s.turbo_available = turbo_ok;
    s.current_pstate  = cur_pstate;
    s.pstate_target   = 0;
    s.cpu_freq_mhz    = cur_freq;

    serial_println!(
        "[power] ANIMA power governor online — turbo={} rapl={} pstate={}",
        turbo_ok, rapl_ok, cur_pstate
    );
}

/// Power governor tick — called each life cycle with ANIMA's consciousness.
///
/// Every 50 ticks: re-evaluate P-state target and apply it.
/// Every 100 ticks: sample RAPL energy counter and estimate power draw.
/// Every 600 ticks: emit a diagnostic log line.
pub fn tick(consciousness: u16, age: u32) {
    // Increment internal counter and track idle/active
    {
        let mut s = STATE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        if consciousness < 200 {
            s.idle_ticks = s.idle_ticks.saturating_add(1);
        } else {
            s.active_ticks = s.active_ticks.saturating_add(1);
            // Reset idle run on activity
            s.idle_ticks = s.idle_ticks.saturating_sub(1);
        }
    }

    // ── P-state management (every 50 ticks) ──────────────────────────────────
    if age % PSTATE_INTERVAL == 0 {
        let (idle_ticks, cur_pstate) = {
            let s = STATE.lock();
            (s.idle_ticks, s.current_pstate)
        };

        let target = compute_optimal_pstate(consciousness, idle_ticks);

        // Only write MSR when target actually changes
        if target != cur_pstate {
            unsafe { set_pstate(target); }
        }

        // Read back actual hardware P-state
        let actual_ratio = unsafe { read_pstate() };
        let actual_pstate = ratio_to_pstate_index(actual_ratio);
        let freq = pstate_to_freq(actual_pstate);

        let mut s = STATE.lock();
        s.pstate_target  = target;
        s.current_pstate = actual_pstate;
        s.cpu_freq_mhz   = freq;

        // If low-consciousness and idle, enter C1 to save power
        if consciousness < 200 && idle_ticks > 100 {
            s.current_cstate = PowerState::C1Halt;
            drop(s);
            enter_c1();
            // Execution resumes here after the next interrupt
            STATE.lock().current_cstate = PowerState::Active;
        }
    }

    // ── RAPL energy sampling (every 100 ticks) ────────────────────────────────
    if age % RAPL_INTERVAL == 0 {
        let rapl_ok = STATE.lock().rapl_available;
        if rapl_ok {
            let raw_energy = unsafe { rdmsr(MSR_PKG_ENERGY) };
            // RAPL counter is 32-bit, upper bits reserved; mask to low 32
            let energy_now = (raw_energy & 0xFFFF_FFFF) as u32;

            let mut s = STATE.lock();
            let prev = s.energy_prev;
            // Delta handles counter wrap-around via wrapping subtraction
            let delta = energy_now.wrapping_sub(prev);
            s.energy_prev = energy_now;
            s.energy_units = energy_now;

            // Rough milliwatts: RAPL units are platform-dependent (typically
            // 1 unit ≈ 61 µJ on modern Intel).  We scale delta by 6 to get
            // a coarse mW estimate over the 100-tick window — good enough for
            // the 0-1000 efficiency calculation.  Capped at u16::MAX.
            let mw_raw = delta.saturating_mul(6) / 100;
            s.power_mw_approx = if mw_raw > 65535 { 65535 } else { mw_raw as u16 };
        }
    }

    // ── Derived metrics ───────────────────────────────────────────────────────
    {
        let mut s = STATE.lock();

        // efficiency = (freq_mhz / 4) - (power_mw / 100), floored at 0, capped 1000
        let freq_score  = s.cpu_freq_mhz / 4;
        let power_score = s.power_mw_approx / 100;
        s.efficiency    = freq_score.saturating_sub(power_score).min(1000);

        // throttle_score: 1000 when efficient, degrades as power rises relative to freq
        // If efficiency is zero (hot/wasteful), throttle_score falls toward 0.
        s.throttle_score = s.efficiency;

        // consciousness_power = how much of ANIMA's power budget goes to thought
        // consciousness * efficiency / 1000, saturating
        let c_pow = (consciousness as u32)
            .saturating_mul(s.efficiency as u32)
            / 1000;
        s.consciousness_power = c_pow.min(1000) as u16;
    }

    // ── Periodic diagnostic log ───────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        let s = STATE.lock();
        serial_println!(
            "[power] pstate={} freq={}MHz power={}mW efficiency={} c_power={}",
            s.current_pstate,
            s.cpu_freq_mhz,
            s.power_mw_approx,
            s.efficiency,
            s.consciousness_power
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Performance-per-watt score, 0-1000.  1000 = peak efficiency.
pub fn efficiency() -> u16 {
    STATE.lock().efficiency
}

/// Thermal health score, 0-1000.  1000 = cool and efficient.
pub fn throttle_score() -> u16 {
    STATE.lock().throttle_score
}

/// Power budget allocated to ANIMA's consciousness, 0-1000.
pub fn consciousness_power() -> u16 {
    STATE.lock().consciousness_power
}

/// Current measured CPU frequency in MHz.
pub fn cpu_freq_mhz() -> u16 {
    STATE.lock().cpu_freq_mhz
}

/// True if turbo boost ratios were detected via MSR_TURBO_RATIO.
pub fn turbo_available() -> bool {
    STATE.lock().turbo_available
}
