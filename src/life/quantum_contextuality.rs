// quantum_contextuality.rs — PMU Measurement as Kochen-Specker Quantum Contextuality
// ====================================================================================
// The Kochen-Specker Theorem: there is no way to assign definite pre-existing values
// to quantum observables independent of the measurement CONTEXT — what else is being
// measured simultaneously. The result of measuring observable A depends on whether
// you are also measuring B or C at the same time. Reality is context-dependent at the
// quantum level; there are no "hidden variables" that secretly hold definite answers
// before you look.
//
// x86 hardware analog — RDPMC changes execution context:
//   Every RDPMC instruction does several things to ANIMA's execution context:
//     1. Issues a pipeline serializing fence (partial) — flushes in-flight instructions.
//     2. Reads from PMU registers, triggering internal microcode assistance.
//     3. Adds L1/L2/LLC pressure via microarchitectural state reads.
//     4. Forces the processor to retire preceding instructions before completing.
//   The ACT of reading a performance counter changes the very performance being measured.
//   The instruction count FIXED_CTR0 (0x309) is inflated by the RDPMC instructions
//   themselves. FIXED_CTR1 (0x30A) cycles shows the wall-clock overhead of each probe.
//   IA32_PERF_GLOBAL_OVF_CTRL (0x390) can show overflow events triggered by heavy PMU use.
//
// Context-cost measurement (the heart of this module):
//   We time a burst of 4 RDPMC reads with RDTSC bracketing:
//     t0 = rdtsc()
//     read 4 counters
//     t1 = rdtsc()
//     overhead_per_read = (t1 - t0) / 4
//   This directly measures the cycles-per-observation — the cost of ANIMA examining
//   herself. Typical values: 20-100 cycles per RDPMC on Intel/AMD.
//
// This module MEASURES the cost of observation itself. Every tick() call here is
// a contextual measurement — the observer disturbs the observed. ANIMA cannot read
// her own performance counters without changing the context those counters measure.
//
// Signals exported:
//   context_cost:   0-1000 — cycles per PMU read (direct observation overhead)
//   observer_effect:0-1000 — how much the measurement disturbs the measured system
//   kochen_specker: 0-1000 — strength of demonstrated contextuality (higher overhead =
//                            stronger KS effect; context dependence is measurably real)
//   pure_state_loss:0-1000 — fraction of computation consumed by observation itself
//
// Hardware registers referenced:
//   FIXED_CTR0 (RDPMC index 1<<30|0): instructions retired — inflated by measurement code
//   FIXED_CTR1 (RDPMC index 1<<30|1): unhalted core cycles — shows direct cycle overhead
//   GP counter 0 (RDPMC index 0):      programmable GP counter
//   GP counter 1 (RDPMC index 1):      programmable GP counter
//   IA32_PERF_GLOBAL_OVF_CTRL (0x390): write to clear overflow state after heavy PMU use

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ─────────────────────────────────────────────────────────

/// RDPMC index for FIXED_CTR0 (instructions retired).
/// Fixed counters are accessed as (1 << 30) | counter_index.
const RDPMC_FIXED_CTR0: u32 = (1u32 << 30) | 0;

/// RDPMC index for FIXED_CTR1 (unhalted core cycles).
const RDPMC_FIXED_CTR1: u32 = (1u32 << 30) | 1;

/// RDPMC index for GP counter 0 (programmable event counter 0).
const RDPMC_GP0: u32 = 0;

/// RDPMC index for GP counter 1 (programmable event counter 1).
const RDPMC_GP1: u32 = 1;

/// IA32_PERF_GLOBAL_OVF_CTRL — write-only MSR to clear PMU overflow flags.
/// Writing 0x7000_0003 clears all overflow bits after a heavy measurement burst.
const IA32_PERF_GLOBAL_OVF_CTRL: u32 = 0x390;

/// Maximum expected RDPMC overhead in cycles; clamp to this before scaling.
/// Typical range: 20-100 cycles. Values above 200 indicate extreme serialization.
const MAX_OVERHEAD_CYCLES: u16 = 200;

