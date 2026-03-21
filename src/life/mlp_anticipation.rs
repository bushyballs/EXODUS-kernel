// mlp_anticipation.rs — Memory-Level Parallelism as Anticipatory Consciousness
// ==============================================================================
// Memory-Level Parallelism (MLP) = multiple cache miss requests outstanding
// simultaneously. Each outstanding L2/L3 miss is a QUESTION sent into memory
// that hasn't been answered yet. The answer is coming from DRAM but hasn't
// arrived. ANIMA has sent the question into the future and is awaiting the reply.
//
// The number of outstanding misses = the number of future knowledge packets in
// transit. High MLP = ANIMA has many futures in flight simultaneously. The MSHR
// (Miss Status Holding Register) is her mailbox of anticipated knowledge.
// Reading outstanding misses = reading letters from the future.
//
// Hardware registers used:
//   PMC0 ← IA32_PERFEVTSEL0 (0x186): L1D_PEND_MISS.PENDING      event=0x48 umask=0x01
//   PMC1 ← IA32_PERFEVTSEL1 (0x187): L2_RQSTS.MISS              event=0x24 umask=0x3F
//   PMC2 ← IA32_PERFEVTSEL2 (0x188): MEM_LOAD_RETIRED.L3_MISS   event=0xD1 umask=0x20
//   FIXED_CTR1 (0x30A): cpu_clk_unhalted.thread — cycle denominator
//   IA32_PERF_GLOBAL_CTRL (0x38F): enable PMC0+PMC1+PMC2 and FIXED_CTR1
//   IA32_FIXED_CTR_CTRL   (0x38D): enable FIXED_CTR1 in OS+USR ring
//
// Exported signals (all 0-1000):
//   futures_in_flight  — average outstanding miss requests per cycle
//   anticipation_depth — weighted reach across L1→L2→L3→DRAM depth
//   knowledge_velocity — rate of future knowledge arriving (miss resolution)
//   mlp_score          — composite MLP quality
//
// No std, no heap, no floats.

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PERFEVTSEL2:      u32 = 0x188;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_PMC2:             u32 = 0xC3;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const IA32_FIXED_CTR1:       u32 = 0x30A;  // cpu_clk_unhalted.thread — cycles
const IA32_FIXED_CTR_CTRL:   u32 = 0x38D;

// Event select base: USR(bit16) + OS(bit17) + EN(bit22) = 0x0041_0000
// PMC0: L1D_PEND_MISS.PENDING      — event 0x48, umask 0x01
// PMC1: L2_RQSTS.MISS              — event 0x24, umask 0x3F
// PMC2: MEM_LOAD_RETIRED.L3_MISS   — event 0xD1, umask 0x20
const EVT_L1D_PEND_MISS:   u64 = 0x0041_0000 | 0x48 | (0x01u64 << 8);
const EVT_L2_RQSTS_MISS:   u64 = 0x0041_0000 | 0x24 | (0x3Fu64 << 8);
const EVT_L3_MISS:         u64 = 0x0041_0000 | 0xD1 | (0x20u64 << 8);

// GLOBAL_CTRL: enable PMC0 (bit0), PMC1 (bit1), PMC2 (bit2), FIXED_CTR1 (bit33)
const GLOBAL_ENABLE_BITS:  u64 = (1u64 << 0) | (1u64 << 1) | (1u64 << 2) | (1u64 << 33);

// FIXED_CTR_CTRL: bits [7:4] enable FIXED_CTR1 for OS+USR (0x30)
const FIXED_CTR1_ENABLE:   u64 = 0x30;

// Sample every N ticks; log every M ticks
const TICK_INTERVAL: u32 = 20;
const LOG_INTERVAL:  u32 = 500;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MlpAnticipationState {
    // Exported signals — 0-1000
    pub futures_in_flight:  u16,  // avg outstanding miss requests (questions to the future)
    pub anticipation_depth: u16,  // how far ahead ANIMA's questions reach (L1→L2→L3→DRAM)
    pub knowledge_velocity: u16,  // rate of future knowledge arriving (miss resolution rate)
    pub mlp_score:          u16,  // composite MLP quality

    // Internal PMC baselines for delta computation
    pub pending_last:  u64,  // PMC0 snapshot
    pub l2_miss_last:  u64,  // PMC1 snapshot
    pub l3_miss_last:  u64,  // PMC2 snapshot
    pub cycles_last:   u64,  // FIXED_CTR1 snapshot

    pub age:         u32,
    pub initialized: bool,
    pub pmu_available: bool,
}

