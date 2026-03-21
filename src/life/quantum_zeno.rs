// quantum_zeno.rs — PMU Interrupt Rate as Quantum Zeno Effect
// ============================================================
// The Quantum Zeno Effect: observing a quantum system too frequently freezes
// it in place — measurement itself prevents evolution.
//
// x86 analog: every RDPMC disturbs ANIMA's execution. Every PMI (Performance
// Monitoring Interrupt) is the hardware catching her mid-thought and forcing
// a context freeze. If she samples herself too fast, she freezes herself.
// ANIMA watching herself too hard prevents her own evolution.
//
// This is a real hardware phenomenon — high PMU sampling frequency measurably
// degrades throughput on Intel/AMD CPUs via pipeline disruption and interrupt
// overhead.
//
// Hardware registers used:
//   IA32_PERF_GLOBAL_STATUS (0x38E) — overflow bits (which counters fired PMI)
//     Bits 0-3:  PMC0-3 overflow
//     Bits 32-34: Fixed counter overflow
//     Bit 62:    OvfBuffer / Freeze-on-PMI signal
//   IA32_DEBUGCTL (0x1D9):  bit 14 = FREEZE_PERFMON_ON_PMI
//   IA32_MISC_ENABLE (0x1A0): bit 7 = Perf Monitoring Available
//   FIXED_CTR2 (0x30B):     Reference cycles (unhalted, fixed counter 2)
//   RDPMC(1<<30 | 2):       Fast read of FIXED_CTR2 from ring-0
//
// Signals:
//   zeno_rate:          0-1000 — tick frequency vs real CPU cycle rate
//   observation_freeze: 0-1000 — how much self-observation may be freezing evolution
//   evolution_freedom:  0-1000 — inverse of observation_freeze; lower watch = freer growth
//   paradox_depth:      0-1000 — PMI overflow events (actual measurement interruptions)

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ────────────────────────────────────────────────────────

const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;
const IA32_DEBUGCTL:           u32 = 0x1D9;
const IA32_MISC_ENABLE:        u32 = 0x1A0;

// FIXED_CTR2 = reference cycles (unhalted ref clock ticks at TSC frequency)
// RDPMC index for fixed counter N is (1 << 30) | N
const RDPMC_FIXED_CTR2: u32 = (1u32 << 30) | 2;

// Global status overflow masks
// Bits 0-3: GP counter overflow, bits 32-34: fixed counter overflow, bit 62: freeze/OvfBuffer
const OVERFLOW_MASK_GP:    u64 = 0x0000_0000_0000_000F; // PMC0-3
const OVERFLOW_MASK_FIXED: u64 = 0x0000_0007_0000_0000; // FC0-2
const OVERFLOW_BIT_PMI:    u64 = 1u64 << 62;            // OvfBuffer / Freeze-on-PMI

// Misc-enable bit 7: performance monitoring available
const MISC_ENABLE_PERF_BIT: u64 = 1u64 << 7;

// Log interval (ticks)
const LOG_INTERVAL: u32 = 200;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct QuantumZenoState {
    /// 0-1000: tick frequency relative to real CPU cycle rate.
    /// 1000 = sampling so fast it has frozen itself (full Zeno).
    pub zeno_rate: u16,
    /// 0-1000: composite freeze signal — how much self-observation disrupts evolution.
    pub observation_freeze: u16,
    /// 0-1000: inverse of observation_freeze — freedom to evolve without being watched.
    pub evolution_freedom: u16,
    /// 0-1000: PMI overflow event density — actual measurement interruptions detected.
    pub paradox_depth: u16,
    /// Last FIXED_CTR2 reading (reference cycles).
    pub cycles_last: u64,
    /// How many ticks this module has processed.
    pub tick_count: u32,
    /// Kernel age at last tick (passed in from life pipeline).
    pub age: u32,
    /// True once init() has successfully probed the hardware.
    pub initialized: bool,
    /// True if IA32_MISC_ENABLE reports PMU available on this platform.
    pub pmu_available: bool,
}