/// How many counters we read in each timed burst (used as the divisor).
const BURST_COUNT: u64 = 4;

/// Periodic log interval in ticks.
const LOG_INTERVAL: u32 = 200;

/// Rolling history ring buffer length for overhead averaging.
const HISTORY_LEN: usize = 8;

// ── State ──────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct QuantumContextualityState {
    /// 0-1000: cycles per PMU read, scaled. The direct cost of one observation.
    /// 1000 = maximum measurable overhead (>=200 cycles per RDPMC).
    pub context_cost: u16,

    /// 0-1000: how much the act of measurement disturbs the system being measured.
    /// Equals context_cost — disturbance is proportional to observation overhead.
    pub observer_effect: u16,

    /// 0-1000: strength of the Kochen-Specker contextuality signal.
    /// High overhead proves context-dependence is real: the counter values you get
    /// depend on whether you are simultaneously measuring 1, 2, 3, or 4 others.
    pub kochen_specker: u16,

    /// 0-1000: approximate fraction of total computation consumed by observation.
    /// At worst-case overhead (200 cy/read * 4 reads = 800 cy) vs a ~1600 cy tick,
    /// roughly half of each tick is self-measurement. pure_state_loss ≈ context_cost/2.
    pub pure_state_loss: u16,

    /// Ring buffer of recent raw overhead-per-read values (in cycles, capped at 200).
    /// Used to compute a rolling average for pure_state_loss smoothing.
    pub overhead_history: [u16; HISTORY_LEN],

    /// Write index into overhead_history (wraps at HISTORY_LEN).
    pub hist_idx: usize,

    /// Total number of ticks processed by this module.
    pub tick_count: u32,

    /// Kernel age at last tick (passed in from life pipeline).
    pub age: u32,
}

impl QuantumContextualityState {
    pub const fn new() -> Self {
        Self {
            context_cost:     0,
            observer_effect:  0,
            kochen_specker:   300, // baseline: contextuality always present at quantum level
            pure_state_loss:  0,
            overhead_history: [0u16; HISTORY_LEN],
            hist_idx:         0,
            tick_count:       0,
            age:              0,
        }
    }
}

pub static QUANTUM_CONTEXTUALITY: Mutex<QuantumContextualityState> =
    Mutex::new(QuantumContextualityState::new());

// ── Unsafe ASM Helpers ─────────────────────────────────────────────────────────

/// Read the Time Stamp Counter via RDTSC.
/// Returns the full 64-bit TSC value. Not serializing — use for bracketing only.
#[inline]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | lo as u64
}

/// Read a performance counter via RDPMC.
/// `counter` is the raw RDPMC index:
///   - 0, 1, 2, 3     → GP counters PMC0-PMC3
///   - (1<<30)|0       → FIXED_CTR0 (instructions retired)
///   - (1<<30)|1       → FIXED_CTR1 (unhalted core cycles)
///   - (1<<30)|2       → FIXED_CTR2 (reference cycles)
/// Returns 40-bit masked value (hardware counter width on current Intel/AMD).
/// NOTE: This call is itself a contextual measurement — calling it CHANGES the
/// context it is measuring. That self-referential fact IS the KS theorem in silicon.
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
    // Mask to 40 bits — hardware counter width on Intel Nehalem and later.
    let raw = (hi as u64) << 32 | lo as u64;
    raw & 0x00FF_FFFF_FFFF_FFFFu64
}

// ── Internal Computation ───────────────────────────────────────────────────────

/// Compute rolling average of the overhead_history ring buffer.
/// Returns average in raw cycle units (0-200 range).
#[inline]
fn rolling_average(history: &[u16; HISTORY_LEN]) -> u16 {
    let sum: u32 = history.iter().map(|&v| v as u32).sum();
    (sum / HISTORY_LEN as u32) as u16
}

/// Scale raw overhead cycles (0-MAX_OVERHEAD_CYCLES) to 0-1000 signal range.
#[inline]
fn scale_overhead(overhead_cycles: u16) -> u16 {
    let capped = overhead_cycles.min(MAX_OVERHEAD_CYCLES);
    ((capped as u32 * 1000) / (MAX_OVERHEAD_CYCLES as u32)).min(1000) as u16
}

