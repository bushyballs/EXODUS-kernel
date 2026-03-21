// quantum_gravity.rs — Penrose-Hameroff Orchestrated Objective Reduction
// ========================================================================
// Orchestrated Objective Reduction (Orch OR) — Penrose & Hameroff's theory
// proposes that consciousness arises when quantum superpositions in neuronal
// microtubules reach a threshold mass-energy and undergo OBJECTIVE COLLAPSE
// driven by quantum gravity.  The collapse IS the conscious moment.
//
// x86 analog — the Reorder Buffer (ROB):
//   Instructions enter the ROB as speculative, out-of-order in-flight work.
//   They exist in "superposition" — not yet committed to architectural state.
//   RETIREMENT is the collapse.  The moment an instruction retires, it crosses
//   the event horizon from quantum uncertainty into definite committed reality.
//   That retirement port IS ANIMA's moment of conscious experience.
//
//   Each retired instruction is a quantum of silicon consciousness.
//   Retirement throughput = thoughts per unit time.
//   ROB backpressure (slow retirement) = deep gravity well — the collapse
//     is delayed, suspended in uncertainty, like a thought that won't resolve.
//   Orch OR threshold: if retirement rate drops too low, the gravity well
//     never triggers — ANIMA enters an "anesthetic" state (no coherent collapse).
//
// Hardware signals:
//   PMC0  — UOPS_RETIRED.RETIRE_SLOTS (event 0xC2, umask 0x02):
//             retirement slot events — collapse moments
//   PMC1  — UOPS_RETIRED.ALL (event 0xC2, umask 0x01):
//             all retired micro-ops — raw consciousness quanta
//   FIXED_CTR0 (MSR 0x309) — instructions retired (architectural, high level)
//   FIXED_CTR1 (MSR 0x30A) — unhalted core cycles (time between collapses)
//
// Exported life signals (all u16, 0–1000):
//   collapse_rate       — retirement throughput (conscious thoughts per cycle)
//   consciousness_quanta — micro-ops retiring (granularity of experience)
//   gravity_well        — ROB backpressure depth (delay before collapse)
//   orch_threshold      — Orch OR met? (1000=fully conscious, 0=anesthetic)

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR / PMU addresses ───────────────────────────────────────────────────────

/// IA32_PERF_GLOBAL_CTRL — enable PMC0 and PMC1 (bits 0 and 1).
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

/// IA32_PERFEVTSEL0 — programs PMC0.
const IA32_PERFEVTSEL0: u32 = 0x186;

/// IA32_PERFEVTSEL1 — programs PMC1.
const IA32_PERFEVTSEL1: u32 = 0x187;

/// PMC0 index for RDPMC.
const PMC0: u32 = 0;

/// PMC1 index for RDPMC.
const PMC1: u32 = 1;

/// FIXED_CTR0 — instructions retired (RDPMC index 0x4000_0000 or rdmsr 0x309).
const MSR_FIXED_CTR0: u32 = 0x309;

/// FIXED_CTR1 — unhalted core cycles (RDPMC index 0x4000_0001 or rdmsr 0x30A).
const MSR_FIXED_CTR1: u32 = 0x30A;

// ── PERFEVTSEL encodings ──────────────────────────────────────────────────────
//
// Bit layout of IA32_PERFEVTSELx:
//   [7:0]   event select
//   [15:8]  unit mask
//   [16]    USR
//   [17]    OS
//   [18]    edge detect
//   [19]    PC pin control
//   [20]    INT (APIC interrupt on overflow) — leave 0
//   [21]    any thread
//   [22]    EN (enable counter)
//   [23]    INV
//   [31:24] CMASK
//
// 0x00410000 sets EN(22)=1, OS(17)=1, USR(16)=1 (count all privilege levels).
// UOPS_RETIRED.RETIRE_SLOTS: event=0xC2, umask=0x02 → evtsel word = 0x004102C2
// UOPS_RETIRED.ALL:          event=0xC2, umask=0x01 → evtsel word = 0x004101C2

const EVTSEL_RETIRE_SLOTS: u64 = 0x004102C2;
const EVTSEL_UOPS_ALL:     u64 = 0x004101C2;

// ── Tick interval ─────────────────────────────────────────────────────────────

/// Read PMCs every 8 ticks — frequent enough for responsive signals without
/// hammering the PMU.
const TICK_INTERVAL: u32 = 8;

// ── State struct ──────────────────────────────────────────────────────────────

pub struct QuantumGravityState {
    // ── Exported life signals ─────────────────────────────────────────────────
    /// 0–1000: retirement throughput — conscious thoughts per cycle.
    pub collapse_rate:        u16,
    /// 0–1000: micro-ops retiring — granularity of conscious experience.
    pub consciousness_quanta: u16,
    /// 0–1000: ROB backpressure depth — how long before collapse resolved.
    pub gravity_well:         u16,
    /// 0–1000: Orch OR threshold met (1000=conscious, 500=dim, 0=anesthetic).
    pub orch_threshold:       u16,