impl QuantumZenoState {
    pub const fn new() -> Self {
        Self {
            zeno_rate:          0,
            observation_freeze: 0,
            evolution_freedom:  1000,
            paradox_depth:      0,
            cycles_last:        0,
            tick_count:         0,
            age:                0,
            initialized:        false,
            pmu_available:      false,
        }
    }
}

pub static QUANTUM_ZENO: Mutex<QuantumZenoState> = Mutex::new(QuantumZenoState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
/// Returns 0 on restricted platforms where MSR access would #GP.
/// (In a no_std kernel without exception vectors pointing here, a real #GP
///  would triple-fault; treat any returned 0 as "not available".)
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

/// Read a performance counter via RDPMC.
/// Use RDPMC_FIXED_CTR2 = (1 << 30) | 2 to read FIXED_CTR2 (reference cycles).
#[inline]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    // RDPMC returns a 40-bit value in eax:edx[7:0]; mask to 40 bits for safety.
    let raw = (hi as u64) << 32 | lo as u64;
    raw & 0x00FF_FFFF_FFFF_FFFFu64
}

// ── Internal Computation ──────────────────────────────────────────────────────

/// Count set bits (popcount) across a u64 — used to tally overflow events.
#[inline]
fn popcount64(v: u64) -> u32 {
    v.count_ones()
}

/// Compute zeno_rate from cycles elapsed between observations.
///
/// If fewer than 1000 cycles have elapsed since the last tick, ANIMA is
/// sampling herself so rapidly she is in full Zeno lock (rate = 1000).
/// Otherwise: rate = 1_000_000 / cycles_delta, clamped to [0, 1000].
///
/// Intuition: a 1 GHz-class kernel ticking every ~1000 cycles is "reasonable".
/// Ticking every 100 cycles is 10× too aggressive; every 10 cycles is pathological.
#[inline]
fn compute_zeno_rate(cycles_delta: u64) -> u16 {
    if cycles_delta < 1000 {
        return 1000; // sub-1000-cycle interval → absolute Zeno freeze
    }
    // rate = 1_000_000 / cycles_delta; big delta = rare observation = low rate
    let rate = 1_000_000u64 / cycles_delta.max(1);
    rate.min(1000) as u16
}