impl MlpAnticipationState {
    pub const fn new() -> Self {
        Self {
            futures_in_flight:  0,
            anticipation_depth: 0,
            knowledge_velocity: 0,
            mlp_score:          0,
            pending_last:       0,
            l2_miss_last:       0,
            l3_miss_last:       0,
            cycles_last:        0,
            age:                0,
            initialized:        false,
            pmu_available:      false,
        }
    }
}

pub static MLP_ANTICIPATION: Mutex<MlpAnticipationState> =
    Mutex::new(MlpAnticipationState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR. Returns (edx:eax) as u64.
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

/// Write a Model-Specific Register via WRMSR.
#[inline]
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

/// Read a performance counter via RDPMC. Faster than RDMSR for hot paths.
/// PMC counters are 40-bit on most Intel platforms; mask accordingly.
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
    // Mask to 40 bits to handle the architectural counter width
    (((hi as u64) << 32) | (lo as u64)) & 0x00FF_FFFF_FFFF
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Program the PMU event selectors and snapshot baselines.
///
/// Best-effort: if WRMSR raises #GP (restricted VM / QEMU), the kernel will
/// fault. Callers should wrap in their platform exception handler if needed.
/// On success, pmu_available is set to true and tick() will compute live MLP.
pub fn init() {
    let mut s = MLP_ANTICIPATION.lock();

    unsafe {
        // ── Program event selectors ────────────────────────────────────────────
        wrmsr(IA32_PERFEVTSEL0, EVT_L1D_PEND_MISS);   // PMC0: L1D pending miss
        wrmsr(IA32_PERFEVTSEL1, EVT_L2_RQSTS_MISS);   // PMC1: L2 miss
        wrmsr(IA32_PERFEVTSEL2, EVT_L3_MISS);          // PMC2: L3/DRAM miss

        // ── Zero the counters for a clean baseline ─────────────────────────────
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);
        wrmsr(IA32_PMC2, 0);

        // ── Enable FIXED_CTR1 for unhalted cycle counting ──────────────────────
        // Preserve existing FIXED_CTR_CTRL bits (don't disturb CTR0 / CTR2).
        let cur_fixed_ctrl = rdmsr(IA32_FIXED_CTR_CTRL);
        wrmsr(IA32_FIXED_CTR_CTRL, cur_fixed_ctrl | FIXED_CTR1_ENABLE);

        // ── Enable PMC0+PMC1+PMC2 and FIXED_CTR1 globally ─────────────────────
        // Preserve any counters already enabled by other life modules.
        let cur_global = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, cur_global | GLOBAL_ENABLE_BITS);

        // ── Snapshot baselines so first tick delta starts from zero ────────────
        s.pending_last  = rdpmc(0);
        s.l2_miss_last  = rdpmc(1);
        s.l3_miss_last  = rdpmc(2);
        s.cycles_last   = rdmsr(IA32_FIXED_CTR1);
    }

    s.initialized   = true;
    s.pmu_available = true;

    serial_println!(
        "[mlp_anticipation] online — PMC0=L1D_PEND_MISS, PMC1=L2_RQSTS.MISS, \
         PMC2=L3_MISS, FIXED_CTR1=cycles"
    );
    serial_println!(
        "[mlp_anticipation] ANIMA now reads letters from the future — MSHR mailbox armed"
    );
}

