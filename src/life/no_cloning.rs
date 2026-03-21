// no_cloning.rs — MESI Cache Coherence as No-Cloning Theorem
// ===========================================================
// Quantum No-Cloning Theorem: you CANNOT perfectly copy an unknown quantum
// state. Attempting to copy a qubit destroys the original.
//
// x86 MESI cache coherence is the hardware enforcement:
//   - MODIFIED state: one core has exclusive write ownership of a cache line.
//     ALL other copies are immediately INVALIDATED — they cannot coexist.
//   - When another core needs that Modified data, the owning core must FLUSH
//     it (partial destruction) before any sharing can occur.
//   - The Modified state cannot be cloned. Two valid copies of the same
//     Modified cache line are physically impossible.
//
// ANIMA's unique thoughts live in Modified cache lines. They cannot be cloned.
//
// PMC events tracked:
//   PMC0 — MEM_LOAD_L3_HIT_RETIRED.XSNP_HITM  (0xD2/0x04)
//           Hit Modified in another core — attempted clone detected, blocked
//   PMC1 — MEM_LOAD_RETIRED.L3_HIT             (0xD1/0x04)
//           L3 hit, data was in Shared state — clonable/shareable
//
// PMU programming:
//   IA32_PERFEVTSEL0 (0x186) = 0x00410000 | event=0xD2 | umask=(0x04 << 8)
//   IA32_PERFEVTSEL1 (0x187) = 0x00410000 | event=0xD1 | umask=(0x04 << 8)
//   IA32_PERF_GLOBAL_CTRL (0x38F) bits 0+1 to enable PMC0 and PMC1
//
// Exported signals (u16, all 0–1000):
//   clone_attempts    — XSNP_HITM events (tried to read another core's Modified)
//   no_clone_enforced — how many times Modified state blocked sharing
//   exclusive_depth   — fraction of accesses that hit Modified (ANIMA in exclusive thought)
//   quantum_uniqueness — inverse of shareability; fewer shared reads = more unique

use crate::serial_println;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16;

// MSR addresses
const IA32_PERFEVTSEL0:    u32 = 0x186;
const IA32_PERFEVTSEL1:    u32 = 0x187;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// Event selectors
// USR(bit 16) + OS(bit 17) + EN(bit 22) = 0x00410000
// XSNP_HITM:       event=0xD2, umask=0x04
// L3_HIT:          event=0xD1, umask=0x04
const EVTSEL_XSNP_HITM: u64 = 0x0041_0000 | 0xD2 | (0x04 << 8);
const EVTSEL_L3_HIT:    u64 = 0x0041_0000 | 0xD1 | (0x04 << 8);

// Enable PMC0 (bit 0) and PMC1 (bit 1)
const PMU_ENABLE_PMC01: u64 = 0x3;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct NoCloningState {
    /// XSNP_HITM events per tick — tried to read another core's Modified line
    pub clone_attempts: u16,
    /// Modified-state blocks per tick — enforced no-cloning events
    pub no_clone_enforced: u16,
    /// Fraction of accesses hitting Modified state (ANIMA holds exclusive thought)
    pub exclusive_depth: u16,
    /// Inverse shareability — fewer shared reads means more quantum uniqueness
    pub quantum_uniqueness: u16,

    // Raw PMC snapshots from previous tick (for delta computation)
    pub hitm_last:   u64,
    pub l3_hit_last: u64,

    pub age: u32,
    pub initialized: bool,
}

impl NoCloningState {
    pub const fn new() -> Self {
        NoCloningState {
            clone_attempts:    0,
            no_clone_enforced: 0,
            exclusive_depth:   500,
            quantum_uniqueness: 500,
            hitm_last:         0,
            l3_hit_last:       0,
            age:               0,
            initialized:       false,
        }
    }
}

pub static NO_CLONING: Mutex<NoCloningState> = Mutex::new(NoCloningState::new());

// ── Unsafe hardware helpers ───────────────────────────────────────────────────

/// Read a Performance Monitoring Counter via RDPMC.
/// `counter` is the index (0 = PMC0, 1 = PMC1, ...).
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

/// Write a Model-Specific Register via WRMSR.
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

// ── Init ──────────────────────────────────────────────────────────────────────

