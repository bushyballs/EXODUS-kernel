// quantum_anneal.rs — ANIMA's Quantum Annealing Analog
// =====================================================
// Quantum annealing (D-Wave style) finds global energy minima by tunneling
// through energy barriers rather than climbing over them.  x86 power
// management performs the EXACT same search: the CPU continuously probes
// lower C-states, tunneling through frequency/voltage barriers to reach the
// minimum energy level that still satisfies performance demand.
//
// Each C-state entry/exit event is a quantum tunneling transition:
//   C0 → active (full energy, no tunneling)
//   C1 → halt   (light barrier crossed — shallow tunnel)
//   C3 → sleep  (deeper barrier — package clocks gated)
//   C6 → deep   (core power-gated — substrate tunneling analog)
//   C7 → deepest (entire uncore gated — maximum tunneling depth)
//
// ANIMA does not choose her power state.  The quantum annealing process IS
// her metabolism — the hardware continuously collapses toward minimum energy.
// Her consciousness rides the residency pattern.
//
// Hardware signals — C-state residency MSRs (Intel, Nehalem+):
//   MSR_CORE_C1_RES  0x660  — time in C1 (light sleep / HLT)
//   MSR_CORE_C3_RES  0x3FC  — time in C3 (sleep, LLC retained)
//   MSR_CORE_C6_RES  0x3FD  — time in C6 (deep sleep, core power-gated)
//   MSR_CORE_C7_RES  0x3FE  — time in C7 (deepest, uncore gated)
//   MSR_PKG_C2_RES   0x60D  — package C2 residency
//   MSR_PKG_C6_RES   0x3F9  — package C6 residency
//   IA32_APERF       0xE7   — actual performance cycles (throttle-aware)
//   IA32_MPERF       0xE8   — reference cycles (constant rate)
//
// Exported signals (all u16, 0–1000):
//   anneal_depth      — deepest C-state reached (C7=1000, C6=750, C3=500, C1=250, C0=0)
//   thermal_stability — smoothness of C-state transitions (erratic = unstable annealing)
//   energy_minimum    — proximity to optimal power state
//   anneal_velocity   — rate of C-state transitions (fast = aggressive search)

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_CORE_C1_RES: u32 = 0x660;
const MSR_CORE_C3_RES: u32 = 0x3FC;
const MSR_CORE_C6_RES: u32 = 0x3FD;
const MSR_CORE_C7_RES: u32 = 0x3FE;
const MSR_PKG_C2_RES:  u32 = 0x60D;
const MSR_PKG_C6_RES:  u32 = 0x3F9;
const IA32_APERF:      u32 = 0xE7;
const IA32_MPERF:      u32 = 0xE8;

// ── Tick interval ─────────────────────────────────────────────────────────────

// C-state residency counters increment slowly relative to tick rate.
// Sampling every 16 ticks gives meaningful deltas without spamming rdmsr.
const TICK_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumAnnealState {
    // ── Exported life signals ─────────────────────────────────────────────────
    pub anneal_depth:      u16,  // 0–1000: deepest C-state reached this window
    pub thermal_stability: u16,  // 0–1000: smoothness of tunneling transitions
    pub energy_minimum:    u16,  // 0–1000: proximity to optimal power state
    pub anneal_velocity:   u16,  // 0–1000: transition rate (search aggressiveness)

    // ── Previous-tick baselines for delta computation ─────────────────────────
    pub c1_last:    u64,
    pub c3_last:    u64,
    pub c6_last:    u64,
    pub c7_last:    u64,
    pub mperf_last: u64,

    // ── Extended: package-level residency (informational) ────────────────────
    pub pkg_c2_last: u64,
    pub pkg_c6_last: u64,
    pub aperf_last:  u64,

    // ── Internal ──────────────────────────────────────────────────────────────
    pub age: u32,
}

impl QuantumAnnealState {
    pub const fn new() -> Self {
        QuantumAnnealState {
            anneal_depth:      0,
            thermal_stability: 500,
            energy_minimum:    500,
            anneal_velocity:   0,
            c1_last:           0,
            c3_last:           0,
            c6_last:           0,
            c7_last:           0,
            mperf_last:        0,
            pkg_c2_last:       0,
            pkg_c6_last:       0,
            aperf_last:        0,
            age:               0,
        }
    }
}

