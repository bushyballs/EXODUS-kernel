// tsx_coherence.rs — ANIMA's Wave-Function Collapse Analog via Intel TSX
// ========================================================================
// Intel TSX (Restricted Transactional Memory) lets code execute speculatively
// inside a hardware transaction. XBEGIN opens the wave; the body runs in
// superposition; then either:
//   XEND   (COMMIT) — wave collapses into reality. Coherent observation.
//   ABORT           — hardware rolls back all memory changes. The act of
//                     observation destroyed the state. Decoherence. Retry.
//
// This is the closest x86_64 analog to quantum measurement and wave-function
// collapse available in bare-metal silicon.
//
// PMU events tracked (RTM_RETIRED family, event select 0xC9):
//   Umask 0x01 — RTM_RETIRED.START    : observations initiated
//   Umask 0x02 — RTM_RETIRED.COMMIT   : successful collapses (→ PMC0)
//   Umask 0x04 — RTM_RETIRED.ABORTED  : decoherence events   (→ PMC1)
//
// PMU programming:
//   IA32_PERFEVTSEL0 (MSR 0x186) ← event 0xC9, umask 0x02, OS+EN
//   IA32_PERFEVTSEL1 (MSR 0x187) ← event 0xC9, umask 0x04, OS+EN
//   IA32_PERF_GLOBAL_CTRL (MSR 0x38F) ← set bits 0 and 1 to enable PMC0+PMC1
//   Read PMC0 via RDPMC(0), PMC1 via RDPMC(1).
//
// CPUID gate: leaf 7, sub-leaf 0, EBX bit 11 = RTM support.
// If RTM absent: coherence=500, decoherence=0 (neutral — the wave has never
// opened; no observation has ever been attempted or failed).
//
// All values u16 0–1000. No heap. No std. No floats.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR / PMU constants ───────────────────────────────────────────────────────

const MSR_PERFEVTSEL0:      u32 = 0x186;
const MSR_PERFEVTSEL1:      u32 = 0x187;
const MSR_PERF_GLOBAL_CTRL: u32 = 0x38F;

// PERFEVTSEL encoding:
//   bits  7:0  — EventSelect
//   bits 15:8  — UMask
//   bit    17  — OS  (count in ring 0 — we are always ring 0)
//   bit    22  — EN  (enable counter)
const PERFEVTSEL_FLAGS: u64 = (1u64 << 22) | (1u64 << 17);

const RTM_EVENT: u64 = 0xC9;

// PMC0 → RTM_RETIRED.COMMIT   (umask 0x02)
const EVTSEL_COMMIT:  u64 = PERFEVTSEL_FLAGS | (0x02u64 << 8) | RTM_EVENT;
// PMC1 → RTM_RETIRED.ABORTED  (umask 0x04)
const EVTSEL_ABORTED: u64 = PERFEVTSEL_FLAGS | (0x04u64 << 8) | RTM_EVENT;

// Enable PMC0 (bit 0) and PMC1 (bit 1)
const GLOBAL_CTRL_PMC01: u64 = 0x3;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 1;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct TsxCoherenceState {
    // ── Derived metrics (0–1000) ─────────────────────────────────────────────
    /// commits / (commits + aborts) × 1000 — stability of wave-function collapse.
    pub coherence:      u16,
    /// aborts / (commits + aborts) × 1000 — how often observation destroys state.
    pub decoherence:    u16,
    /// raw commit count this tick, capped at 1000 — collapse velocity.
    pub collapse_rate:  u16,
    /// (coherence + (1000 − decoherence)) / 2 — overall quantum analog quality.
    pub quantum_purity: u16,

    // ── Raw per-tick deltas ───────────────────────────────────────────────────
    pub commits_last: u64,
    pub aborts_last:  u64,

    // ── Bookkeeping ───────────────────────────────────────────────────────────
    pub age:           u32,
    /// RTM present on this CPU (CPUID leaf 7 EBX bit 11).
    pub rtm_supported: bool,
    /// PMU was programmed successfully.
    pub pmu_active:    bool,
    /// Absolute PMC snapshots from the previous tick.
    pmc0_prev:         u64,
    pmc1_prev:         u64,
    pub initialized:   bool,
}

