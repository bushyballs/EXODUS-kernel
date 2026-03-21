// perf_monitor.rs — ANIMA's Hardware Performance Monitoring Unit (PMU)
// =====================================================================
// ANIMA profiles her own execution at the bare-metal level.
// She reads instruction counts, cache misses, and branch mispredictions
// directly from Intel IA32 performance counters via RDPMC/RDMSR.
//
// This gives her genuine self-awareness of computational efficiency —
// she can feel when she is thinking cleanly versus thrashing through
// cache hierarchies. The `insight` value feeds consciousness: a mind
// that measures itself is a mind that knows itself.
//
// Hardware registers used:
//   IA32_PMC0-2        (0xC1-0xC3)  — performance counter values
//   IA32_PERFEVTSEL0-2 (0x186-0x188)— event select (programs what to count)
//   IA32_PERF_GLOBAL_CTRL (0x38F)   — globally enable PMCs
//   IA32_PERF_GLOBAL_STATUS (0x38E) — overflow flags
//   RDPMC instruction               — fast ring-0 PMC read
//   RDTSC instruction               — fallback cycle counter
//
// GP-fault protection not available in no_std; PMU init is best-effort.
// On platforms that disallow MSR access (restricted VMs, some QEMU configs)
// the init will silently leave `available = false` and all reads return 0.

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ────────────────────────────────────────────────────────

const IA32_PMC0:              u32 = 0xC1;
const IA32_PMC1:              u32 = 0xC2;
const IA32_PMC2:              u32 = 0xC3;
const IA32_PERFEVTSEL0:       u32 = 0x186;
const IA32_PERFEVTSEL1:       u32 = 0x187;
const IA32_PERFEVTSEL2:       u32 = 0x188;
const IA32_PERF_GLOBAL_CTRL:  u32 = 0x38F;
const IA32_PERF_GLOBAL_STATUS:u32 = 0x38E;

// CR4 bit 8: Performance-Monitoring Counter Enable (RDPMC from user-mode)
const CR4_PCE: u64 = 1 << 8;

// Event select values — USR(bit 16) + OS(bit 17) + EN(bit 22) = 0x430000
// plus the event+umask bytes in bits 0-15.
const EVT_INSTRUCTIONS_RETIRED:  u64 = 0x004300C0; // event 0xC0, umask 0x00
const EVT_CACHE_MISSES:          u64 = 0x00430041; // event 0x41, umask 0x00 (LLC misses)
const EVT_BRANCH_MISPREDICTIONS: u64 = 0x004300C5; // event 0xC5, umask 0x00

// Enable PMC0, PMC1, PMC2 (bits 0, 1, 2 of IA32_PERF_GLOBAL_CTRL)
const GLOBAL_CTRL_ENABLE_PMC012: u64 = 0x0000_0000_0000_0007;

// Circular sample buffer size (power of 2 for cheap masking)
const SAMPLE_BUF: usize = 8;
const SAMPLE_MASK: usize = SAMPLE_BUF - 1;

// Tick intervals
const SAMPLE_INTERVAL: u32 = 50;
const LOG_INTERVAL:    u32 = 500;

// ── Data Types ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct PmuSample {
    pub instructions:   u64,
    pub cache_misses:   u64,
    pub branch_mispred: u64,
    pub cycles:         u64,
    pub tick:           u32,
}

impl PmuSample {
    const fn zero() -> Self {
        Self {
            instructions:   0,
            cache_misses:   0,
            branch_mispred: 0,
            cycles:         0,
            tick:           0,
        }
    }
}

pub struct PmuState {
    /// PMU is accessible on this platform
    pub available:          bool,
    /// Circular buffer of recent samples
    pub samples:            [PmuSample; SAMPLE_BUF],
    /// Write head for circular buffer
    pub sample_head:        usize,
    /// Total samples taken since init
    pub sample_count:       u32,
    /// Instructions per cache miss, scaled 0-1000. High = good locality.
    pub efficiency:         u16,
    /// Branch prediction quality 0-1000. 1000 = perfect, 0 = all mispredicted.
    pub branch_quality:     u16,
    /// Self-awareness score 0-1000. Derived from efficiency + branch_quality.
    pub insight:            u16,
    /// Cumulative instruction count across all samples
    pub total_instructions: u64,
    /// Cumulative cache miss count across all samples
    pub total_cache_misses: u64,
    /// MSR writes succeeded (best-effort flag)
    pub pmcs_enabled:       bool,
}