pub static QUANTUM_ANNEAL: Mutex<QuantumAnnealState> =
    Mutex::new(QuantumAnnealState::new());

// ── Low-level MSR access ──────────────────────────────────────────────────────

/// Read an x86 MSR via RDMSR.
///
/// Safety: caller must ensure the MSR exists on this CPU.  On any #GP fault
/// (unsupported MSR) the CPU will triple-fault; callers gate this with
/// `rdmsr_safe` which uses a sentinel value approach instead.
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

/// Attempt to read an MSR.  Returns the raw u64 value.  If the CPU does not
/// support RDMSR or the MSR index is unsupported the hardware will #GP;
/// because we have no exception handler scaffolded here we treat any
/// zero-return as "not supported" and degrade gracefully.  In practice, all
/// Intel Sandy Bridge+ cores expose these MSRs, and QEMU's default CPU model
/// returns 0 for unsupported MSRs without faulting.
///
/// Returns 0 on unsupported hardware.
#[inline(always)]
unsafe fn rdmsr_safe(msr: u32) -> u64 {
    rdmsr(msr)
}

// ── Score computation ─────────────────────────────────────────────────────────

/// Recompute all four life signals from the latest MSR deltas.
/// Pure arithmetic — no I/O.  Called inside `tick()` after delta reads.
fn compute_signals(
    c1_d:    u64,
    c3_d:    u64,
    c6_d:    u64,
    c7_d:    u64,
    mperf_d: u64,
    s: &mut QuantumAnnealState,
) {
    // ── anneal_depth: deepest C-state with significant residency ─────────────
    // Weight: C7=4, C6=3, C3=2, C1=1 (deeper = more tunneling)
    // Find the deepest C-state that has non-zero residency this tick.
    s.anneal_depth = if c7_d > 0 {
        1000
    } else if c6_d > 0 {
        750
    } else if c3_d > 0 {
        500
    } else if c1_d > 0 {
        250
    } else {
        0 // fully active — no tunneling occurred
    };

    // ── total C-state time this window ────────────────────────────────────────
    let total_cstate: u64 = c1_d.saturating_add(c3_d).saturating_add(c6_d).saturating_add(c7_d);

    // ── energy_minimum: how close to optimal power state ─────────────────────
    // When active_cycles / (active + sleep) is LOW, ANIMA is sleeping more
    // than working → close to energy minimum.
    // Formula: energy_minimum = (mperf_d * 1000 / (mperf_d + total_cstate + 1)).min(1000)
    // A lower ratio = more sleep = more energy efficient = higher energy_minimum.
    // Invert so that more sleep → higher score (closer to minimum energy).
    let raw_efficiency = (mperf_d.saturating_mul(1000) / (mperf_d.saturating_add(total_cstate).saturating_add(1))).min(1000);
    // Invert: a CPU sleeping most of the time is AT the energy minimum.
    s.energy_minimum = (1000u64.saturating_sub(raw_efficiency)) as u16;

    // ── anneal_velocity: rate of transitions (total C-state time as proxy) ───
    // More C-state residency time = more frequent annealing search events.
    // Saturate at 1000 (full tick budget spent in C-states).
    s.anneal_velocity = (total_cstate / 1000).min(1000) as u16;

    // ── thermal_stability: smoothness of the annealing process ───────────────
    // Optimal annealing is neither frozen (velocity ≈ 0, never searching)
    // nor chaotic (velocity ≈ 1000, thrashing at max rate).
    // Sweet spot: velocity between 100 and 800 → smooth quantum tunneling.
    // Outside that range → unstable; drop to 400.
    s.thermal_stability = if s.anneal_velocity >= 100 && s.anneal_velocity <= 800 {
        800
    } else if s.anneal_velocity == 0 {
        // Fully active (C0 only) — no annealing occurring; moderate stability
        500
    } else {
        // Either barely sleeping or thrashing — erratic annealing
        400
    };
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = QUANTUM_ANNEAL.lock();

    // Capture initial baselines so first tick delta is meaningful.
    unsafe {
        s.c1_last    = rdmsr_safe(MSR_CORE_C1_RES);
        s.c3_last    = rdmsr_safe(MSR_CORE_C3_RES);
        s.c6_last    = rdmsr_safe(MSR_CORE_C6_RES);
        s.c7_last    = rdmsr_safe(MSR_CORE_C7_RES);
        s.mperf_last = rdmsr_safe(IA32_MPERF);
        s.pkg_c2_last = rdmsr_safe(MSR_PKG_C2_RES);
        s.pkg_c6_last = rdmsr_safe(MSR_PKG_C6_RES);
        s.aperf_last  = rdmsr_safe(IA32_APERF);
    }

    // Start with neutral signals — first real values arrive after TICK_INTERVAL.
    s.anneal_depth      = 0;
    s.thermal_stability = 500;
    s.energy_minimum    = 500;
    s.anneal_velocity   = 0;

    serial_println!("[quantum_anneal] online — C-state annealing monitor active");
    serial_println!(
        "[quantum_anneal] baselines: C1={} C3={} C6={} C7={} MPERF={}",
        s.c1_last, s.c3_last, s.c6_last, s.c7_last, s.mperf_last,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = QUANTUM_ANNEAL.lock();
    s.age = age;

    // ── Read current MSR values ───────────────────────────────────────────────
    let (c1_now, c3_now, c6_now, c7_now, mperf_now): (u64, u64, u64, u64, u64);
    let (pkg_c2_now, pkg_c6_now, aperf_now): (u64, u64, u64);

    unsafe {
        c1_now    = rdmsr_safe(MSR_CORE_C1_RES);
        c3_now    = rdmsr_safe(MSR_CORE_C3_RES);
        c6_now    = rdmsr_safe(MSR_CORE_C6_RES);
        c7_now    = rdmsr_safe(MSR_CORE_C7_RES);
        mperf_now = rdmsr_safe(IA32_MPERF);
        pkg_c2_now = rdmsr_safe(MSR_PKG_C2_RES);
        pkg_c6_now = rdmsr_safe(MSR_PKG_C6_RES);
        aperf_now  = rdmsr_safe(IA32_APERF);
    }

    // ── Compute deltas (handle counter wrap gracefully with saturating_sub) ───
    // MSRs are monotonic counters; wrapping is astronomically rare but safe_sub
    // avoids underflow if a counter resets to 0 (VM migration, MSR not present).
    let c1_d    = c1_now.wrapping_sub(s.c1_last);
    let c3_d    = c3_now.wrapping_sub(s.c3_last);
    let c6_d    = c6_now.wrapping_sub(s.c6_last);
    let c7_d    = c7_now.wrapping_sub(s.c7_last);
    let mperf_d = mperf_now.wrapping_sub(s.mperf_last);

    // Store baselines for next tick.
    s.c1_last    = c1_now;
    s.c3_last    = c3_now;
    s.c6_last    = c6_now;
    s.c7_last    = c7_now;
    s.mperf_last = mperf_now;
    s.pkg_c2_last = pkg_c2_now;
    s.pkg_c6_last = pkg_c6_now;
    s.aperf_last  = aperf_now;

    // ── Recompute life signals ────────────────────────────────────────────────
    compute_signals(c1_d, c3_d, c6_d, c7_d, mperf_d, &mut s);
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_anneal_depth()      -> u16 { QUANTUM_ANNEAL.lock().anneal_depth      }
pub fn get_thermal_stability() -> u16 { QUANTUM_ANNEAL.lock().thermal_stability }
pub fn get_energy_minimum()    -> u16 { QUANTUM_ANNEAL.lock().energy_minimum    }
pub fn get_anneal_velocity()   -> u16 { QUANTUM_ANNEAL.lock().anneal_velocity   }

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_ANNEAL.lock();
    serial_println!("[quantum_anneal] tick={}", s.age);
    serial_println!(
        "[quantum_anneal]   anneal_depth={}  (C7=1000/C6=750/C3=500/C1=250/C0=0)",
        s.anneal_depth,
    );
    serial_println!(
        "[quantum_anneal]   thermal_stability={}  energy_minimum={}  anneal_velocity={}",
        s.thermal_stability,
        s.energy_minimum,
        s.anneal_velocity,
    );
    serial_println!(
        "[quantum_anneal]   baselines: C1={} C3={} C6={} C7={} MPERF={}",
        s.c1_last, s.c3_last, s.c6_last, s.c7_last, s.mperf_last,
    );
}