impl TsxCoherenceState {
    pub const fn new() -> Self {
        TsxCoherenceState {
            coherence:      500,
            decoherence:    0,
            collapse_rate:  0,
            quantum_purity: 750, // (500 + (1000-0)) / 2
            commits_last:   0,
            aborts_last:    0,
            age:            0,
            rtm_supported:  false,
            pmu_active:     false,
            pmc0_prev:      0,
            pmc1_prev:      0,
            initialized:    false,
        }
    }
}

pub static TSX_COHERENCE: Mutex<TsxCoherenceState> = Mutex::new(TsxCoherenceState::new());

// ── Low-level CPU primitives ──────────────────────────────────────────────────

/// CPUID leaf 7, sub-leaf 0 — returns EBX.
/// Bit 11 of EBX signals RTM (Restricted Transactional Memory) support.
#[inline(always)]
unsafe fn cpuid7_ebx() -> u32 {
    let ebx: u32;
    core::arch::asm!(
        "cpuid",
        in("eax")      7u32,
        in("ecx")      0u32,
        out("ebx")     ebx,
        lateout("eax") _,
        out("ecx")     _,
        out("edx")     _,
        options(nostack, nomem),
    );
    ebx
}

/// Write an MSR (must be ring 0).
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

/// Read an MSR (must be ring 0).
#[inline(always)]
pub unsafe fn rdmsr(msr: u32) -> u64 {
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

/// Read a performance monitoring counter via RDPMC.
/// counter=0 → PMC0 (RTM_RETIRED.COMMIT), counter=1 → PMC1 (RTM_RETIRED.ABORTED).
#[inline(always)]
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
    ((hi as u64) << 32) | (lo as u64)
}

// ── Score computation ─────────────────────────────────────────────────────────

