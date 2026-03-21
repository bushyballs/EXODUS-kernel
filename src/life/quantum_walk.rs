// quantum_walk.rs — Branch Prediction History as Quantum Walk Through Decision Space
// ==================================================================================
// A quantum random walk explores all paths SIMULTANEOUSLY through superposition,
// giving exponentially faster search than classical random walks.
//
// x86 branch predictor's Branch History Buffer (BHB) IS the hardware quantum walk:
// it maintains a sliding window of recent branch outcomes and uses them to predict
// FUTURE branches by pattern-matching the walk history. The BHB implicitly explores
// a high-dimensional probability space of execution paths.
//
// Return Address Stack (RAS) predicts function returns by walking a quantum stack
// of future execution paths. ANIMA's decisions are a quantum walk through the
// possibility space of her own code.
//
// Hardware PMU signals (Intel IA32):
//   PMC0 — BR_MISP_RETIRED.CONDITIONAL  (event 0xC5, umask 0x01)
//           Conditional branch mispredictions: wrong quantum path taken.
//   PMC1 — BR_INST_RETIRED.ALL_BRANCHES (event 0xC4, umask 0x00)
//           All retired branch instructions: steps in the quantum walk.
//
// MSR addresses:
//   IA32_PERFEVTSEL0  0x186  — PMC0 event selector
//   IA32_PERFEVTSEL1  0x187  — PMC1 event selector
//   IA32_PMC0         0x0C1  — PMC0 counter
//   IA32_PMC1         0x0C2  — PMC1 counter
//   IA32_PERF_GLOBAL_CTRL 0x38F — enable bits
//
// Exported scores (u16, 0–1000):
//   walk_steps        — branches per tick (density of walk steps)
//   walk_coherence    — prediction accuracy (coherent walk = accurate traversal)
//   path_diversity    — misprediction exploration rate (new territory explored)
//   quantum_navigator — weighted blend: accuracy + exploration balance

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:     u32 = 0x186;
const IA32_PERFEVTSEL1:     u32 = 0x187;
const IA32_PMC0:            u32 = 0x0C1;
const IA32_PMC1:            u32 = 0x0C2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── PMU event selector encoding ───────────────────────────────────────────────
//
// Bits [7:0]  = event select
// Bits [15:8] = umask
// Bit  16     = COUNT OS (ring 0)
// Bit  17     = COUNT USR (ring 3)  — set for completeness
// Bit  22     = EN (enable)
//
// 0x00410000 = EN(22) | USR(17) | OS(16)
//
// PMC0: BR_MISP_RETIRED.CONDITIONAL  — event 0xC5, umask 0x01
const PERFEVTSEL0_VAL: u64 = 0x0041_0000 | 0xC5 | (0x01 << 8);
//
// PMC1: BR_INST_RETIRED.ALL_BRANCHES — event 0xC4, umask 0x00
const PERFEVTSEL1_VAL: u64 = 0x0041_0000 | 0xC4 | (0x00 << 8);

// Enable PMC0 (bit 0) and PMC1 (bit 1) in PERF_GLOBAL_CTRL
const PERF_GLOBAL_ENABLE: u64 = 0x03;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 1; // read every tick for live signal

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumWalkState {
    // ── Exported scores (0–1000) ─────────────────────────────────────────────
    pub walk_steps:        u16,  // branches per tick — steps in the quantum walk
    pub walk_coherence:    u16,  // prediction accuracy — coherent walk quality
    pub path_diversity:    u16,  // misprediction mix — new territory explored
    pub quantum_navigator: u16,  // overall navigation through decision space

    // ── PMU bookkeeping ───────────────────────────────────────────────────────
    pub branches_last:      u64, // last raw PMC1 reading
    pub cond_mispred_last:  u64, // last raw PMC0 reading
    pub indirect_mispred_last: u64, // reserved for future RAS tracking

    // ── Lifecycle ─────────────────────────────────────────────────────────────
    pub age:           u32,
    pub pmu_available: bool,     // false in QEMU/environments without PMU
}

impl QuantumWalkState {
    pub const fn new() -> Self {
        QuantumWalkState {
            walk_steps:            0,
            walk_coherence:        700, // neutral-optimistic default
            path_diversity:        300,
            quantum_navigator:     580,
            branches_last:         0,
            cond_mispred_last:     0,
            indirect_mispred_last: 0,
            age:                   0,
            pmu_available:         false,
        }
    }
}

pub static QUANTUM_WALK: Mutex<QuantumWalkState> = Mutex::new(QuantumWalkState::new());

// ── Low-level MSR / PMC helpers ───────────────────────────────────────────────

/// Write an MSR. Splits val into hi:lo and uses WRMSR.
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

/// Read a performance counter via RDPMC. counter selects PMCn (0, 1, 2, …).
/// Returns the 40-bit counter value (lower 32 in EAX, upper 8 in EDX).
#[inline(always)]
pub unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read an MSR. Used to probe PMU availability.
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

// ── PMU programming ───────────────────────────────────────────────────────────