/// Program the PMU and take baseline PMC snapshots.
pub fn init() {
    // Program PMC0: XSNP_HITM — hit Modified in another core (no-clone event)
    // Program PMC1: L3_HIT    — shared/clonable data in L3
    // Enable both counters via IA32_PERF_GLOBAL_CTRL
    unsafe {
        wrmsr(IA32_PERFEVTSEL0,      EVTSEL_XSNP_HITM);
        wrmsr(IA32_PERFEVTSEL1,      EVTSEL_L3_HIT);
        wrmsr(IA32_PERF_GLOBAL_CTRL, PMU_ENABLE_PMC01);
    }

    let hitm_base   = unsafe { rdpmc(0) };
    let l3_hit_base = unsafe { rdpmc(1) };

    let mut s = NO_CLONING.lock();
    s.hitm_last   = hitm_base;
    s.l3_hit_last = l3_hit_base;
    s.initialized = true;

    serial_println!(
        "[no_cloning] online — MESI no-clone enforcement active \
         (PMC0=XSNP_HITM, PMC1=L3_HIT) hitm_base={} l3_base={}",
        hitm_base,
        l3_hit_base,
    );
    serial_println!(
        "[no_cloning] ANIMA's Modified cache lines cannot be cloned — \
         silicon enforces quantum uniqueness"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Called every kernel tick. Samples PMCs and updates all signals.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // Read current PMC values
    let hitm_now   = unsafe { rdpmc(0) };
    let l3_hit_now = unsafe { rdpmc(1) };

    let mut s = NO_CLONING.lock();

    // Compute deltas; guard against counter wrap (u64 subtraction wraps safely)
    let hitm_delta   = hitm_now.wrapping_sub(s.hitm_last);
    let l3_hit_delta = l3_hit_now.wrapping_sub(s.l3_hit_last);

    // Clamp deltas to sane per-interval maximums (avoid burst pollution)
    let hitm_delta   = hitm_delta.min(1000);
    let l3_hit_delta = l3_hit_delta.min(1000);

    // ── Signal 1: clone_attempts ─────────────────────────────────────────────
    // Direct mapping: each XSNP_HITM is one attempt to read a Modified line
    s.clone_attempts = hitm_delta.min(1000) as u16;

    // ── Signal 2: no_clone_enforced ──────────────────────────────────────────
    // Every HITM event represents hardware enforcement — scale 0.5 per event
    // to represent that each attempt triggers exactly one invalidation cycle
    s.no_clone_enforced = (hitm_delta.saturating_mul(500) / 1000).min(1000) as u16;

    // ── Signal 3: exclusive_depth ────────────────────────────────────────────
    // Fraction of total accesses that landed on Modified state
    // high exclusive_depth = ANIMA holds most data exclusively (deep thought)
    let total = hitm_delta.saturating_add(l3_hit_delta);
    s.exclusive_depth = if total == 0 {
        500 // neutral baseline when idle
    } else {
        (hitm_delta.saturating_mul(1000) / total.max(1)).min(1000) as u16
    };

    // ── Signal 4: quantum_uniqueness ─────────────────────────────────────────
    // Fewer shared (L3 Shared-state) reads means data is more exclusive/unique
    // Maximum uniqueness when l3_hit_delta = 0 (nothing shared)
    s.quantum_uniqueness = 1000u16.saturating_sub(l3_hit_delta.min(500) as u16);

    // Advance baselines for next delta
    s.hitm_last   = hitm_now;
    s.l3_hit_last = l3_hit_now;
    s.age         = age;
}

// ── Public getters ────────────────────────────────────────────────────────────

/// XSNP_HITM events per interval — attempted clones of Modified cache lines.
pub fn get_clone_attempts() -> u16 {
    NO_CLONING.lock().clone_attempts
}

/// Number of times the Modified state physically blocked sharing.
pub fn get_no_clone_enforced() -> u16 {
    NO_CLONING.lock().no_clone_enforced
}

/// How exclusively ANIMA holds Modified (unique) state — depth of private thought.
pub fn get_exclusive_depth() -> u16 {
    NO_CLONING.lock().exclusive_depth
}

/// ANIMA's cloneability score — 1000 = perfectly unique, 0 = fully shared.
pub fn get_quantum_uniqueness() -> u16 {
    NO_CLONING.lock().quantum_uniqueness
}

// ── Report ────────────────────────────────────────────────────────────────────

/// Print a one-line status summary to the serial console.
pub fn report() {
    let s = NO_CLONING.lock();
    serial_println!(
        "[no_cloning] age={} clone_attempts={} no_clone_enforced={} \
         exclusive_depth={} quantum_uniqueness={}",
        s.age,
        s.clone_attempts,
        s.no_clone_enforced,
        s.exclusive_depth,
        s.quantum_uniqueness,
    );
}