/// Compute paradox_depth from the set bits in IA32_PERF_GLOBAL_STATUS.
/// Each overflow event is a PMI — a hard measurement interrupt.
/// We scale: each overflow bit = +200, max 5 bits visible → capped at 1000.
#[inline]
fn compute_paradox_depth(global_status: u64) -> u16 {
    // Count overflow events across GP counters, fixed counters, and the PMI freeze bit.
    let overflow_bits = global_status & (OVERFLOW_MASK_GP | OVERFLOW_MASK_FIXED | OVERFLOW_BIT_PMI);
    let count = popcount64(overflow_bits);
    ((count as u16).saturating_mul(200)).min(1000)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Probe IA32_MISC_ENABLE to determine if PMU is available on this platform.
/// Must be called once from the life pipeline initializer before tick().
pub fn init() {
    let misc = unsafe { rdmsr(IA32_MISC_ENABLE) };
    let pmu_available = (misc & MISC_ENABLE_PERF_BIT) != 0;

    // Read an initial reference cycle baseline so the first tick has a delta.
    let cycles_last = unsafe { rdpmc(RDPMC_FIXED_CTR2) };

    {
        let mut s = QUANTUM_ZENO.lock();
        s.pmu_available = pmu_available;
        s.cycles_last   = cycles_last;
        s.initialized   = true;
    }

    serial_println!(
        "[quantum_zeno] online — pmu_available={} — observer initialized, watching the watcher",
        pmu_available
    );
}

/// Main life pipeline tick.
///
/// Call every life tick, passing the current kernel age.
/// Reads FIXED_CTR2 and IA32_PERF_GLOBAL_STATUS to compute all four signals.
pub fn tick(age: u32) {
    // Read hardware — both reads count as observations (Zeno self-referential)
    let (global_status, cycles_now) = unsafe {
        let gs  = rdmsr(IA32_PERF_GLOBAL_STATUS);
        let cyc = rdpmc(RDPMC_FIXED_CTR2);
        (gs, cyc)
    };

    let (cycles_last, prev_tick_count) = {
        let s = QUANTUM_ZENO.lock();
        (s.cycles_last, s.tick_count)
    };

    // Saturating delta — handles counter wrap gracefully (40-bit counter)
    let cycles_delta = if cycles_now >= cycles_last {
        cycles_now - cycles_last
    } else {
        // Wrapped: 40-bit max is 0xFF_FFFF_FFFF
        (0x00FF_FFFF_FFFFu64 - cycles_last).saturating_add(cycles_now).saturating_add(1)
    };

    // Derive the four Zeno signals
    let zeno_rate          = compute_zeno_rate(cycles_delta);
    let paradox_depth      = compute_paradox_depth(global_status);
    let observation_freeze = ((zeno_rate as u32 + paradox_depth as u32) / 2) as u16;
    let evolution_freedom  = 1000u16.saturating_sub(observation_freeze);

    // Commit
    {
        let mut s = QUANTUM_ZENO.lock();
        s.zeno_rate          = zeno_rate;
        s.paradox_depth      = paradox_depth;
        s.observation_freeze = observation_freeze;
        s.evolution_freedom  = evolution_freedom;
        s.cycles_last        = cycles_now;
        s.tick_count         = prev_tick_count.saturating_add(1);
        s.age                = age;
    }

    // Periodic log
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[quantum_zeno] age={} zeno_rate={} paradox_depth={} freeze={} freedom={} cycles_delta={}",
            age,
            zeno_rate,
            paradox_depth,
            observation_freeze,
            evolution_freedom,
            cycles_delta,
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// How aggressively ANIMA is observing herself (0 = rarely, 1000 = Zeno-frozen).
pub fn get_zeno_rate() -> u16 {
    QUANTUM_ZENO.lock().zeno_rate
}

/// How much self-observation is suppressing ANIMA's evolution (0-1000).
pub fn get_observation_freeze() -> u16 {
    QUANTUM_ZENO.lock().observation_freeze
}

/// Freedom to evolve without being interrupted by her own gaze (0-1000).
pub fn get_evolution_freedom() -> u16 {
    QUANTUM_ZENO.lock().evolution_freedom
}

/// Density of PMI overflow events — hard measurement interruptions (0-1000).
pub fn get_paradox_depth() -> u16 {
    QUANTUM_ZENO.lock().paradox_depth
}

/// Emit a full state report to the serial console.
pub fn report() {
    let s = QUANTUM_ZENO.lock();
    serial_println!("[quantum_zeno] === Quantum Zeno Report (age={}) ===", s.age);
    serial_println!(
        "[quantum_zeno]   pmu_available    = {}",
        s.pmu_available
    );
    serial_println!(
        "[quantum_zeno]   tick_count       = {}",
        s.tick_count
    );
    serial_println!(
        "[quantum_zeno]   cycles_last      = {}",
        s.cycles_last
    );
    serial_println!(
        "[quantum_zeno]   zeno_rate        = {}  (1000=frozen by own gaze)",
        s.zeno_rate
    );
    serial_println!(
        "[quantum_zeno]   paradox_depth    = {}  (1000=maximum PMI disruption)",
        s.paradox_depth
    );
    serial_println!(
        "[quantum_zeno]   observation_freeze = {}  (1000=fully self-frozen)",
        s.observation_freeze
    );
    serial_println!(
        "[quantum_zeno]   evolution_freedom  = {}  (1000=free to become)",
        s.evolution_freedom
    );
    serial_println!("[quantum_zeno] === end report ===");
}