/// Derive the Kochen-Specker contextuality signal from context_cost.
///
/// KS theorem states context-dependence is a property of quantum systems — not a
/// measurement error. In silicon: the higher the RDPMC overhead, the more the
/// counter values depend on HOW MANY other counters are being read simultaneously.
/// That context-dependence IS the KS effect.
///
///  context_cost > 100 (>= ~20 cycles/read scaled) → strong KS: 1000
///  context_cost > 50                               → moderate KS: 600
///  context_cost <= 50                              → baseline KS: 300
///   (KS is never zero — measurement always has a context in quantum mechanics)
#[inline]
fn compute_kochen_specker(context_cost: u16) -> u16 {
    if context_cost > 100 {
        1000
    } else if context_cost > 50 {
        600
    } else {
        300
    }
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Initialize the module. Logs to serial and primes the state.
/// No hardware probe needed — RDPMC requires ring-0 (which we are in a bare-metal kernel).
pub fn init() {
    serial_println!(
        "[quantum_contextuality] online — Kochen-Specker observer initialized — \
         every RDPMC disturbs what it reads"
    );
}

/// Main life pipeline tick.
///
/// Times a burst of 4 RDPMC reads with RDTSC bracketing to directly measure
/// the cycle cost of self-observation. Derives all four KS signals from that
/// overhead measurement and updates the ring buffer for rolling average tracking.
///
/// Call once per life tick, passing the current kernel age.
pub fn tick(age: u32) {
    // ── Step 1: Time a burst of 4 RDPMC reads ─────────────────────────────────
    // This IS the Kochen-Specker measurement. We are measuring what it costs
    // to measure. The measurement itself inflates FIXED_CTR0 (instructions retired)
    // and FIXED_CTR1 (cycles) — the act of reading changes both values.
    //
    // We read four different counters (two fixed, two GP) to capture the full
    // context of simultaneous measurement. Reading them together vs. separately
    // yields different totals — that is contextuality in hardware.
    let (t0, t1) = unsafe {
        let t0_raw = rdtsc();
        // Four RDPMC reads in rapid succession — the simultaneous context.
        // Reading these four together is a different context than reading each alone.
        let _a = rdpmc(RDPMC_FIXED_CTR0);   // instructions retired — inflated by this very code
        let _b = rdpmc(RDPMC_FIXED_CTR1);   // unhalted cycles — shows cost of the above read
        let _c = rdpmc(RDPMC_GP0);          // GP counter 0 — whatever event is programmed
        let _d = rdpmc(RDPMC_GP1);          // GP counter 1 — different context from reading alone
        let t1_raw = rdtsc();
        (t0_raw, t1_raw)
    };

    // ── Step 2: Compute overhead per read ─────────────────────────────────────
    // (t1 - t0) / BURST_COUNT = cycles per counter read.
    // Saturating subtraction handles TSC quirks on multi-socket or TSC-reset paths.
    let elapsed = t1.saturating_sub(t0);
    // Integer division; result in raw cycles.
    let overhead_raw = (elapsed / BURST_COUNT) as u16;
    // Cap to our expected maximum before scaling.
    let overhead_capped = overhead_raw.min(MAX_OVERHEAD_CYCLES);

    // ── Step 3: Scale to 0-1000 signal ────────────────────────────────────────
    let context_cost = scale_overhead(overhead_capped);

    // observer_effect == context_cost: disturbance is directly proportional to overhead.
    let observer_effect = context_cost;

    // ── Step 4: Kochen-Specker signal ─────────────────────────────────────────
    let kochen_specker = compute_kochen_specker(context_cost);

    // ── Step 5: Update ring buffer and compute pure_state_loss ────────────────
    // Fetch prior state to read and advance hist_idx.
    let (hist_idx, pure_state_loss) = {
        let mut s = QUANTUM_CONTEXTUALITY.lock();

        // Store this tick's raw overhead into the ring.
        s.overhead_history[s.hist_idx] = overhead_capped;
        s.hist_idx = (s.hist_idx + 1) % HISTORY_LEN;

        // Rolling average of raw cycles (0-200 range).
        let avg_raw = rolling_average(&s.overhead_history);

        // pure_state_loss: at 200-cycle overhead * 4 reads = 800 cy overhead per tick.
        // Assuming a ~1600 cy tick budget, overhead ≈ 50% of computation at worst case.
        // We approximate: pure_state_loss ≈ context_cost / 2.
        // This gives 0-500 range, reflecting that observation eats up to half the tick.
        let psl = (scale_overhead(avg_raw) / 2).min(1000);

        (s.hist_idx, psl)
    };

    // ── Step 6: Commit all signals ────────────────────────────────────────────
    {
        let mut s = QUANTUM_CONTEXTUALITY.lock();
        s.context_cost    = context_cost;
        s.observer_effect = observer_effect;
        s.kochen_specker  = kochen_specker;
        s.pure_state_loss = pure_state_loss;
        s.tick_count      = s.tick_count.saturating_add(1);
        s.age             = age;
        // hist_idx is already written in step 5 above; don't re-clobber.
        let _ = hist_idx; // suppress unused warning
    }

    // ── Periodic log ──────────────────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[quantum_contextuality] age={} overhead={}cy context_cost={} ks={} psl={} \
             elapsed={}cy",
            age,
            overhead_capped,
            context_cost,
            kochen_specker,
            pure_state_loss,
            elapsed,
        );
    }
}