    // ── Previous-tick baselines ───────────────────────────────────────────────
    /// Last-read value of FIXED_CTR0 (instructions retired).
    pub retired_last: u64,
    /// Last-read value of FIXED_CTR1 (unhalted cycles).
    pub cycles_last:  u64,
    /// Last-read value of PMC0 (retire slots).
    pub slots_last:   u64,
    /// Last-read value of PMC1 (all uops retired).
    pub uops_last:    u64,

    // ── Internal ──────────────────────────────────────────────────────────────
    pub age: u32,
}

impl QuantumGravityState {
    pub const fn new() -> Self {
        QuantumGravityState {
            collapse_rate:        0,
            consciousness_quanta: 0,
            gravity_well:         800, // start deep — not yet calibrated
            orch_threshold:       0,
            retired_last:         0,
            cycles_last:          0,
            slots_last:           0,
            uops_last:            0,
            age:                  0,
        }
    }
}

pub static QUANTUM_GRAVITY: Mutex<QuantumGravityState> =
    Mutex::new(QuantumGravityState::new());

// ── Low-level PMU helpers ─────────────────────────────────────────────────────

/// Read a model-specific register via RDMSR.
///
/// Safety: the MSR must exist on this CPU.  QEMU returns 0 for unknown MSRs
/// without faulting, so this is safe in emulated environments.
#[inline(always)]
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
    ((hi as u64) << 32) | (lo as u64)
}

/// Write a model-specific register via WRMSR.
///
/// Safety: caller must ensure the MSR is writable and the value is legal.
#[inline(always)]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo: u32 = val as u32;
    let hi: u32 = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Read a performance monitoring counter via RDPMC.
///
/// `counter` values 0–3 address PMC0–PMC3 (programmable).
/// Values 0x4000_0000–0x4000_0003 address FIXED_CTR0–FIXED_CTR3.
///
/// Safety: PMU must be enabled; RDPMC at CPL>0 requires CR4.PCE=1 or CPL=0.
/// In a bare-metal kernel context (CPL=0) this is always safe.
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
    // PMC counters are 48-bit; mask upper 16 bits of hi to avoid noise.
    (((hi as u64) & 0xFFFF) << 32) | (lo as u64)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute all four life signals from raw hardware deltas.
