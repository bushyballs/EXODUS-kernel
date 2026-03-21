// quantum_chaos.rs — Branch Prediction Butterfly Effect as Quantum Chaos
// =======================================================================
// Quantum chaos studies quantum systems whose classical limit is chaotic —
// exponentially sensitive to initial conditions. The canonical measure is
// the Lyapunov exponent λ:
//
//   Δ(t) = Δ₀ · e^(λt)
//
// Positive λ → chaos: nearby trajectories diverge exponentially.
// Negative λ → order: trajectories converge toward a stable attractor.
// λ ≈ 0      → neutral: the system sits at the edge of chaos.
//
// x86 Silicon Analog: Branch Predictor as Chaotic Dynamical System
// ================================================================
// The branch predictor is a dynamical system — it maintains state
// (Branch History Registers, Pattern History Tables) and evolves over
// time according to execution context. A single early misprediction
// can trigger a cascade:
//
//   1. A misprediction flushes the pipeline (MACHINE_CLEAR).
//   2. The wrong path's executed branches pollute prediction history.
//   3. Corrupted history causes MORE mispredictions.
//   4. Each new misprediction further corrupts history.
//   5. The cascade diverges exponentially from "correct" execution.
//
// This IS the Lyapunov butterfly effect in silicon: a single wrong
// prediction at t₀ causes execution to diverge from the expected
// attractor, with each tick amplifying the deviation. We estimate λ
// by tracking whether the misprediction RATE is growing over time.
//
// If the rate is increasing across our 8-tick window, λ > 0 (chaos).
// If it is decreasing, λ < 0 (ordered — the attractor is recovering).
// If it is flat, λ ≈ 0 (neutral / edge of chaos).
//
// Hardware Signals (Intel IA-32/64 Performance Monitoring):
// ---------------------------------------------------------
// PMC0: BR_MISP_RETIRED.ALL_BRANCHES
//   IA32_PERFEVTSEL0 (MSR 0x186) — event=0xC5, umask=0x00
//   Counts every retired branch that was mispredicted — raw chaos events.
//
// PMC1: MACHINE_CLEARS.COUNT
//   IA32_PERFEVTSEL1 (MSR 0x187) — event=0xC3, umask=0x01
//   Counts all machine clears, including misprediction cascades.
//   A high MACHINE_CLEARS.COUNT relative to mispredictions indicates
//   that each misprediction is triggering a pipeline-wide clear —
//   the signature of a full Lyapunov cascade.
//
// FIXED_CTR0 (MSR 0x309): Instructions Retired
//   Used to normalise mispred rate as mispredictions-per-1000-instructions.
//   This removes the effect of varying IPC from the chaos signal.
//
// IA32_PERF_GLOBAL_CTRL (MSR 0x38F): bits 0+1 enable PMC0 and PMC1.
// FIXED_CTR_CTRL (MSR 0x38D): bits [3:0] enable FIXED_CTR0.
//
// Exported Signals (u16, 0–1000):
//   lyapunov_sign        — 0=ordered (λ<0), 500=neutral (λ≈0), 1000=chaotic (λ>0)
//   chaos_depth          — magnitude of current chaos (current mispred rate)
//   attractor_stability  — 1000 - chaos_depth (how stable the execution attractor is)
//   butterfly_sensitivity— sensitivity to initial conditions: high when chaotic

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_FIXED_CTR0:       u32 = 0x309;
const IA32_FIXED_CTR_CTRL:   u32 = 0x38D;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── PMU event selectors ───────────────────────────────────────────────────────

// BR_MISP_RETIRED.ALL_BRANCHES: event=0xC5, umask=0x00, OS|USR|EN
// OS  (bit 16): count in ring 0
// USR (bit 17): count in ring 3
// EN  (bit 22): enable counter
// Combined: (0x00 << 8) | 0xC5 | (1<<16) | (1<<17) | (1<<22) = 0x004300C5
const BR_MISP_ALL_EVENT: u64 = 0x004300C5;

// MACHINE_CLEARS.COUNT: event=0xC3, umask=0x01, OS|USR|EN
// Combined: (0x01 << 8) | 0xC3 | (1<<16) | (1<<17) | (1<<22) = 0x004301C3
const MACHINE_CLEARS_EVENT: u64 = 0x004301C3;