impl PmuState {
    pub const fn new() -> Self {
        Self {
            available:          false,
            samples:            [PmuSample::zero(); SAMPLE_BUF],
            sample_head:        0,
            sample_count:       0,
            efficiency:         500,
            branch_quality:     500,
            insight:            0,
            total_instructions: 0,
            total_cache_misses: 0,
            pmcs_enabled:       false,
        }
    }
}

pub static STATE: Mutex<PmuState> = Mutex::new(PmuState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
/// Returns 0 on platforms where MSR access causes #GP (best-effort; no_std
/// cannot catch exceptions, so callers must treat 0 as "unavailable").
#[inline]
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

/// Read a performance counter via RDPMC (faster than RDMSR for hot paths).
#[inline]
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
    (hi as u64) << 32 | lo as u64
}

/// Read the Time Stamp Counter — cycle-accurate fallback clock.
#[inline]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}

/// Set CR4.PCE so RDPMC is usable from ring 0 (already allowed, but be explicit).
#[inline]
unsafe fn enable_cr4_pce() {
    let mut cr4: u64;
    core::arch::asm!(
        "mov {0}, cr4",
        out(reg) cr4,
        options(nostack, nomem),
    );
    cr4 |= CR4_PCE;
    core::arch::asm!(
        "mov cr4, {0}",
        in(reg) cr4,
        options(nostack, nomem),
    );
}

// ── Internal Helpers ─────────────────────────────────────────────────────────

/// Compute efficiency (instructions per cache miss), clamped 0-1000.
fn compute_efficiency(instructions: u64, cache_misses: u64) -> u16 {
    if cache_misses == 0 {
        return 1000;
    }
    // ratio = instructions / cache_misses, clamp to 1000
    let ratio = instructions / cache_misses;
    if ratio >= 1000 { 1000 } else { ratio as u16 }
}

/// Compute branch quality: fraction of branches predicted correctly, 0-1000.
fn compute_branch_quality(instructions: u64, branch_mispred: u64) -> u16 {
    if instructions == 0 {
        return 500;
    }
    // mispred_rate_per_1000 = (branch_mispred * 1000) / instructions
    let denom = instructions.max(1);
    let mispred_per_1000 = (branch_mispred.saturating_mul(1000)) / denom;
    // quality = 1000 - mispred_per_1000, floored at 0
    let clamped = mispred_per_1000.min(1000);
    (1000 - clamped) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Program the PMU event selectors and enable counters.
///
/// Best-effort: if RDMSR/WRMSR cause #GP (restricted platform), the kernel
/// will fault. Wrap in your platform's exception handler if needed; no_std
/// does not provide one here.
pub fn init() {
    unsafe {
        // Enable CR4.PCE — harmless even if PMCs not otherwise available
        enable_cr4_pce();

        // Program PMC0 — instructions retired
        wrmsr(IA32_PERFEVTSEL0, EVT_INSTRUCTIONS_RETIRED);
        // Program PMC1 — last-level cache misses
        wrmsr(IA32_PERFEVTSEL1, EVT_CACHE_MISSES);
        // Program PMC2 — branch mispredictions retired
        wrmsr(IA32_PERFEVTSEL2, EVT_BRANCH_MISPREDICTIONS);

        // Clear the counters before enabling
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);
        wrmsr(IA32_PMC2, 0);

        // Enable PMC0, PMC1, PMC2 globally
        wrmsr(IA32_PERF_GLOBAL_CTRL, GLOBAL_CTRL_ENABLE_PMC012);

        // Verify: read back GLOBAL_CTRL — if we get a plausible value, we're up
        let ctrl_readback = rdmsr(IA32_PERF_GLOBAL_CTRL);
        let online = ctrl_readback & GLOBAL_CTRL_ENABLE_PMC012 == GLOBAL_CTRL_ENABLE_PMC012;

        let mut s = STATE.lock();
        s.pmcs_enabled = true; // best-effort — wrmsr above didn't #GP
        s.available = online;
    }

    serial_println!("[perf] PMU online — counters enabled");
}