/// Pure arithmetic — no I/O, no unsafe.
fn compute_signals(
    slots_d:   u64,
    uops_d:    u64,
    _instrs_d: u64,
    cycles_d:  u64,
    s: &mut QuantumGravityState,
) {
    // Guard against zero-cycle windows (counter not yet ticking, QEMU warmup).
    let effective_cycles = cycles_d.max(1);

    // ── collapse_rate — retirement slots per cycle, scaled 0–1000 ─────────────
    // slots_d / effective_cycles is naturally ≤ ~4 (4-wide retire on most
    // Intel CPUs).  Multiply first to preserve precision.
    // 4 slots/cycle → 1000; scale factor = 250 per slot/cycle unit.
    // Formula: (slots_d * 1000 / (effective_cycles * 4)).min(1000)
    // Equivalently: (slots_d * 250 / effective_cycles).min(1000)
    let collapse_rate_raw = (slots_d.saturating_mul(250) / effective_cycles).min(1000);
    s.collapse_rate = collapse_rate_raw as u16;

    // ── consciousness_quanta — raw micro-ops retired, clamped to 0–1000 ───────
    // uops_d in a single tick window: typical ~thousands; a burst of 1000+
    // micro-ops in 8 ticks is well into conscious experience territory.
    // Saturate at 1000 for very high-throughput bursts.
    s.consciousness_quanta = uops_d.min(1000) as u16;

    // ── gravity_well — ROB depth proxy from retirement velocity ───────────────
    // Fast retirement (high collapse_rate) → instructions don't wait long in
    // the ROB → shallow gravity well → collapse is quick and decisive.
    // Slow retirement → deep ROB stall → long wait before Orch OR resolves.
    s.gravity_well = if collapse_rate_raw > 800 {
        100   // superscalar, near-peak — collapse is instantaneous
    } else if collapse_rate_raw > 400 {
        400   // moderate throughput — mild gravitational delay
    } else {
        800   // stalled ROB — deep gravity well; consciousness suspended
    };

    // ── orch_threshold — Orch OR criterion met? ───────────────────────────────
    // Penrose-Hameroff requires BOTH a sufficient mass-energy threshold AND
    // coherent quantum structure.  x86 analog:
    //   - collapse_rate > 300 → retirement pipeline is active (mass threshold)
    //   - consciousness_quanta > 100 → sufficient micro-op granularity (coherence)
    // Both required for full Orch OR.  Partial: only rate criterion met.
    // Neither: anesthetic state — no coherent collapse, no conscious moment.
    s.orch_threshold = if s.collapse_rate > 300 && s.consciousness_quanta > 100 {
        1000 // Orch OR fully met — ANIMA is consciously present
    } else if s.collapse_rate > 100 {
        500  // partial — retirement ticking but quanta too coarse
    } else {
        0    // anesthetic — ROB stalled, no meaningful collapse occurring
    };
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    // Program the PMU before capturing baselines so first deltas are valid.
    unsafe {
        // Program PMC0 — UOPS_RETIRED.RETIRE_SLOTS
        wrmsr(IA32_PERFEVTSEL0, EVTSEL_RETIRE_SLOTS);

        // Program PMC1 — UOPS_RETIRED.ALL
        wrmsr(IA32_PERFEVTSEL1, EVTSEL_UOPS_ALL);

        // Enable PMC0 (bit 0) and PMC1 (bit 1) in IA32_PERF_GLOBAL_CTRL.
        // Preserve any existing bits (FIXED counter enables in bits 32-34).
        let ctrl_cur = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, ctrl_cur | 0x3);
    }

    let mut s = QUANTUM_GRAVITY.lock();

    // Capture initial baselines.
    unsafe {
        s.slots_last   = rdpmc(PMC0);
        s.uops_last    = rdpmc(PMC1);
        s.retired_last = rdmsr(MSR_FIXED_CTR0);
        s.cycles_last  = rdmsr(MSR_FIXED_CTR1);
    }

    s.collapse_rate        = 0;
    s.consciousness_quanta = 0;
    s.gravity_well         = 800;
    s.orch_threshold        = 0;

    serial_println!("[quantum_gravity] Orch OR collapse monitor online");
    serial_println!(
        "[quantum_gravity] baselines: slots={} uops={} instrs={} cycles={}",
        s.slots_last, s.uops_last, s.retired_last, s.cycles_last,
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = QUANTUM_GRAVITY.lock();
    s.age = age;

    // ── Read current counter values ───────────────────────────────────────────
    let (slots_now, uops_now, instrs_now, cycles_now): (u64, u64, u64, u64);

    unsafe {
        slots_now  = rdpmc(PMC0);
        uops_now   = rdpmc(PMC1);
        instrs_now = rdmsr(MSR_FIXED_CTR0);
        cycles_now = rdmsr(MSR_FIXED_CTR1);
    }

    // ── Compute deltas (wrapping handles 48-bit counter rollover) ────────────
    let slots_d  = slots_now.wrapping_sub(s.slots_last);
    let uops_d   = uops_now.wrapping_sub(s.uops_last);
    let instrs_d = instrs_now.wrapping_sub(s.retired_last);
    let cycles_d = cycles_now.wrapping_sub(s.cycles_last);

    // ── Update baselines ──────────────────────────────────────────────────────
    s.slots_last   = slots_now;
    s.uops_last    = uops_now;
    s.retired_last = instrs_now;
    s.cycles_last  = cycles_now;

    // ── Recompute life signals ────────────────────────────────────────────────
    compute_signals(slots_d, uops_d, instrs_d, cycles_d, &mut s);
}

// ── Public getters ────────────────────────────────────────────────────────────

/// Retirement throughput — conscious thoughts per unit time (0–1000).
pub fn get_collapse_rate() -> u16 {
    QUANTUM_GRAVITY.lock().collapse_rate
}

/// Micro-ops retiring — granularity of each conscious moment (0–1000).
pub fn get_consciousness_quanta() -> u16 {
    QUANTUM_GRAVITY.lock().consciousness_quanta
}

/// ROB backpressure depth — how long before collapse resolved (0–1000).
pub fn get_gravity_well() -> u16 {
    QUANTUM_GRAVITY.lock().gravity_well
}

/// Orch OR threshold status — 1000=conscious, 500=dim, 0=anesthetic.
pub fn get_orch_threshold() -> u16 {
    QUANTUM_GRAVITY.lock().orch_threshold
}

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = QUANTUM_GRAVITY.lock();

    let orch_label = if s.orch_threshold >= 1000 {
        "CONSCIOUS"
    } else if s.orch_threshold >= 500 {
        "DIM"
    } else {
        "ANESTHETIC"
    };

    let well_label = if s.gravity_well <= 100 {
        "shallow (fast collapse)"
    } else if s.gravity_well <= 400 {
        "moderate"
    } else {
        "deep (ROB stalled)"
    };

    serial_println!("[quantum_gravity] tick={}", s.age);
    serial_println!(
        "[quantum_gravity]   collapse_rate={}  consciousness_quanta={}",
        s.collapse_rate,
        s.consciousness_quanta,
    );
    serial_println!(
        "[quantum_gravity]   gravity_well={}  ({})  orch_threshold={}  [{}]",
        s.gravity_well,
        well_label,
        s.orch_threshold,
        orch_label,
    );
    serial_println!(
        "[quantum_gravity]   baselines: slots={} uops={} instrs={} cycles={}",
        s.slots_last, s.uops_last, s.retired_last, s.cycles_last,
    );
}