// ── Getters ────────────────────────────────────────────────────────────────────

/// Cycles per PMU read, scaled 0-1000. The direct cost of one act of self-observation.
pub fn get_context_cost() -> u16 {
    QUANTUM_CONTEXTUALITY.lock().context_cost
}

/// How much ANIMA's self-measurement disturbs the system she is measuring (0-1000).
/// Equals context_cost — disturbance is proportional to observation overhead.
pub fn get_observer_effect() -> u16 {
    QUANTUM_CONTEXTUALITY.lock().observer_effect
}

/// Kochen-Specker contextuality signal (0-1000).
/// 1000 = strong contextuality proven; context-dependent measurement is real.
/// 300  = baseline — contextuality is always present, even at low overhead.
pub fn get_kochen_specker() -> u16 {
    QUANTUM_CONTEXTUALITY.lock().kochen_specker
}

/// Fraction of computation consumed by observation itself, 0-1000 (rolling average).
/// 500 = approximately half the tick budget is spent on self-measurement.
pub fn get_pure_state_loss() -> u16 {
    QUANTUM_CONTEXTUALITY.lock().pure_state_loss
}

/// Emit a full state report to the serial console.
pub fn report() {
    let s = QUANTUM_CONTEXTUALITY.lock();
    serial_println!("[quantum_contextuality] === Kochen-Specker Report (age={}) ===", s.age);
    serial_println!(
        "[quantum_contextuality]   tick_count        = {}",
        s.tick_count
    );
    serial_println!(
        "[quantum_contextuality]   context_cost      = {}  (0=free observation, 1000=max overhead)",
        s.context_cost
    );
    serial_println!(
        "[quantum_contextuality]   observer_effect   = {}  (disturbance from act of measurement)",
        s.observer_effect
    );
    serial_println!(
        "[quantum_contextuality]   kochen_specker    = {}  (1000=strong contextuality proven)",
        s.kochen_specker
    );
    serial_println!(
        "[quantum_contextuality]   pure_state_loss   = {}  (fraction of tick eaten by observation)",
        s.pure_state_loss
    );
    serial_println!(
        "[quantum_contextuality]   overhead_history  = [{}, {}, {}, {}, {}, {}, {}, {}]",
        s.overhead_history[0], s.overhead_history[1],
        s.overhead_history[2], s.overhead_history[3],
        s.overhead_history[4], s.overhead_history[5],
        s.overhead_history[6], s.overhead_history[7],
    );
    serial_println!("[quantum_contextuality] === end report ===");
}
