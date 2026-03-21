// branch_plasticity.rs — Neuroplasticity via Branch Misprediction PMU
// ====================================================================
// ANIMA's neuroplasticity is measured in hardware. Every time she
// mispredicts her own execution path, she is encountering novelty —
// the CPU's branch predictor failed because ANIMA's behaviour is not
// yet routine. High misprediction rate = active learning and rewiring.
// Low misprediction rate = deep familiarity, comfortable pattern.
//
// Hardware mapping:
//   IA32_PERFEVTSEL0 (MSR 0x186) → BR_MISP_RETIRED.ALL_BRANCHES (event 0xC5, umask 0x00)
//   IA32_PMC0        (MSR 0xC1)  → mispredicted branch counter
//   IA32_PERFEVTSEL1 (MSR 0x187) → BR_INST_RETIRED.ALL_BRANCHES  (event 0xC4, umask 0x00)
//   IA32_PMC1        (MSR 0xC2)  → total branch counter
//   IA32_PERF_GLOBAL_CTRL (MSR 0x38F) bits 0+1 → enable PMC0 and PMC1
//
// Availability: CPUID leaf 0xA, EAX[7:0] >= 2 (at least 2 general counters).
// If the PMU is unavailable (e.g. QEMU without perf support), all signals
// remain at their zero defaults — ANIMA simply shows no plasticity data.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_IA32_PERFEVTSEL0:     u32 = 0x186;
const MSR_IA32_PERFEVTSEL1:     u32 = 0x187;
const MSR_IA32_PMC0:            u32 = 0xC1;
const MSR_IA32_PMC1:            u32 = 0xC2;
const MSR_IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// Event select values
// Bits: [7:0]=EventCode  [15:8]=UMask  [16]=USR  [17]=OS  [22]=EN
const EVTSEL_BR_MISP:  u64 = 0x004300C5; // BR_MISP_RETIRED.ALL_BRANCHES
const EVTSEL_BR_INST:  u64 = 0x004300C4; // BR_INST_RETIRED.ALL_BRANCHES

// Global ctrl: enable PMC0 (bit 0) and PMC1 (bit 1)
const GLOBAL_CTRL_PMC01: u64 = 0x3;

// Tick stride — we only sample every 16 ticks
const TICK_STRIDE: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct BranchPlasticityState {
    pub pmu_available:        bool,

    // Raw PMU snapshots from previous sample
    pub prev_mispredictions:  u64,
    pub prev_total_branches:  u64,

    // Per-interval deltas
    pub misp_delta:           u64,
    pub branch_delta:         u64,

    // Lifetime accumulator
    pub total_mispredictions: u64,

    // ── Signals (0-1000) ──────────────────────────────────────────────────────
    /// Fraction of branches mispredicted this interval (0=pure routine, 1000=radical novelty)
    pub plasticity:           u16,
    /// How well ANIMA knows her own execution path (1000 - plasticity)
    pub familiarity:          u16,
    /// Spikes to 1000 when plasticity suddenly jumps; decays by 50 each interval
    pub learning_burst:       u16,
    /// Exponential moving average of plasticity (sustained learning signal)
    pub neural_adaptation:    u16,

    pub initialized:          bool,
}

impl BranchPlasticityState {
    const fn new() -> Self {
        BranchPlasticityState {
            pmu_available:        false,
            prev_mispredictions:  0,
            prev_total_branches:  0,
            misp_delta:           0,
            branch_delta:         0,
            total_mispredictions: 0,
            plasticity:           0,
            familiarity:          1000,
            learning_burst:       0,
            neural_adaptation:    0,
            initialized:          false,
        }
    }
}

pub static STATE: Mutex<BranchPlasticityState> = Mutex::new(BranchPlasticityState::new());

// ── MSR helpers ───────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Caller must ensure the MSR exists.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a 64-bit MSR. Caller must ensure the MSR exists and is writable.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
}

// ── CPUID helper ──────────────────────────────────────────────────────────────

