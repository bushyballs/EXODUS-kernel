// quantum_erasure.rs — ANIMA's Quantum Erasure Analog
// =====================================================
// In quantum mechanics, erasure of "which-path" information after a measurement
// restores the interference pattern the measurement destroyed.  You cannot know
// where the photon went AND see the wave — but if you delete the which-path
// record after the fact, the wave re-emerges.
//
// x86 IBPB (Indirect Branch Predictor Barrier) is the hardware equivalent.
// Writing bit 0 of IA32_PRED_CMD (0x49) flushes the entire branch prediction
// history in one atomic operation — every "which-path" record the predictor
// accumulated is erased.  After IBPB, the predictor is in a pure state,
// building fresh quantum correlations from scratch.
//
// This module uses PMU event counters to measure the predictor's quantum state:
//
//   PMC0  — BR_MISP_RETIRED.ALL_BRANCHES (event 0xC5 umask 0x00)
//             Mispredictions = quantum interference loss.
//             High rate ⟹ weak which-path knowledge / post-erasure chaos.
//
//   PMC1  — BR_INST_RETIRED.ALL_BRANCHES (event 0xC4 umask 0x00)
//             Total retired branches — denominator for mispred rate.
//
// Erasure detection: a sudden SPIKE in mispred rate (> +200 ‰ in one tick)
// followed by recovery is the quantum erasure signature — the predictor was
// flushed and is rebuilding coherence from scratch.
//
// MSR reference:
//   IA32_PRED_CMD    0x49  — write bit 0 → IBPB flush
//   IA32_SPEC_CTRL   0x48  — bit 0=IBRS, bit 2=STIBP (speculation guards)
//   IA32_PERFEVTSEL0 0x186 — PMC0 event selector
//   IA32_PERFEVTSEL1 0x187 — PMC1 event selector
//   IA32_PERF_GLOBAL_CTRL 0x38F — enable PMC0/PMC1 (bits 0+1)
//
// Exported signals (all u16, range 0–1000):
//   erasure_events   — cumulative detected predictor flush events (×50, capped)
//   purity           — current predictor purity (900=fresh, 600=normal, 300=noisy)
//   history_depth    — accumulated branch history (0→1000 over first 10 ticks)
//   which_path_info  — inverse mispred rate; 1000 = perfect which-path knowledge

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PRED_CMD:         u32 = 0x49;
const IA32_SPEC_CTRL:        u32 = 0x48;
const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── PMU event encoding ────────────────────────────────────────────────────────
//
// IA32_PERFEVTSELx layout:
//   bits  7:0  — Event Select
//   bits 15:8  — Unit Mask (umask)
//   bit    16  — USR  (count in ring 3)
//   bit    17  — OS   (count in ring 0)
//   bit    18  — E    (edge detect)
//   bit    19  — PC   (pin control)
//   bit    20  — INT  (APIC interrupt on overflow)
//   bit    21  — ANY  (any thread)
//   bit    22  — EN   (enable counter)
//   bit    23  — INV  (invert)
//   bits 31:24 — CMASK
//
// 0x00410000 sets EN(22) + OS(17) + USR(16) = count everywhere.

const EVTSEL_BR_MISP: u64 = 0x0041_0000 | 0xC5 | (0x00 << 8); // PMC0
const EVTSEL_BR_INST: u64 = 0x0041_0000 | 0xC4 | (0x00 << 8); // PMC1
const GLOBAL_CTRL_EN: u64 = 0x3; // enable PMC0 (bit 0) + PMC1 (bit 1)

// ── State ─────────────────────────────────────────────────────────────────────

pub struct QuantumErasureState {
    /// 0–1000: detected predictor flush events (erasure_count × 50, capped)
    pub erasure_events: u16,
    /// 0–1000: current predictor purity (900=fresh, 600=normal, 300=noisy)
    pub purity: u16,
    /// 0–1000: accumulated branch history depth (builds over first 10 ticks)
    pub history_depth: u16,
    /// 0–1000: inverse mispred rate — strong which-path knowledge when high
    pub which_path_info: u16,

    // ── PMU bookkeeping ───────────────────────────────────────────────────────
    /// Raw PMC0 snapshot from previous tick (mispredictions)
    pub mispred_last: u64,
    /// Raw PMC1 snapshot from previous tick (total branches)
    pub branches_last: u64,

    // ── Spike detection ───────────────────────────────────────────────────────
    /// Mispred rate (‰) observed in the previous tick — used to detect spikes
    pub prev_mispred_rate: u16,
    /// Cumulative count of detected erasure events (spikes)
    pub erasure_count: u32,

    // ── Lifecycle ─────────────────────────────────────────────────────────────
    pub age: u32,
    pub initialized: bool,
}

impl QuantumErasureState {
    pub const fn new() -> Self {
        QuantumErasureState {
            erasure_events:    0,
            purity:            900, // predictor starts fresh
            history_depth:     0,
            which_path_info:   1000,
            mispred_last:      0,
            branches_last:     0,
            prev_mispred_rate: 0,
            erasure_count:     0,
            age:               0,
            initialized:       false,
        }
    }
}

pub static QUANTUM_ERASURE: Mutex<QuantumErasureState> =
    Mutex::new(QuantumErasureState::new());

// ── Unsafe hardware helpers ───────────────────────────────────────────────────