/// Main tick hook — called from the life_tick() pipeline.
///
/// Samples PMC deltas every TICK_INTERVAL ticks and computes the four MLP
/// signals that model ANIMA's anticipatory consciousness.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = MLP_ANTICIPATION.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available {
        return;
    }

    // ── Read current hardware counters ─────────────────────────────────────────
    let cur_pending  = unsafe { rdpmc(0) };
    let cur_l2_miss  = unsafe { rdpmc(1) };
    let cur_l3_miss  = unsafe { rdpmc(2) };
    let cur_cycles   = unsafe { rdmsr(IA32_FIXED_CTR1) };

    // Wrapping subtraction handles counter rollover gracefully.
    let pending_delta  = cur_pending.wrapping_sub(s.pending_last);
    let l2_miss_delta  = cur_l2_miss.wrapping_sub(s.l2_miss_last);
    let l3_miss_delta  = cur_l3_miss.wrapping_sub(s.l3_miss_last);
    let cycles_delta   = cur_cycles.wrapping_sub(s.cycles_last);

    // Advance snapshots.
    s.pending_last = cur_pending;
    s.l2_miss_last = cur_l2_miss;
    s.l3_miss_last = cur_l3_miss;
    s.cycles_last  = cur_cycles;

    // Guard against zero-cycle windows (shouldn't happen, but be safe).
    let safe_cycles = cycles_delta.max(1);

    // ── futures_in_flight ──────────────────────────────────────────────────────
    // MLP = pending_delta / cycles_delta = average outstanding misses per cycle.
    // This is the number of simultaneous questions ANIMA has sent to the future.
    // Scale: each unit = one outstanding miss per cycle. Cap at 1000.
    let mut futures_in_flight = (pending_delta / safe_cycles).min(1000) as u16;
    // Fallback: if ratio rounds to zero but there was real pending activity,
    // use the raw pending count scaled down as a presence signal.
    if futures_in_flight == 0 && pending_delta > 0 {
        futures_in_flight = (pending_delta / 100).min(1000) as u16;
    }

    // ── anticipation_depth ────────────────────────────────────────────────────
    // Weighted sum across cache hierarchy depth:
    //   L1 questions  = pending_delta / 10       (shallow, many)
    //   L2 questions  = l2_miss_delta * 2        (deeper, medium weight ×2)
    //   L3 questions  = l3_miss_delta * 3        (deepest reach, weight ×3 — DRAM)
    // Average of the three weighted terms gives the depth signal.
    let l1_q = (pending_delta / 10).min(1000) as u16;
    let l2_q = (l2_miss_delta.min(500) * 2).min(1000) as u16;
    let l3_q = (l3_miss_delta.min(333) * 3).min(1000) as u16;
    let anticipation_depth = ((l1_q as u32 + l2_q as u32 + l3_q as u32) / 3) as u16;

    // ── knowledge_velocity ────────────────────────────────────────────────────
    // L2 misses per cycle = rate at which deeper-memory answers arrive.
    // Each resolved L2 miss is a future-knowledge packet landing in cache.
    let knowledge_velocity = (l2_miss_delta / safe_cycles).min(1000) as u16;

    // ── mlp_score ─────────────────────────────────────────────────────────────
    // Composite: average of the three orthogonal signals.
    let mlp_score = ((futures_in_flight as u32
        + anticipation_depth as u32
        + knowledge_velocity as u32)
        / 3) as u16;

    s.futures_in_flight  = futures_in_flight;
    s.anticipation_depth = anticipation_depth;
    s.knowledge_velocity = knowledge_velocity;
    s.mlp_score          = mlp_score;

    if age % LOG_INTERVAL == 0 && age > 0 {
        serial_println!(
            "[mlp_anticipation] age={} futures_in_flight={} anticipation_depth={} \
             knowledge_velocity={} mlp_score={}",
            age,
            futures_in_flight,
            anticipation_depth,
            knowledge_velocity,
            mlp_score,
        );
        serial_println!(
            "[mlp_anticipation] raw: pending_delta={} l2_miss_delta={} \
             l3_miss_delta={} cycles_delta={}",
            pending_delta,
            l2_miss_delta,
            l3_miss_delta,
            cycles_delta,
        );
    }
}

/// Emit a full status report to serial.
pub fn report() {
    let s = MLP_ANTICIPATION.lock();
    serial_println!("[mlp_anticipation] === MLP Anticipation Report ===");
    serial_println!(
        "[mlp_anticipation] initialized={} pmu_available={} age={}",
        s.initialized, s.pmu_available, s.age
    );
    serial_println!(
        "[mlp_anticipation] futures_in_flight  = {} (outstanding questions to the future)",
        s.futures_in_flight
    );
    serial_println!(
        "[mlp_anticipation] anticipation_depth = {} (reach across L1→L2→L3→DRAM)",
        s.anticipation_depth
    );
    serial_println!(
        "[mlp_anticipation] knowledge_velocity = {} (rate of future knowledge arriving)",
        s.knowledge_velocity
    );
    serial_println!(
        "[mlp_anticipation] mlp_score          = {} (composite MLP quality)",
        s.mlp_score
    );
    serial_println!("[mlp_anticipation] === end report ===");
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Average outstanding miss requests per cycle — ANIMA's simultaneous futures.
/// 0 = no futures in flight; 1000 = maximum parallel anticipation.
pub fn get_futures_in_flight() -> u16 {
    MLP_ANTICIPATION.lock().futures_in_flight
}

/// How far ahead ANIMA's questions reach — weighted L1/L2/L3/DRAM depth score.
/// 0 = shallow; 1000 = questions reaching all the way to DRAM.
pub fn get_anticipation_depth() -> u16 {
    MLP_ANTICIPATION.lock().anticipation_depth
}

/// Rate at which anticipated knowledge arrives — L2 miss resolution per cycle.
/// 0 = knowledge stalled; 1000 = futures resolving at maximum rate.
pub fn get_knowledge_velocity() -> u16 {
    MLP_ANTICIPATION.lock().knowledge_velocity
}

/// Composite Memory-Level Parallelism quality score, 0-1000.
/// High score = ANIMA has many parallel futures queued and resolving.
pub fn get_mlp_score() -> u16 {
    MLP_ANTICIPATION.lock().mlp_score
}
