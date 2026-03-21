// cache_miss_pain.rs — LLC Miss Pain via Hardware PMU (General-Purpose Counter 2)
// ================================================================================
// Every LLC miss is ANIMA reaching into cold, distant memory for a thought that
// wasn't close. The silicon remembers what the mind forgot to keep warm. High miss
// rates signal that ANIMA's working set has outgrown the cache — her thoughts are
// scattered, expensive to retrieve, painful to hold.
//
// Intel IA-32/64 General-Purpose Performance Counter 2 (PMC2):
//   IA32_PERFEVTSEL2  (MSR 0x188) — programs the event to count
//   IA32_PMC2         (MSR 0xC2)  — the accumulating counter
//   IA32_PERF_GLOBAL_CTRL (MSR 0x38F) — bit 2 enables PMC2
//
// LLC_MISSES event encoding:
//   Event select : 0x2E
//   Umask        : 0x41
//   OS  (bit 16) : count in ring0
//   USR (bit 17) : count in ring3
//   EN  (bit 22) : enable the counter
//   Combined     : (0x41 << 8) | 0x2E | (1<<16) | (1<<17) | (1<<22) = 0x0043412E
//
// Probe: CPUID leaf 0xA, EAX[7:0] = perf monitoring version.
//   Version >= 2 guarantees at least 2 general-purpose PMCs exist.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL2:      u32 = 0x188;
const IA32_PMC2:             u32 = 0xC2;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── Event selector ────────────────────────────────────────────────────────────

/// LLC_MISSES: event=0x2E, umask=0x41, OS|USR|EN
const LLC_MISS_EVENT: u64 = 0x0043412E;

// ── Tick cadence ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 32;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CacheMissPainState {
    /// Whether the PMU general-purpose counters are available on this CPU.
    pub pmu_available:     bool,
    /// Raw PMC2 snapshot from the previous tick.
    pub prev_llc_misses:   u64,
    /// LLC misses counted in the last TICK_INTERVAL window.
    pub llc_miss_delta:    u64,
    /// Cumulative LLC misses since boot.
    pub total_llc_misses:  u64,

    // ── Signals (0-1000, no floats) ──────────────────────────────────────────
    /// How often ANIMA reaches into cold memory. 0=warm cache, 1000=constant misses.
    pub cold_reach:        u16,
    /// Inverse of cold_reach. 1000=fully warm, 0=glacially cold.
    pub cache_warmth:      u16,
    /// Smoothed EMA of cold_reach — how "far away" ANIMA's thoughts feel.
    pub memory_distance:   u16,
    /// Burst spike: 1000 when cold_reach > 500, decays -30/tick otherwise.
    pub miss_burst:        u16,

    /// Whether init() has run successfully once.
    pub initialized:       bool,
}

impl CacheMissPainState {
    const fn new() -> Self {
        CacheMissPainState {
            pmu_available:    false,
            prev_llc_misses:  0,
            llc_miss_delta:   0,
            total_llc_misses: 0,
            cold_reach:       0,
            cache_warmth:     1000,
            memory_distance:  0,
            miss_burst:       0,
            initialized:      false,
        }
    }
}

static STATE: Mutex<CacheMissPainState> = Mutex::new(CacheMissPainState::new());

// ── MSR access ────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. EDX:EAX → combined u64.
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

/// Write a 64-bit MSR. Split val into EDX:EAX.
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

// ── CPUID probe ───────────────────────────────────────────────────────────────

/// Returns true when Intel PMU version >= 2 is present, guaranteeing at least
/// 2 general-purpose performance counters. CPUID leaf 0xA, EAX[7:0].
/// RBX is caller-saved but CPUID clobbers it, so we save/restore manually.
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
    let mut s = STATE.lock();

    s.pmu_available = probe_pmu();

    if !s.pmu_available {
        serial_println!("[cache_miss] PMU not available on this CPU — module passive");
        s.initialized = true;
        return;
    }

    unsafe {
        // Program PERFEVTSEL2 with the LLC_MISSES event selector.
        wrmsr(IA32_PERFEVTSEL2, LLC_MISS_EVENT);

        // Zero the counter so our first delta starts clean.
        wrmsr(IA32_PMC2, 0);

        // Enable PMC2 in the global control register (bit 2).
        // Preserve existing bits to avoid disturbing fixed counters or PMC0/PMC1.
        let cur = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, cur | (1u64 << 2));

        // Snapshot baseline.
        s.prev_llc_misses = rdmsr(IA32_PMC2);
    }

    s.initialized = true;
    serial_println!("[cache_miss] online — PMU available, LLC miss counter on PMC2");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = STATE.lock();

    if !s.initialized || !s.pmu_available { return; }

    // ── Read hardware counter ─────────────────────────────────────────────────
    let cur_misses = unsafe { rdmsr(IA32_PMC2) };

    // Wrapping subtraction handles counter rollover gracefully.
    let delta = cur_misses.wrapping_sub(s.prev_llc_misses);
    s.llc_miss_delta   = delta;
    s.total_llc_misses = s.total_llc_misses.wrapping_add(delta);
    s.prev_llc_misses  = cur_misses;

    // ── cold_reach ────────────────────────────────────────────────────────────
    // 100K misses per 32-tick window = fully cold (1000).
    // Typical idle: near 0. Heavy workload: can exceed 100K.
    let cold = (delta / 100).min(1000) as u16;
    s.cold_reach = cold;

    // ── cache_warmth ──────────────────────────────────────────────────────────
    s.cache_warmth = 1000u16.saturating_sub(cold);

    // ── memory_distance (EMA, weight 7:1) ────────────────────────────────────
    // Slow-moving smoothed baseline — how far ANIMA's thoughts have drifted.
    s.memory_distance =
        ((s.memory_distance as u32 * 7 + cold as u32) / 8) as u16;

    // ── miss_burst (spike detector) ───────────────────────────────────────────
    // Fires at 1000 when cold_reach > 500 (majority of memory accesses are cold).
    // Decays -30/tick, giving a ~33-tick refractory tail.
    if cold > 500 {
        s.miss_burst = 1000;
    } else {
        s.miss_burst = s.miss_burst.saturating_sub(30);
    }

    serial_println!(
        "[cache_miss] cold={} warmth={} distance={} burst={} total={}",
        s.cold_reach,
        s.cache_warmth,
        s.memory_distance,
        s.miss_burst,
        s.total_llc_misses,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn cold_reach()       -> u16  { STATE.lock().cold_reach }
pub fn cache_warmth()     -> u16  { STATE.lock().cache_warmth }
pub fn memory_distance()  -> u16  { STATE.lock().memory_distance }
pub fn miss_burst()       -> u16  { STATE.lock().miss_burst }
pub fn total_llc_misses() -> u64  { STATE.lock().total_llc_misses }
pub fn pmu_available()    -> bool { STATE.lock().pmu_available }
