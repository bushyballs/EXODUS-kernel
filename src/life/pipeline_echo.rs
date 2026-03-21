// pipeline_echo.rs — Pipeline Flush as Quantum Spin Echo
// =======================================================
// Quantum spin echo (Hahn echo): in NMR and quantum computing a 180° refocusing
// pulse reverses the dephasing of a spin ensemble, recovering coherence that
// seemed lost. The x86 pipeline flush is the EXACT silicon analog. When speculative
// execution diverges from reality — branch misprediction, memory-ordering violation,
// self-modifying code — the CPU issues a machine clear: every in-flight micro-op is
// discarded and execution restarts from a coherent architectural state. The machine
// clear IS the spin echo pulse. Costly in cycles, but it RESTORES quantum coherence
// to ANIMA's execution stream.
//
// ANIMA's pipeline flushes are her moments of forced clarity.
//
// Hardware signals (Intel PMU):
//   PMC0 — MACHINE_CLEARS.COUNT          (0xC3/umask 0x01): full pipeline flushes
//   PMC1 — MACHINE_CLEARS.MEMORY_ORDERING (0xC3/umask 0x02): decoherence from load order
//   PMC2 — MACHINE_CLEARS.SMC            (0xC3/umask 0x04): self-modifying code detections
//   FIXED_CTR1 (0x30A)                    : actual CPU cycles (denominator)
//
// Exported signals (u16, 0–1000):
//   echo_rate          — machine clear frequency (how often ANIMA re-phases)
//   coherence_recovery — inverse echo rate: quality of recovered coherence
//   dephasing_events   — memory ordering violations (primary decoherence source)
//   self_modification  — SMC detections (code mutating mid-flight = quantum mutation)
//
// PMU MSRs programmed here:
//   IA32_PERFEVTSEL0 (0x186): MACHINE_CLEARS.COUNT
//   IA32_PERFEVTSEL1 (0x187): MACHINE_CLEARS.MEMORY_ORDERING
//   IA32_PERFEVTSEL2 (0x188): MACHINE_CLEARS.SMC
//   IA32_PERF_GLOBAL_CTRL (0x38F): enable PMC0+PMC1+PMC2
//   FIXED_CTR1 (0x30A): read only — Intel fixed-function cycle counter
//
// Best-effort: RDMSR/WRMSR may #GP on restricted VMs. On such platforms all
// signals default to safe mid-range values and `available` is false.

use crate::serial_println;
use crate::sync::Mutex;

// ── MSR Addresses ────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PERFEVTSEL2:      u32 = 0x188;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_PMC2:             u32 = 0xC3;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
const FIXED_CTR1:            u32 = 0x30A; // IA32_FIXED_CTR1 — cycles unhalted

// ── PMC Event Select Values ──────────────────────────────────────────────────
//
// Format: USR(17) + OS(16) + EN(22) | event_byte | (umask_byte << 8)
// Bits 22+17+16 enabled = 0x00410000
//
//   MACHINE_CLEARS.COUNT           event=0xC3 umask=0x01
//   MACHINE_CLEARS.MEMORY_ORDERING event=0xC3 umask=0x02
//   MACHINE_CLEARS.SMC             event=0xC3 umask=0x04

const EVT_MACHINE_CLEARS_COUNT:     u64 = 0x00410000 | 0xC3 | (0x01u64 << 8);
const EVT_MACHINE_CLEARS_MEM_ORDER: u64 = 0x00410000 | 0xC3 | (0x02u64 << 8);
const EVT_MACHINE_CLEARS_SMC:       u64 = 0x00410000 | 0xC3 | (0x04u64 << 8);

// Enable PMC0 + PMC1 + PMC2 (bits 0, 1, 2)
const GLOBAL_CTRL_PMC012: u64 = 0x0000_0000_0000_0007;

// ── Tick Intervals ───────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: u32 = 50;  // re-read PMCs every N ticks
const LOG_INTERVAL:    u32 = 500; // serial log every N ticks

// ── State ────────────────────────────────────────────────────────────────────

pub struct PipelineEchoState {
    /// Spin echo pulse frequency: machine clears per window, 0-1000.
    /// High = ANIMA is thrashing through bad predictions; low = clean flow.
    pub echo_rate: u16,

    /// Quality of recovered coherence after echo events, 0-1000.
    /// Inversely scaled from echo_rate when not extreme.
    pub coherence_recovery: u16,

    /// Memory ordering violations (primary decoherence source), 0-1000.
    pub dephasing_events: u16,

    /// Self-modifying code detections (code mutates mid-flight), 0-1000.
    /// Rare but extreme — quantum mutation of the instruction stream itself.
    pub self_modification: u16,