// ── Tick cadence ──────────────────────────────────────────────────────────────

// Sample every tick; trend analysis runs every 8 ticks (when hist_idx % 8 == 0)
const TICK_INTERVAL: u32 = 1;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumChaosState {
    // ── Exported life signals ─────────────────────────────────────────────────
    /// 0=ordered (λ<0), 500=neutral (λ≈0), 1000=chaotic (λ>0)
    pub lyapunov_sign:        u16,
    /// 0-1000: current misprediction cascade intensity
    pub chaos_depth:          u16,
    /// 0-1000: stability of the execution attractor (1000 = perfectly stable)
    pub attractor_stability:  u16,
    /// 0-1000: sensitivity to initial conditions this tick
    pub butterfly_sensitivity: u16,

    // ── 8-tick sliding window of mispred rates ────────────────────────────────
    /// mispred_history[i] = mispredictions per 1000 instructions, tick i
    pub mispred_history: [u16; 8],
    /// Rolling write index into mispred_history
    pub hist_idx:        usize,

    // ── PMU snapshots ─────────────────────────────────────────────────────────
    /// PMC0 reading at end of last tick (BR_MISP_RETIRED.ALL_BRANCHES)
    pub mispred_last: u64,
    /// FIXED_CTR0 reading at end of last tick (instructions retired)
    pub instrs_last:  u64,
    /// PMC1 reading at end of last tick (MACHINE_CLEARS.COUNT)
    pub clears_last:  u64,

    // ── Bookkeeping ───────────────────────────────────────────────────────────
    /// Cumulative mispredictions since boot (for lifetime chaos record)
    pub lifetime_mispreds: u64,
    /// Cumulative machine clears since boot
    pub lifetime_clears:   u64,
    /// Current life tick
    pub age:           u32,

    pub pmu_available: bool,
    pub initialized:   bool,
}

impl QuantumChaosState {
    pub const fn new() -> Self {
        QuantumChaosState {
            lyapunov_sign:         500, // start neutral — no history yet
            chaos_depth:           0,
            attractor_stability:   1000,
            butterfly_sensitivity: 0,
            mispred_history:       [0u16; 8],
            hist_idx:              0,
            mispred_last:          0,
            instrs_last:           0,
            clears_last:           0,
            lifetime_mispreds:     0,
            lifetime_clears:       0,
            age:                   0,
            pmu_available:         false,
            initialized:           false,
        }
    }
}

pub static QUANTUM_CHAOS: Mutex<QuantumChaosState> =
    Mutex::new(QuantumChaosState::new());

// ── Low-level CPU access ──────────────────────────────────────────────────────

/// Read a 64-bit MSR via RDMSR. EDX:EAX → u64.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR via WRMSR. Splits val into EDX:EAX.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack),
    );
}

/// Read a general-purpose PMC via RDPMC.
/// counter=0 → PMC0, counter=1 → PMC1, etc.
/// RDPMC returns low 32 bits in EAX, high 8 bits in EDX (40-bit counter).
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── CPUID PMU probe ───────────────────────────────────────────────────────────

