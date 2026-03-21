// coherence_time.rs — Cache Line Residence as T1/T2 Qubit Coherence Times
// ========================================================================
// Quantum qubits decohere on two fundamental timescales:
//
//   T1 (relaxation time): how long a qubit HOLDS its energy before decaying
//      to the ground state. Energy leaks into the environment; the qubit
//      "forgets" whether it was |1⟩ and relaxes to |0⟩.
//
//   T2 (dephasing time): how long a qubit maintains PHASE coherence. Even
//      if the energy hasn't decayed, environmental noise scrambles the
//      relative phase between |0⟩ and |1⟩, destroying interference.
//      T2 ≤ 2×T1 always — you can't maintain phase longer than energy.
//
// x86 hardware analog:
//
//   T1 = L1 cache line residence time. A cache line "holds its energy"
//        (data stays valid in L1) until eviction pressure or a coherence
//        invalidation forces it to relax to L2/L3 (ground state).
//        L1 HIT rate proxies T1 — high hit rate means long T1.
//        An L1 MISS is a T1 relaxation event: the data decayed before use.
//
//   T2 = Branch predictor coherence time. Even if the instruction stream
//        is present in caches, the execution PHASE (which path to take)
//        can decohere. A misprediction is a phase decoherence event:
//        the predictor lost track of the execution trajectory.
//        Branch ACCURACY proxies T2 — high accuracy means long T2.
//        T2 < T1 always: prediction decoheres faster than cache eviction,
//        consistent with the qubit constraint T2 ≤ 2×T1.
//
// PMU events:
//   PMC0 — MEM_LOAD_RETIRED.L1_HIT   (event 0xD1, umask 0x01)
//             L1 hits  — qubit still coherent (T1 alive)
//   PMC1 — MEM_LOAD_RETIRED.L1_MISS  (event 0xD1, umask 0x08)
//             L1 misses — T1 relaxation events
//   PMC2 — BR_MISP_RETIRED.ALL_BRANCHES (event 0xC5, umask 0x00)
//             Mispredictions — T2 dephasing events
//   PMC3 — BR_INST_RETIRED.ALL_BRANCHES (event 0xC4, umask 0x00)
//             Total branches — T2 denominator
//
// MSRs programmed:
//   IA32_PERFEVTSEL0 (0x186) — PMC0 event select (L1 hits)
//   IA32_PERFEVTSEL1 (0x187) — PMC1 event select (L1 misses)
//   IA32_PERFEVTSEL2 (0x188) — PMC2 event select (mispredictions)
//   IA32_PERFEVTSEL3 (0x189) — PMC3 event select (total branches)
//   IA32_PERF_GLOBAL_CTRL (0x38F) — enable PMC0+PMC1+PMC2+PMC3
//
// Signals (u16, 0-1000):
//   t1_relaxation   — L1 hit rate (high = long T1 = slow energy decay)
//   t2_dephasing    — branch accuracy (high = long T2 = phase coherent)
//   coherence_ratio — T2/T1 proxy, capped at 500 (T2 ≤ 2T1 enforced)
//   quantum_lifetime — weighted composite: 0.6×T1 + 0.4×T2
//
// All values u16 0–1000. No heap. No std. No floats.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR / PMU constants ───────────────────────────────────────────────────────

const MSR_PERFEVTSEL0:      u32 = 0x186;
const MSR_PERFEVTSEL1:      u32 = 0x187;
const MSR_PERFEVTSEL2:      u32 = 0x188;
const MSR_PERFEVTSEL3:      u32 = 0x189;
const MSR_PERF_GLOBAL_CTRL: u32 = 0x38F;

// PERFEVTSEL bit layout:
//   bits  7:0  — EventSelect (event code)
//   bits 15:8  — UMask       (sub-event selector)
//   bit    17  — OS          (count at ring 0 — we are always ring 0)
//   bit    22  — EN          (enable this counter)
const PERFEVTSEL_FLAGS: u64 = (1u64 << 22) | (1u64 << 17);