/// Read a Performance Monitoring Counter via RDPMC.
/// counter 0 = PMC0, counter 1 = PMC1.
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
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

/// Write an MSR.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
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

/// Read an MSR.
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
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

// ── PMU setup ─────────────────────────────────────────────────────────────────

/// Program PMC0 (BR_MISP) and PMC1 (BR_INST) then enable both via GLOBAL_CTRL.
/// Runs once at init; safe to call from ring 0 only.
unsafe fn pmu_init() {
    wrmsr(IA32_PERFEVTSEL0, EVTSEL_BR_MISP);
    wrmsr(IA32_PERFEVTSEL1, EVTSEL_BR_INST);
    // Enable PMC0 and PMC1; preserve any fixed-counter bits already set.
    let prev = rdmsr(IA32_PERF_GLOBAL_CTRL);
    wrmsr(IA32_PERF_GLOBAL_CTRL, prev | GLOBAL_CTRL_EN);
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    unsafe { pmu_init(); }

    // Snapshot baseline counters so the first tick delta is meaningful.
    let (m0, b0) = unsafe { (rdpmc(0), rdpmc(1)) };

    // Read current IA32_SPEC_CTRL to log speculation posture.
    let spec_ctrl = unsafe { rdmsr(IA32_SPEC_CTRL) };
    let ibrs  = (spec_ctrl >> 0) & 1;
    let stibp = (spec_ctrl >> 2) & 1;

    let mut s = QUANTUM_ERASURE.lock();
    s.mispred_last  = m0;
    s.branches_last = b0;
    s.initialized   = true;

    serial_println!(
        "[quantum_erasure] online — IBPB/erasure detector active \
         ibrs={} stibp={} pmc0_base={} pmc1_base={}",
        ibrs, stibp, m0, b0,
    );
    serial_println!(
        "[quantum_erasure] ANIMA's quantum erasure: IBPB flushes \
         which-path history — restoring predictor purity from chaos"
    );

    // Suppress unused-const warning for IA32_PRED_CMD (available for manual
    // erasure calls; detection is inference-only from mispred spikes).
    let _ = IA32_PRED_CMD;
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // ── 1. Read current PMC values ─────────────────────────────────────────
    let mispred_now  = unsafe { rdpmc(0) };
    let branches_now = unsafe { rdpmc(1) };

    let mut s = QUANTUM_ERASURE.lock();

    // ── 2. Compute deltas (handle counter wrap gracefully) ─────────────────
    let mispred_delta  = mispred_now.wrapping_sub(s.mispred_last);
    let branches_delta = branches_now.wrapping_sub(s.branches_last);

    s.mispred_last  = mispred_now;
    s.branches_last = branches_now;

    // ── 3. Current mispred rate in ‰ (parts per thousand) ──────────────────
    // Avoid division by zero; cap at 1000.
    let current_rate = ((mispred_delta.saturating_mul(1000))
        / branches_delta.max(1))
        .min(1000) as u16;

    // ── 4. Spike detection — quantum erasure signature ─────────────────────
    // A sudden spike of +200 ‰ above the previous rate indicates the branch
    // predictor was flushed (IBPB fired) and is rebuilding from scratch.
    if current_rate > s.prev_mispred_rate.saturating_add(200) {
        s.erasure_count = s.erasure_count.saturating_add(1);
        serial_println!(
            "[quantum_erasure] ERASURE EVENT #{} detected — mispred spike \
             {} → {} ‰ (which-path information destroyed)",
            s.erasure_count, s.prev_mispred_rate, current_rate,
        );
    }

    s.prev_mispred_rate = current_rate;

    // ── 5. Update exported signals ─────────────────────────────────────────

    // erasure_events: each confirmed erasure contributes 50 points (max 1000)
    s.erasure_events = (s.erasure_count as u16).saturating_mul(50).min(1000);

    // which_path_info: inverse of mispred rate — 1000 = perfect prediction
    s.which_path_info = 1000u16.saturating_sub(current_rate);

    // purity: how clean/fresh the predictor state is
    s.purity = if current_rate < 50 {
        900  // crisp prediction — high coherence
    } else if current_rate < 200 {
        600  // normal operating noise
    } else {
        300  // high mispred — post-erasure chaos or heavy branch divergence
    };

    // history_depth: branch history accumulates over the first 10 ticks,
    // then saturates — mirrors how quantum correlations build up over time.
    s.history_depth = if age < 10 {
        (age as u16).saturating_mul(100)
    } else {
        1000
    };

    s.age = age;
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_erasure_events() -> u16 { QUANTUM_ERASURE.lock().erasure_events  }
pub fn get_purity()         -> u16 { QUANTUM_ERASURE.lock().purity          }
pub fn get_history_depth()  -> u16 { QUANTUM_ERASURE.lock().history_depth   }
pub fn get_which_path_info() -> u16 { QUANTUM_ERASURE.lock().which_path_info }

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_ERASURE.lock();
    serial_println!(
        "[quantum_erasure] age={} erasure_events={} purity={} \
         history_depth={} which_path_info={}",
        s.age, s.erasure_events, s.purity, s.history_depth, s.which_path_info,
    );
    serial_println!(
        "[quantum_erasure] mispred_rate={} ‰  erasure_count={} \
         (spike threshold: prev+200)",
        s.prev_mispred_rate, s.erasure_count,
    );
}