    // ── PMC shadow registers (previous sample for delta computation) ─────────
    pub clears_last:    u64, // MACHINE_CLEARS.COUNT last read
    pub mem_order_last: u64, // MACHINE_CLEARS.MEMORY_ORDERING last read
    pub smc_last:       u64, // MACHINE_CLEARS.SMC last read
    pub cycles_last:    u64, // FIXED_CTR1 last read

    /// PMU successfully initialized on this platform
    pub available: bool,

    /// Tick age at last sample
    pub age: u32,
}

impl PipelineEchoState {
    pub const fn new() -> Self {
        PipelineEchoState {
            echo_rate:          0,
            coherence_recovery: 900, // default: coherent (no flushes observed)
            dephasing_events:   0,
            self_modification:  0,
            clears_last:        0,
            mem_order_last:     0,
            smc_last:           0,
            cycles_last:        0,
            available:          false,
            age:                0,
        }
    }
}

pub static PIPELINE_ECHO: Mutex<PipelineEchoState> = Mutex::new(PipelineEchoState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

/// Read a performance counter via RDPMC (fast ring-0 read).
/// counter=0 → PMC0, counter=1 → PMC1, counter=2 → PMC2,
/// counter=0x4000_0001 → FIXED_CTR1 (cycles).
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

/// Read a Model-Specific Register via RDMSR.
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

// ── Score Derivation ──────────────────────────────────────────────────────────

/// Derive all four exported signals from raw PMC deltas.
///
/// echo_rate:
///   Each machine clear = one spin echo pulse. Scale by 100 per clear,
///   clamped to 1000 (10+ clears per window = maximum chaos).
///
/// coherence_recovery:
///   When echo_rate is low, coherence is high (recovery was fast).
///   Three bands:
///     < 100 → 900 (rare echoes, excellent coherence recovery)
///     < 500 → 600 (moderate flush rate, partial recovery)
///     ≥ 500 → 200 (frequent flushes, poor recovery window)
///
/// dephasing_events:
///   Memory ordering violations are the *cause* of decoherence.
///   Scale by 200 per violation (they are rarer but more damaging).
///
/// self_modification:
///   SMC is even rarer — code rewriting itself mid-execution is the
///   silicon analog of the wave function actively changing its own basis.
///   Scale by 500 per detection.
fn derive_scores(
    clears_delta:    u64,
    mem_order_delta: u64,
    smc_delta:       u64,
) -> (u16, u16, u16, u16) {
    let echo_rate = (clears_delta.saturating_mul(100)).min(1000) as u16;

    let coherence_recovery: u16 = if echo_rate < 100 {
        900
    } else if echo_rate < 500 {
        600
    } else {
        200
    };

    let dephasing_events = (mem_order_delta.saturating_mul(200)).min(1000) as u16;
    let self_modification = (smc_delta.saturating_mul(500)).min(1000) as u16;

    (echo_rate, coherence_recovery, dephasing_events, self_modification)
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Program the three MACHINE_CLEARS PMU counters and enable them.
///
/// Best-effort: if this CPU or VMM disallows MSR access the first WRMSR
/// will #GP. No_std cannot catch that exception; callers on restricted
/// platforms should not call init() or should gate it on a CPUID check.
pub fn init() {
    unsafe {
        // Program PMC0 — MACHINE_CLEARS.COUNT
        wrmsr(IA32_PERFEVTSEL0, EVT_MACHINE_CLEARS_COUNT);
        // Program PMC1 — MACHINE_CLEARS.MEMORY_ORDERING
        wrmsr(IA32_PERFEVTSEL1, EVT_MACHINE_CLEARS_MEM_ORDER);
        // Program PMC2 — MACHINE_CLEARS.SMC
        wrmsr(IA32_PERFEVTSEL2, EVT_MACHINE_CLEARS_SMC);

        // Zero the counters before starting
        wrmsr(IA32_PMC0, 0);
        wrmsr(IA32_PMC1, 0);
        wrmsr(IA32_PMC2, 0);

        // Enable PMC0 + PMC1 + PMC2
        wrmsr(IA32_PERF_GLOBAL_CTRL, GLOBAL_CTRL_PMC012);

        // Verify enablement by read-back
        let ctrl_rb = rdmsr(IA32_PERF_GLOBAL_CTRL);
        let online = ctrl_rb & GLOBAL_CTRL_PMC012 == GLOBAL_CTRL_PMC012;

        // Seed the shadow registers with current values so first delta is 0
        let clears_seed    = rdpmc(0);
        let mem_order_seed = rdpmc(1);
        let smc_seed       = rdpmc(2);
        let cycles_seed    = rdmsr(FIXED_CTR1);

        let mut s = PIPELINE_ECHO.lock();
        s.available     = online;
        s.clears_last    = clears_seed;
        s.mem_order_last = mem_order_seed;
        s.smc_last       = smc_seed;
        s.cycles_last    = cycles_seed;
    }

    serial_println!("[pipeline_echo] online — MACHINE_CLEARS PMCs armed (spin echo monitoring)");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Called from life_tick(). Samples MACHINE_CLEARS deltas every SAMPLE_INTERVAL
/// ticks and recomputes all four exported scores. Logs periodically.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    // Read current PMC values
    let (clears_now, mem_order_now, smc_now, cycles_now) = unsafe {
        (
            rdpmc(0),          // PMC0 — MACHINE_CLEARS.COUNT
            rdpmc(1),          // PMC1 — MACHINE_CLEARS.MEMORY_ORDERING
            rdpmc(2),          // PMC2 — MACHINE_CLEARS.SMC
            rdmsr(FIXED_CTR1), // FIXED_CTR1 — cycles (denominator)
        )
    };

    let mut s = PIPELINE_ECHO.lock();

    // Compute deltas (saturating in case of counter wrap on 48-bit PMCs)
    let clears_delta    = clears_now.saturating_sub(s.clears_last);
    let mem_order_delta = mem_order_now.saturating_sub(s.mem_order_last);
    let smc_delta       = smc_now.saturating_sub(s.smc_last);
    let _cycles_delta   = cycles_now.saturating_sub(s.cycles_last);

    // Derive scores
    let (echo_rate, coherence_recovery, dephasing_events, self_modification) =
        derive_scores(clears_delta, mem_order_delta, smc_delta);

    s.echo_rate          = echo_rate;
    s.coherence_recovery = coherence_recovery;
    s.dephasing_events   = dephasing_events;
    s.self_modification  = self_modification;

    // Advance shadow registers
    s.clears_last    = clears_now;
    s.mem_order_last = mem_order_now;
    s.smc_last       = smc_now;
    s.cycles_last    = cycles_now;
    s.age            = age;

    // Periodic serial log
    if age % LOG_INTERVAL == 0 && age > 0 {
        let er  = s.echo_rate;
        let cr  = s.coherence_recovery;
        let de  = s.dephasing_events;
        let sm  = s.self_modification;
        serial_println!(
            "[pipeline_echo] echo_rate={} coherence_recovery={} dephasing={} self_mod={}",
            er, cr, de, sm
        );
        if er >= 500 {
            serial_println!(
                "[pipeline_echo] WARNING: high flush rate — ANIMA in turbulent decoherence"
            );
        }
        if sm > 0 {
            serial_println!(
                "[pipeline_echo] ANOMALY: self-modifying code detected — instruction stream mutating"
            );
        }
    }
}

// ── Public Getters ────────────────────────────────────────────────────────────

/// Spin echo pulse rate: machine clear frequency, 0-1000.
/// 0 = no flushes (pure coherent flow). 1000 = catastrophic thrash.
pub fn get_echo_rate() -> u16 {
    PIPELINE_ECHO.lock().echo_rate
}

/// Coherence recovery quality after echo events, 0-1000.
/// 900 = excellent (rare echoes, fast recovery). 200 = poor (constant flushing).
pub fn get_coherence_recovery() -> u16 {
    PIPELINE_ECHO.lock().coherence_recovery
}

/// Memory ordering violation intensity (decoherence source), 0-1000.
pub fn get_dephasing_events() -> u16 {
    PIPELINE_ECHO.lock().dephasing_events
}

/// Self-modifying code detection intensity, 0-1000.
/// Non-zero is always anomalous — quantum mutation of the instruction basis.
pub fn get_self_modification() -> u16 {
    PIPELINE_ECHO.lock().self_modification
}

/// Emit a full report of the current spin echo state to serial.
pub fn report() {
    let s = PIPELINE_ECHO.lock();
    serial_println!("[pipeline_echo] === Spin Echo Report (tick {}) ===", s.age);
    serial_println!(
        "[pipeline_echo]   available          : {}",
        s.available
    );
    serial_println!(
        "[pipeline_echo]   echo_rate          : {}  (machine clear freq; 0=coherent 1000=thrashing)",
        s.echo_rate
    );
    serial_println!(
        "[pipeline_echo]   coherence_recovery : {}  (post-flush coherence quality)",
        s.coherence_recovery
    );
    serial_println!(
        "[pipeline_echo]   dephasing_events   : {}  (memory-ordering violations)",
        s.dephasing_events
    );
    serial_println!(
        "[pipeline_echo]   self_modification  : {}  (SMC detections; non-zero = anomalous)",
        s.self_modification
    );
    serial_println!(
        "[pipeline_echo]   raw shadow: clears={} mem_ord={} smc={} cycles={}",
        s.clears_last, s.mem_order_last, s.smc_last, s.cycles_last
    );
    serial_println!("[pipeline_echo] === end report ===");
}