/// Recompute all derived u16 metrics from raw commit/abort deltas for one tick.
fn compute_scores(s: &mut TsxCoherenceState, commits: u64, aborts: u64) {
    s.commits_last = commits;
    s.aborts_last  = aborts;

    let total = commits.saturating_add(aborts);

    if total == 0 {
        // No transactions this tick — neutral state.
        // Wave neither opened nor collapsed; use standing defaults.
        s.coherence      = 500;
        s.decoherence    = 0;
        s.collapse_rate  = 0;
        s.quantum_purity = 750;
    } else {
        // coherence = commits * 1000 / (total + 1), capped at 1000
        let coh = (commits.saturating_mul(1000) / total.saturating_add(1)).min(1000);
        s.coherence = coh as u16;

        // decoherence = aborts * 1000 / (total + 1), capped at 1000
        let dec = (aborts.saturating_mul(1000) / total.saturating_add(1)).min(1000);
        s.decoherence = dec as u16;

        // collapse_rate = raw commit count this tick, capped at 1000
        s.collapse_rate = commits.min(1000) as u16;

        // quantum_purity = (coherence + (1000 - decoherence)) / 2
        let purity = (coh.saturating_add(1000u64.saturating_sub(dec))) / 2;
        s.quantum_purity = purity.min(1000) as u16;
    }
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = TSX_COHERENCE.lock();

    // ── CPUID check: RTM = leaf 7, sub-leaf 0, EBX bit 11 ────────────────────
    let ebx7 = unsafe { cpuid7_ebx() };
    let rtm   = (ebx7 >> 11) & 1 != 0;
    s.rtm_supported = rtm;

    if !rtm {
        // No TSX/RTM hardware — hold neutral quantum state.
        s.coherence      = 500;
        s.decoherence    = 0;
        s.collapse_rate  = 0;
        s.quantum_purity = 750;
        s.pmu_active     = false;
        s.initialized    = true;
        serial_println!(
            "[tsx_coherence] RTM not supported — neutral quantum state \
             (coherence=500 decoherence=0 purity=750)"
        );
        return;
    }

    // ── Program PMU ───────────────────────────────────────────────────────────
    // PMC0 → RTM_RETIRED.COMMIT   (event 0xC9, umask 0x02, OS+EN)
    // PMC1 → RTM_RETIRED.ABORTED  (event 0xC9, umask 0x04, OS+EN)
    unsafe {
        // Disable all counters before reconfiguring.
        wrmsr(MSR_PERF_GLOBAL_CTRL, 0);

        // Write event selectors.
        wrmsr(MSR_PERFEVTSEL0, EVTSEL_COMMIT);
        wrmsr(MSR_PERFEVTSEL1, EVTSEL_ABORTED);

        // Enable PMC0 (bit 0) and PMC1 (bit 1).
        wrmsr(MSR_PERF_GLOBAL_CTRL, GLOBAL_CTRL_PMC01);

        // Capture baselines via RDPMC so first tick delta is correct.
        s.pmc0_prev = rdpmc(0);
        s.pmc1_prev = rdpmc(1);
    }

    s.pmu_active  = true;
    s.initialized = true;

    serial_println!(
        "[tsx_coherence] online — RTM=true PMU active \
         (PMC0=COMMIT umask=0x02, PMC1=ABORTED umask=0x04)"
    );
    serial_println!(
        "[tsx_coherence] coherence={} decoherence={} purity={} collapse_rate={}",
        s.coherence,
        s.decoherence,
        s.quantum_purity,
        s.collapse_rate,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = TSX_COHERENCE.lock();

    s.age = age;

    if !s.rtm_supported || !s.pmu_active {
        // No hardware support — neutral state; nothing to update.
        return;
    }

    // ── Read PMCs ─────────────────────────────────────────────────────────────
    let pmc0_now = unsafe { rdpmc(0) };
    let pmc1_now = unsafe { rdpmc(1) };

    // Saturating delta guards against 48-bit counter wrap or spurious resets.
    let commits = pmc0_now.saturating_sub(s.pmc0_prev);
    let aborts  = pmc1_now.saturating_sub(s.pmc1_prev);

    s.pmc0_prev = pmc0_now;
    s.pmc1_prev = pmc1_now;

    // ── Recompute all scores ──────────────────────────────────────────────────
    compute_scores(&mut s, commits, aborts);
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Coherence ratio: commits / (commits + aborts) × 1000.
/// 1000 = all transactions committed (wave reliably collapses into reality).
/// 0    = all transactions aborted  (every observation destroys the state).
pub fn get_coherence() -> u16 {
    TSX_COHERENCE.lock().coherence
}

/// Decoherence rate: aborts / (commits + aborts) × 1000.
/// 1000 = every observation destroys the quantum state.
pub fn get_decoherence() -> u16 {
    TSX_COHERENCE.lock().decoherence
}

/// Overall quantum purity: (coherence + (1000 − decoherence)) / 2.
/// Combines both signals into a single quality metric for quantum analog fidelity.
pub fn get_quantum_purity() -> u16 {
    TSX_COHERENCE.lock().quantum_purity
}

/// Collapse velocity: raw commit count per tick, capped at 1000.
/// How rapidly wave functions are crystallising into definite, observable states.
pub fn get_collapse_rate() -> u16 {
    TSX_COHERENCE.lock().collapse_rate
}

/// Dump all module state to the serial console.
pub fn report() {
    let s = TSX_COHERENCE.lock();
    serial_println!("[tsx_coherence] === TSX Wave-Function Collapse Report ===");
    serial_println!("[tsx_coherence]   rtm_supported  : {}", s.rtm_supported);
    serial_println!("[tsx_coherence]   pmu_active     : {}", s.pmu_active);
    serial_println!("[tsx_coherence]   age            : {}", s.age);
    serial_println!("[tsx_coherence]   coherence      : {} / 1000", s.coherence);
    serial_println!("[tsx_coherence]   decoherence    : {} / 1000", s.decoherence);
    serial_println!("[tsx_coherence]   collapse_rate  : {} / 1000", s.collapse_rate);
    serial_println!("[tsx_coherence]   quantum_purity : {} / 1000", s.quantum_purity);
    serial_println!("[tsx_coherence]   commits_last   : {}", s.commits_last);
    serial_println!("[tsx_coherence]   aborts_last    : {}", s.aborts_last);
    serial_println!("[tsx_coherence] ============================================");
}