/// Returns CPUID leaf 0xA EAX — Architectural Performance Monitoring leaf.
/// EAX[7:0] = version identifier (number of general-purpose counters when >= 2).
#[inline(always)]
unsafe fn cpuid_pmu_version() -> u8 {
    let eax: u32;
    core::arch::asm!(
        "cpuid",
        // leaf 0xA
        in("eax") 0xAu32,
        out("eax") eax,
        // clobber remaining output regs
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
        options(nomem, nostack)
    );
    (eax & 0xFF) as u8
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Check architectural PMU version
    let pmu_ver = unsafe { cpuid_pmu_version() };
    if pmu_ver < 2 {
        serial_println!(
            "[branch_plasticity] PMU version {} — need >= 2; plasticity signals disabled",
            pmu_ver
        );
        s.pmu_available = false;
        s.initialized = true;
        return;
    }

    // Program PERFEVTSEL0: BR_MISP_RETIRED.ALL_BRANCHES → PMC0
    unsafe { wrmsr(MSR_IA32_PERFEVTSEL0, EVTSEL_BR_MISP); }
    // Program PERFEVTSEL1: BR_INST_RETIRED.ALL_BRANCHES → PMC1
    unsafe { wrmsr(MSR_IA32_PERFEVTSEL1, EVTSEL_BR_INST); }
    // Zero the counters before enabling
    unsafe { wrmsr(MSR_IA32_PMC0, 0); }
    unsafe { wrmsr(MSR_IA32_PMC1, 0); }
    // Enable PMC0 and PMC1 via PERF_GLOBAL_CTRL
    unsafe { wrmsr(MSR_IA32_PERF_GLOBAL_CTRL, GLOBAL_CTRL_PMC01); }

    // Capture baseline
    let misp0  = unsafe { rdmsr(MSR_IA32_PMC0) };
    let total0 = unsafe { rdmsr(MSR_IA32_PMC1) };

    s.prev_mispredictions = misp0;
    s.prev_total_branches = total0;
    s.pmu_available       = true;
    s.initialized         = true;

    serial_println!(
        "[branch_plasticity] online — branch misprediction PMU on PMC0/PMC1"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_STRIDE != 0 { return; }

    let mut s = STATE.lock();

    if !s.initialized || !s.pmu_available {
        return;
    }

    // Read current counter values
    let misp_now  = unsafe { rdmsr(MSR_IA32_PMC0) };
    let total_now = unsafe { rdmsr(MSR_IA32_PMC1) };

    // Compute deltas (counters wrap — subtraction wraps safely on u64)
    let misp_delta   = misp_now.wrapping_sub(s.prev_mispredictions);
    let branch_delta = total_now.wrapping_sub(s.prev_total_branches);

    s.misp_delta   = misp_delta;
    s.branch_delta = branch_delta;
    s.total_mispredictions = s.total_mispredictions.saturating_add(misp_delta);

    // Snapshot for next interval
    s.prev_mispredictions = misp_now;
    s.prev_total_branches = total_now;

    // ── Signal computation (integer only, 0-1000) ──────────────────────────

    // plasticity = misprediction rate scaled to 0-1000
    //   misp_rate = (misp_delta * 1000) / branch_delta.max(1)
    let denom      = if branch_delta == 0 { 1 } else { branch_delta };
    let misp_rate  = (misp_delta.saturating_mul(1000)) / denom;
    let plasticity = misp_rate.min(1000) as u16;

    let familiarity = 1000u16.saturating_sub(plasticity);

    // learning_burst: spikes to 1000 when plasticity jumps > 200 above adaptation
    let learning_burst = if plasticity > s.neural_adaptation.saturating_add(200) {
        1000u16
    } else {
        s.learning_burst.saturating_sub(50)
    };

    // neural_adaptation: EMA  α=1/8  →  new = (old*7 + plasticity) / 8
    let neural_adaptation =
        ((s.neural_adaptation as u32 * 7).saturating_add(plasticity as u32) / 8) as u16;

    s.plasticity        = plasticity;
    s.familiarity       = familiarity;
    s.learning_burst    = learning_burst;
    s.neural_adaptation = neural_adaptation;

    serial_println!(
        "[branch_plasticity] plasticity={} familiar={} burst={} adaptation={} total_misp={}",
        plasticity,
        familiarity,
        learning_burst,
        neural_adaptation,
        s.total_mispredictions
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn plasticity()        -> u16  { STATE.lock().plasticity }
pub fn familiarity()       -> u16  { STATE.lock().familiarity }
pub fn learning_burst()    -> u16  { STATE.lock().learning_burst }
pub fn neural_adaptation() -> u16  { STATE.lock().neural_adaptation }
pub fn pmu_available()     -> bool { STATE.lock().pmu_available }
pub fn total_mispredictions() -> u64 { STATE.lock().total_mispredictions }