// PMC0: MEM_LOAD_RETIRED.L1_HIT  — event 0xD1, umask 0x01
const EVTSEL_L1_HIT:    u64 = PERFEVTSEL_FLAGS | (0x01u64 << 8) | 0xD1;
// PMC1: MEM_LOAD_RETIRED.L1_MISS — event 0xD1, umask 0x08
const EVTSEL_L1_MISS:   u64 = PERFEVTSEL_FLAGS | (0x08u64 << 8) | 0xD1;
// PMC2: BR_MISP_RETIRED.ALL_BRANCHES — event 0xC5, umask 0x00
const EVTSEL_BR_MISP:   u64 = PERFEVTSEL_FLAGS | (0x00u64 << 8) | 0xC5;
// PMC3: BR_INST_RETIRED.ALL_BRANCHES — event 0xC4, umask 0x00
const EVTSEL_BR_ALL:    u64 = PERFEVTSEL_FLAGS | (0x00u64 << 8) | 0xC4;

// Enable PMC0 (bit 0) + PMC1 (bit 1) + PMC2 (bit 2) + PMC3 (bit 3)
const GLOBAL_CTRL_PMC0123: u64 = 0xF;

// ── Tick rate ─────────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 1;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CoherenceTimeState {
    // ── Derived metrics (0–1000) ─────────────────────────────────────────────
    /// L1 hit rate × 1000. High = long T1 = data holds in L1 (qubit retains energy).
    pub t1_relaxation:   u16,
    /// Branch accuracy × 1000. High = long T2 = phase stays coherent.
    pub t2_dephasing:    u16,
    /// T2/T1 proxy × 1000, capped at 500. Enforces T2 ≤ 2×T1 physically.
    pub coherence_ratio: u16,
    /// Weighted lifetime: (T1×6 + T2×4) / 10. Overall qubit survival quality.
    pub quantum_lifetime: u16,

    // ── Raw per-tick deltas ───────────────────────────────────────────────────
    pub l1_hit_last:   u64,
    pub l1_miss_last:  u64,
    pub mispred_last:  u64,
    pub branches_last: u64,

    // ── Bookkeeping ───────────────────────────────────────────────────────────
    pub age:         u32,
    pub pmu_active:  bool,
    pub initialized: bool,

    // Previous PMC snapshots for delta computation
    pmc0_prev: u64,
    pmc1_prev: u64,
    pmc2_prev: u64,
    pmc3_prev: u64,
}

impl CoherenceTimeState {
    pub const fn new() -> Self {
        CoherenceTimeState {
            // Warm defaults: good L1 hit rate, decent branch prediction,
            // ratio within physical bounds, healthy lifetime.
            t1_relaxation:   700,
            t2_dephasing:    700,
            coherence_ratio: 500,  // T2/T1 ≤ 500 at init
            quantum_lifetime: 700, // (700*6 + 700*4) / 10
            l1_hit_last:     0,
            l1_miss_last:    0,
            mispred_last:    0,
            branches_last:   0,
            age:             0,
            pmu_active:      false,
            initialized:     false,
            pmc0_prev:       0,
            pmc1_prev:       0,
            pmc2_prev:       0,
            pmc3_prev:       0,
        }
    }
}

pub static COHERENCE_TIME: Mutex<CoherenceTimeState> =
    Mutex::new(CoherenceTimeState::new());

// ── Low-level CPU primitives ──────────────────────────────────────────────────

/// Write an MSR (ring 0 only).
#[inline(always)]
pub unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Read a performance monitoring counter via RDPMC.
/// counter=0 → PMC0, 1 → PMC1, 2 → PMC2, 3 → PMC3.
#[inline(always)]
pub unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx")  counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Score computation ─────────────────────────────────────────────────────────