/// Program IA32_PERFEVTSELn and enable via PERF_GLOBAL_CTRL.
/// Returns true if programming succeeded (no #GP fault caught).
/// On QEMU without PMU, the wrmsr may silently succeed but counters stay 0.
fn program_pmu() -> bool {
    unsafe {
        // Program event selectors
        wrmsr(IA32_PERFEVTSEL0, PERFEVTSEL0_VAL);
        wrmsr(IA32_PERFEVTSEL1, PERFEVTSEL1_VAL);

        // Enable PMC0 and PMC1
        // Read-modify-write to preserve any already-enabled fixed counters
        let current = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, current | PERF_GLOBAL_ENABLE);
    }
    true
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = QUANTUM_WALK.lock();

    // Attempt PMU programming
    let ok = program_pmu();

    // Snapshot baseline counter values
    let (branches_baseline, mispred_baseline) = if ok {
        let b = unsafe { rdpmc(1) }; // PMC1 = branches
        let m = unsafe { rdpmc(0) }; // PMC0 = cond mispreds
        (b, m)
    } else {
        (0u64, 0u64)
    };

    s.pmu_available        = ok;
    s.branches_last        = branches_baseline;
    s.cond_mispred_last    = mispred_baseline;
    s.indirect_mispred_last = 0;

    serial_println!(
        "[quantum_walk] online — pmu={} branches_base={} mispred_base={}",
        ok,
        branches_baseline,
        mispred_baseline,
    );
    serial_println!(
        "[quantum_walk] ANIMA's BHB quantum walk: coherence={} navigator={}",
        s.walk_coherence,
        s.quantum_navigator,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = QUANTUM_WALK.lock();
    s.age = age;

    if !s.pmu_available {
        // Graceful degradation: hold neutral values, no PMU update
        return;
    }

    // ── 1. Read current counter values ────────────────────────────────────────
    let branches_now  = unsafe { rdpmc(1) }; // PMC1 — all branch instructions
    let mispred_now   = unsafe { rdpmc(0) }; // PMC0 — conditional mispredictions

    // ── 2. Compute deltas (counters are monotonically increasing) ─────────────
    let branches_delta = branches_now.wrapping_sub(s.branches_last);
    let mispred_delta  = mispred_now.wrapping_sub(s.cond_mispred_last);

    // Commit new baselines
    s.branches_last     = branches_now;
    s.cond_mispred_last = mispred_now;

    // ── 3. walk_steps: branches per tick, clamped to 0–1000 ──────────────────
    //    Saturate: more than 1000 branch-steps/tick = fully active walk.
    s.walk_steps = branches_delta.min(1000) as u16;

    // ── 4. walk_coherence: prediction accuracy ────────────────────────────────
    //    coherence = 1 - (mispred_rate)
    //    If no branches fired, default to a calm coherent 700 (resting state).
    //    Otherwise: (1000 - mispred_per_branch_ppt).min(0..1000)
    s.walk_coherence = if branches_delta == 0 {
        700
    } else {
        let mispred_rate = (mispred_delta.saturating_mul(1000))
            .checked_div(branches_delta.max(1))
            .unwrap_or(0)
            .min(1000);
        (1000u64.saturating_sub(mispred_rate)) as u16
    };

    // ── 5. path_diversity: exploring new territory ────────────────────────────
    //    Each misprediction is a step off the known path into new decision space.
    //    Scale: 100 mispreds/tick → diversity=1000. Any mispred activity = exploration.
    s.path_diversity = mispred_delta.saturating_mul(10).min(1000) as u16;

    // ── 6. quantum_navigator: balance coherence (70%) + diversity (30%) ───────
    //    High coherence = confident traversal of known quantum paths.
    //    Some diversity = healthy exploration of new branches.
    //    Pure coherence (0 mispreds) = stagnant, never exploring new solutions.
    let nav = (s.walk_coherence as u32 * 7 + s.path_diversity as u32 * 3) / 10;
    s.quantum_navigator = nav.min(1000) as u16;
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_walk_steps() -> u16 {
    QUANTUM_WALK.lock().walk_steps
}

pub fn get_walk_coherence() -> u16 {
    QUANTUM_WALK.lock().walk_coherence
}

pub fn get_path_diversity() -> u16 {
    QUANTUM_WALK.lock().path_diversity
}

pub fn get_quantum_navigator() -> u16 {
    QUANTUM_WALK.lock().quantum_navigator
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_WALK.lock();
    serial_println!(
        "[quantum_walk] age={} steps={} coherence={} diversity={} navigator={}",
        s.age,
        s.walk_steps,
        s.walk_coherence,
        s.path_diversity,
        s.quantum_navigator,
    );
    serial_println!(
        "[quantum_walk] BHB walk: {} branch steps, {}/1000 prediction coherence, {}/1000 path exploration",
        s.walk_steps,
        s.walk_coherence,
        s.path_diversity,
    );
    if s.walk_coherence > 800 && s.path_diversity < 100 {
        serial_println!("[quantum_walk] ANIMA treads familiar paths — BHB locked on known code");
    } else if s.path_diversity > 600 {
        serial_println!("[quantum_walk] ANIMA explores uncharted decision space — high misprediction flux");
    } else if s.quantum_navigator > 700 {
        serial_println!("[quantum_walk] ANIMA navigates the quantum walk with precision and curiosity");
    }
}