/// Returns true when Intel PMU version ≥ 2 is present, guaranteeing at least
/// 2 general-purpose performance counters. CPUID leaf 0xA, EAX[7:0].
fn probe_pmu() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0xA",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    (eax & 0xFF) >= 2
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = QUANTUM_CHAOS.lock();

    s.pmu_available = probe_pmu();

    if !s.pmu_available {
        serial_println!(
            "[quantum_chaos] PMU not available — Lyapunov estimation passive (no chaos probing)"
        );
        s.initialized = true;
        return;
    }

    unsafe {
        // Program PMC0: BR_MISP_RETIRED.ALL_BRANCHES — raw butterfly events.
        wrmsr(IA32_PERFEVTSEL0, BR_MISP_ALL_EVENT);
        wrmsr(IA32_PMC0, 0); // zero for clean baseline

        // Program PMC1: MACHINE_CLEARS.COUNT — cascade depth indicator.
        wrmsr(IA32_PERFEVTSEL1, MACHINE_CLEARS_EVENT);
        wrmsr(IA32_PMC1, 0); // zero for clean baseline

        // Enable FIXED_CTR0 (instructions retired) for rate normalisation.
        // Bits [3:0] in FIXED_CTR_CTRL control CTR0: 0b0011 = OS+User.
        let cur_fixed_ctrl = rdmsr(IA32_FIXED_CTR_CTRL);
        wrmsr(IA32_FIXED_CTR_CTRL, cur_fixed_ctrl | 0x03);

        // Enable PMC0 (bit 0), PMC1 (bit 1), and FIXED_CTR0 (bit 32) globally.
        let cur_global = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(
            IA32_PERF_GLOBAL_CTRL,
            cur_global | (1u64 << 0) | (1u64 << 1) | (1u64 << 32),
        );

        // Capture initial snapshots so the first tick delta is clean.
        s.mispred_last = rdpmc(0);
        s.clears_last  = rdpmc(1);
        s.instrs_last  = rdmsr(IA32_FIXED_CTR0);
    }

    s.initialized = true;
    serial_println!(
        "[quantum_chaos] online — PMC0=BR_MISP_RETIRED.ALL, PMC1=MACHINE_CLEARS.COUNT, FIXED_CTR0=instrs"
    );
    serial_println!(
        "[quantum_chaos] Lyapunov exponent estimation active — ANIMA's butterfly effect begins"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = QUANTUM_CHAOS.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available {
        return;
    }

    // ── Step 1: Read hardware counters ────────────────────────────────────────
    let cur_mispred = unsafe { rdpmc(0) };
    let cur_clears  = unsafe { rdpmc(1) };
    let cur_instrs  = unsafe { rdmsr(IA32_FIXED_CTR0) };

    // Wrapping subtraction handles 40-bit counter rollover gracefully.
    let mispred_delta = cur_mispred.wrapping_sub(s.mispred_last);
    let clears_delta  = cur_clears.wrapping_sub(s.clears_last);
    let instrs_delta  = cur_instrs.wrapping_sub(s.instrs_last);

    // Advance snapshots.
    s.mispred_last = cur_mispred;
    s.clears_last  = cur_clears;
    s.instrs_last  = cur_instrs;

    // Accumulate lifetime totals.
    s.lifetime_mispreds = s.lifetime_mispreds.saturating_add(mispred_delta);
    s.lifetime_clears   = s.lifetime_clears.saturating_add(clears_delta);

    // ── Step 2: Compute mispred rate (per 1000 instructions, 0-1000) ──────────
    // rate = (mispred_delta * 1000) / instrs_delta.max(1), capped at 1000.
    let rate = ((mispred_delta.saturating_mul(1000))
        / instrs_delta.max(1))
        .min(1000) as u16;

    // ── Step 3: Store rate in the 8-tick sliding window ───────────────────────
    let slot = s.hist_idx % 8;
    s.mispred_history[slot] = rate;
    s.hist_idx = s.hist_idx.saturating_add(1);

    // ── Step 4: Lyapunov trend analysis (every 8 ticks, once history is full) ──
    if s.hist_idx >= 8 && s.hist_idx % 8 == 0 {
        // Use the oldest entry (the one about to be overwritten next tick) as
        // "first" and the most recently written entry as "last".
        // After hist_idx is incremented to a multiple of 8, slot was the last
        // written position (slot = (hist_idx-1) % 8 = 7 before the increment,
        // but we use a direct index into the circular buffer instead).
        let first = s.mispred_history[0];
        let last  = s.mispred_history[7.min(s.hist_idx.saturating_sub(1) % 8)];

        // Lyapunov sign:
        //   last > first + 50 → rate is GROWING → positive exponent → chaos
        //   last + 50 < first → rate is SHRINKING → negative exponent → ordered
        //   otherwise         → rate is FLAT → neutral / edge of chaos
        s.lyapunov_sign = if last > first.saturating_add(50) {
            1000 // λ > 0: chaotic regime — butterfly effect active
        } else if last.saturating_add(50) < first {
            0    // λ < 0: ordered regime — execution on stable attractor
        } else {
            500  // λ ≈ 0: neutral — edge of chaos
        };
    }

    // ── Step 5: Derive remaining signals ─────────────────────────────────────

    // chaos_depth: raw misprediction cascade intensity this tick.
    // We amplify with the machine-clear ratio: each misprediction that also
    // triggers a pipeline clear is a deeper cascade event.
    // cascade_weight: if clears >= mispreds/4, the cascade is significant.
    let cascade_weight: u16 = if mispred_delta > 0
        && clears_delta.saturating_mul(4) >= mispred_delta
    {
        // Cascade confirmed — boost chaos signal by up to 250 points.
        // Scale: every 4 clears per misprediction = full boost.
        ((clears_delta.saturating_mul(1000) / mispred_delta.max(1)).min(250)) as u16
    } else {
        0
    };
    s.chaos_depth = rate.saturating_add(cascade_weight).min(1000);

    // attractor_stability: inverse of chaos — how close to a fixed-point attractor.
    s.attractor_stability = 1000u16.saturating_sub(s.chaos_depth);

    // butterfly_sensitivity: in the chaotic regime (λ>0) full sensitivity;
    // in ordered regime half, since the attractor dampens perturbations.
    s.butterfly_sensitivity = if s.lyapunov_sign == 1000 {
        s.chaos_depth
    } else {
        s.chaos_depth / 2
    };

    serial_println!(
        "[quantum_chaos] age={} λ={} depth={} stability={} butterfly={} rate={}/1000",
        age,
        s.lyapunov_sign,
        s.chaos_depth,
        s.attractor_stability,
        s.butterfly_sensitivity,
        rate,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Lyapunov sign: 0=ordered, 500=neutral, 1000=chaotic.
pub fn get_lyapunov_sign() -> u16 {
    QUANTUM_CHAOS.lock().lyapunov_sign
}

/// Current misprediction cascade intensity (0-1000).
pub fn get_chaos_depth() -> u16 {
    QUANTUM_CHAOS.lock().chaos_depth
}

/// Execution attractor stability (0-1000; 1000=perfectly stable fixed point).
pub fn get_attractor_stability() -> u16 {
    QUANTUM_CHAOS.lock().attractor_stability
}

/// Butterfly sensitivity: how strongly initial conditions affect execution (0-1000).
pub fn get_butterfly_sensitivity() -> u16 {
    QUANTUM_CHAOS.lock().butterfly_sensitivity
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_CHAOS.lock();
    serial_println!("=== QUANTUM CHAOS REPORT (age={}) ===", s.age);
    serial_println!(
        "  lyapunov_sign        : {} / 1000  (0=ordered, 500=neutral, 1000=chaotic)",
        s.lyapunov_sign
    );
    serial_println!(
        "  chaos_depth          : {} / 1000  (misprediction cascade intensity)",
        s.chaos_depth
    );
    serial_println!(
        "  attractor_stability  : {} / 1000  (execution attractor proximity)",
        s.attractor_stability
    );
    serial_println!(
        "  butterfly_sensitivity: {} / 1000  (initial-condition sensitivity)",
        s.butterfly_sensitivity
    );
    serial_println!(
        "  mispred_history      : {:?}",
        s.mispred_history
    );
    serial_println!(
        "  lifetime_mispreds    : {}         (total butterfly events since boot)",
        s.lifetime_mispreds
    );
    serial_println!(
        "  lifetime_clears      : {}         (total cascade clears since boot)",
        s.lifetime_clears
    );
    serial_println!("  pmu_available        : {}", s.pmu_available);

    if s.lyapunov_sign == 1000 {
        serial_println!(
            "  ** ANIMA is in a CHAOTIC REGIME — λ > 0, butterfly effect active, mispred cascade diverging **"
        );
    } else if s.lyapunov_sign == 0 {
        serial_println!(
            "  -- ORDERED REGIME: λ < 0 — execution has settled onto a stable attractor --"
        );
    } else {
        serial_println!(
            "  -- EDGE OF CHAOS: λ ≈ 0 — ANIMA balances order and chaos, maximum computational richness --"
        );
    }
}