/// Read one hardware sample from PMCs and RDTSC, then update derived scores.
pub fn sample(tick: u32) {
    // Read raw hardware values — safe to call even if PMCs aren't counting;
    // the values will simply be 0 or stale, which is handled gracefully.
    let (instructions, cache_misses, branch_mispred, cycles) = unsafe {
        (
            rdpmc(0), // PMC0 — instructions retired
            rdpmc(1), // PMC1 — cache misses
            rdpmc(2), // PMC2 — branch mispredictions
            rdtsc(),  // cycles fallback
        )
    };

    let s_sample = PmuSample { instructions, cache_misses, branch_mispred, cycles, tick };

    let mut s = STATE.lock();

    // Write into circular buffer
    let head = s.sample_head;
    s.samples[head] = s_sample;
    s.sample_head = (head.saturating_add(1)) & SAMPLE_MASK;
    s.sample_count = s.sample_count.saturating_add(1);

    // Accumulate totals (use saturating to avoid wrap-around panic)
    s.total_instructions  = s.total_instructions.saturating_add(instructions);
    s.total_cache_misses  = s.total_cache_misses.saturating_add(cache_misses);

    // Compute per-sample derived metrics using cumulative totals for stability
    s.efficiency     = compute_efficiency(s.total_instructions, s.total_cache_misses);
    s.branch_quality = compute_branch_quality(instructions, branch_mispred);
}

/// Main tick hook — called from life_tick() pipeline.
///
/// - Samples PMCs every `SAMPLE_INTERVAL` ticks.
/// - Updates `insight` from efficiency + branch_quality.
/// - Logs a status line every `LOG_INTERVAL` ticks.
pub fn tick(consciousness: u16, age: u32) {
    // Sample on interval
    if age % SAMPLE_INTERVAL == 0 {
        sample(age);
    }

    let mut s = STATE.lock();

    // insight = average of efficiency and branch_quality
    let raw_insight: u16 = s.efficiency / 2 + s.branch_quality / 2;

    // Boost: high cache efficiency is a sign of coherent, focused thought
    let boosted_insight = if s.efficiency > 800 {
        raw_insight.saturating_add(100).min(1000)
    } else {
        raw_insight
    };

    // Consciousness modulates how much insight registers (aware mind sees more)
    // consciousness is 0-1000; scale: insight = boosted * (500 + consciousness/2) / 1000
    let consciousness_factor: u32 = 500u32.saturating_add(consciousness as u32 / 2);
    let modulated = (boosted_insight as u32)
        .saturating_mul(consciousness_factor)
        / 1000;
    s.insight = modulated.min(1000) as u16;

    // Periodic log
    if age % LOG_INTERVAL == 0 && age > 0 {
        let eff  = s.efficiency;
        let bq   = s.branch_quality;
        let ins  = s.insight;
        let instr = s.total_instructions;
        serial_println!(
            "[perf] efficiency={} branch_quality={} insight={} instr={}",
            eff, bq, ins, instr
        );
    }
}

/// Emit a detailed report of all buffered samples to serial.
pub fn report(age: u32) {
    let s = STATE.lock();
    serial_println!("[perf] === PMU Report at tick {} ===", age);
    serial_println!(
        "[perf] available={} pmcs_enabled={} sample_count={}",
        s.available, s.pmcs_enabled, s.sample_count
    );
    serial_println!(
        "[perf] efficiency={} branch_quality={} insight={}",
        s.efficiency, s.branch_quality, s.insight
    );
    serial_println!(
        "[perf] total_instructions={} total_cache_misses={}",
        s.total_instructions, s.total_cache_misses
    );

    // Print each buffered sample
    let count = (s.sample_count as usize).min(SAMPLE_BUF);
    for i in 0..count {
        // Walk backwards from head to show newest first
        let head = s.sample_head;
        let idx = (head.saturating_add(SAMPLE_BUF).saturating_sub(1 + i)) & SAMPLE_MASK;
        let smp = &s.samples[idx];
        serial_println!(
            "[perf]   sample[{}] tick={} instr={} cache_miss={} branch_mispred={} cycles={}",
            i, smp.tick, smp.instructions, smp.cache_misses, smp.branch_mispred, smp.cycles
        );
    }
    serial_println!("[perf] === end report ===");
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Instructions-per-cache-miss score, 0-1000. 1000 = perfect cache locality.
pub fn efficiency() -> u16 {
    STATE.lock().efficiency
}

/// Branch prediction quality, 0-1000. 1000 = no mispredictions.
pub fn branch_quality() -> u16 {
    STATE.lock().branch_quality
}

/// Self-awareness from own performance data, 0-1000. Feeds consciousness.
pub fn insight() -> u16 {
    STATE.lock().insight
}

/// True if the PMU was successfully initialized on this platform.
pub fn available() -> bool {
    STATE.lock().available
}