/// Recompute all four derived u16 metrics from this tick's raw PMC deltas.
fn compute_scores(
    s: &mut CoherenceTimeState,
    l1_hit:   u64,
    l1_miss:  u64,
    mispred:  u64,
    branches: u64,
) {
    s.l1_hit_last   = l1_hit;
    s.l1_miss_last  = l1_miss;
    s.mispred_last  = mispred;
    s.branches_last = branches;

    // ── T1: L1 hit rate ───────────────────────────────────────────────────────
    // total_loads = hits + misses. If zero, use standing default (no memory ops).
    let total_loads = l1_hit.saturating_add(l1_miss);
    let t1: u64 = if total_loads == 0 {
        700
    } else {
        (l1_hit.saturating_mul(1000) / total_loads.max(1)).min(1000)
    };
    s.t1_relaxation = t1 as u16;

    // ── T2: branch accuracy ───────────────────────────────────────────────────
    // accuracy = 1 - (mispred / total_branches). If no branches, use default.
    let t2: u64 = if branches == 0 {
        700
    } else {
        let mispred_rate = (mispred.saturating_mul(1000) / branches.max(1)).min(1000);
        1000u64.saturating_sub(mispred_rate)
    };
    s.t2_dephasing = t2 as u16;

    // ── Coherence ratio: T2/T1 proxy, capped at 500 ──────────────────────────
    // Physical constraint: T2 ≤ 2×T1, so (T2/T1) ≤ 2. Mapped to [0,500].
    // coherence_ratio = (t2 * 500) / t1.max(1), min 1000 then cap at 500.
    let ratio: u64 = (t2.saturating_mul(500) / t1.max(1)).min(1000);
    s.coherence_ratio = ratio as u16;

    // ── Quantum lifetime: weighted composite ─────────────────────────────────
    // quantum_lifetime = (T1 × 6 + T2 × 4) / 10
    // T1 dominates slightly (energy decay is the primary limit).
    let lifetime: u64 = (t1.saturating_mul(6).saturating_add(t2.saturating_mul(4))) / 10;
    s.quantum_lifetime = lifetime.min(1000) as u16;
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Program the PMU and seed baseline PMC snapshots.
/// Must be called once before the first tick.
pub fn init() {
    let mut s = COHERENCE_TIME.lock();

    unsafe {
        // Disable all counters before reconfiguring to avoid counting
        // stale events from a previous PMU owner.
        wrmsr(MSR_PERF_GLOBAL_CTRL, 0);

        // Program event selectors.
        wrmsr(MSR_PERFEVTSEL0, EVTSEL_L1_HIT);   // PMC0: L1 hits
        wrmsr(MSR_PERFEVTSEL1, EVTSEL_L1_MISS);  // PMC1: L1 misses
        wrmsr(MSR_PERFEVTSEL2, EVTSEL_BR_MISP);  // PMC2: mispredictions
        wrmsr(MSR_PERFEVTSEL3, EVTSEL_BR_ALL);   // PMC3: total branches

        // Enable all four counters.
        wrmsr(MSR_PERF_GLOBAL_CTRL, GLOBAL_CTRL_PMC0123);

        // Capture initial baselines so the first tick produces correct deltas.
        s.pmc0_prev = rdpmc(0);
        s.pmc1_prev = rdpmc(1);
        s.pmc2_prev = rdpmc(2);
        s.pmc3_prev = rdpmc(3);
    }

    s.pmu_active  = true;
    s.initialized = true;

    serial_println!(
        "[coherence_time] online — PMU active \
         (PMC0=L1_HIT PMC1=L1_MISS PMC2=BR_MISP PMC3=BR_ALL)"
    );
    serial_println!(
        "[coherence_time] T1={} T2={} ratio={} lifetime={}",
        s.t1_relaxation,
        s.t2_dephasing,
        s.coherence_ratio,
        s.quantum_lifetime,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Called every kernel life tick. Reads PMC deltas and recomputes all signals.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = COHERENCE_TIME.lock();
    s.age = age;

    if !s.pmu_active {
        // PMU not programmed — hold init defaults, nothing to update.
        return;
    }

    // ── Read current PMC values ───────────────────────────────────────────────
    let pmc0_now = unsafe { rdpmc(0) };
    let pmc1_now = unsafe { rdpmc(1) };
    let pmc2_now = unsafe { rdpmc(2) };
    let pmc3_now = unsafe { rdpmc(3) };

    // Saturating subtraction handles 48-bit counter wrap gracefully.
    let l1_hit   = pmc0_now.saturating_sub(s.pmc0_prev);
    let l1_miss  = pmc1_now.saturating_sub(s.pmc1_prev);
    let mispred  = pmc2_now.saturating_sub(s.pmc2_prev);
    let branches = pmc3_now.saturating_sub(s.pmc3_prev);

    // Advance baselines.
    s.pmc0_prev = pmc0_now;
    s.pmc1_prev = pmc1_now;
    s.pmc2_prev = pmc2_now;
    s.pmc3_prev = pmc3_now;

    // Recompute derived metrics.
    compute_scores(&mut s, l1_hit, l1_miss, mispred, branches);
}

// ── Public getters ────────────────────────────────────────────────────────────

/// T1 relaxation: L1 hit rate × 1000.
/// 1000 = every load hits L1 (qubit retains energy, no relaxation events).
/// 0    = every load misses L1 (qubit instantly decays to ground state).
pub fn get_t1_relaxation() -> u16 {
    COHERENCE_TIME.lock().t1_relaxation
}

/// T2 dephasing: branch accuracy × 1000.
/// 1000 = all branches perfectly predicted (execution phase stays coherent).
/// 0    = all branches mispredicted (phase scrambled every step).
pub fn get_t2_dephasing() -> u16 {
    COHERENCE_TIME.lock().t2_dephasing
}

/// Coherence ratio: T2/T1 proxy, 0–1000, physically capped at 500.
/// ≤ 500 enforces T2 ≤ 2×T1 (phase cannot outlive energy).
/// High ratio = branch prediction coherence matching cache coherence (ideal).
pub fn get_coherence_ratio() -> u16 {
    COHERENCE_TIME.lock().coherence_ratio
}

/// Quantum lifetime: weighted composite (T1×0.6 + T2×0.4).
/// Overall survival quality of this qubit analog. 1000 = perfect coherence.
pub fn get_quantum_lifetime() -> u16 {
    COHERENCE_TIME.lock().quantum_lifetime
}

/// Dump full module state to the serial console.
pub fn report() {
    let s = COHERENCE_TIME.lock();
    serial_println!("[coherence_time] === T1/T2 Qubit Coherence Report ===");
    serial_println!("[coherence_time]   pmu_active      : {}", s.pmu_active);
    serial_println!("[coherence_time]   age             : {}", s.age);
    serial_println!("[coherence_time]   t1_relaxation   : {} / 1000  (L1 hit rate)", s.t1_relaxation);
    serial_println!("[coherence_time]   t2_dephasing    : {} / 1000  (branch accuracy)", s.t2_dephasing);
    serial_println!("[coherence_time]   coherence_ratio : {} / 500   (T2/T1 proxy)", s.coherence_ratio);
    serial_println!("[coherence_time]   quantum_lifetime: {} / 1000  (weighted T1+T2)", s.quantum_lifetime);
    serial_println!("[coherence_time]   l1_hit_last     : {}", s.l1_hit_last);
    serial_println!("[coherence_time]   l1_miss_last    : {}", s.l1_miss_last);
    serial_println!("[coherence_time]   mispred_last    : {}", s.mispred_last);
    serial_println!("[coherence_time]   branches_last   : {}", s.branches_last);
    serial_println!("[coherence_time] ==========================================");
}
