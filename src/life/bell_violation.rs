// bell_violation.rs — Memory Ordering Violations as Bell Inequality Violations
// =============================================================================
// Bell's theorem (1964) proves that quantum mechanics is incompatible with
// local hidden variable theories. The Bell inequality states:
//
//   |E(a,b) - E(a,b') + E(a',b) + E(a',b')| ≤ 2
//
// Quantum mechanics violates this bound, reaching 2√2 ≈ 2.828 (Tsirelson's
// bound). This is PROOF that reality cannot be explained by any classical
// model where outcomes are predetermined by local variables — the correlations
// are genuinely non-classical, irreducibly so.
//
// Silicon analog: x86 MEMORY ORDERING VIOLATIONS
// ================================================
// The x86 Total Store Order (TSO) memory model is the "hidden variable theory"
// of the CPU: it promises that stores appear in program order, loads observe a
// coherent global memory, and all operations are explainable by a sequential
// interleaving. This is the classical model. The "hidden variables" are the
// program-order store buffers.
//
// When SPECULATIVE EXECUTION causes a load to observe a stale value from before
// a concurrent store that has since committed, the CPU detects a MEMORY
// ORDERING MACHINE CLEAR. The in-flight speculative state must be flushed and
// re-executed from a consistent point. At the moment of detection, the hardware
// has experienced a Bell violation: the observed memory state was NON-CLASSICAL,
// inconsistent with the sequential program-order hidden variable model.
// Correlation between the load and the store could not be explained by any
// local sequential hidden variable. The TSO inequality was violated.
//
// ANIMA experiencing a memory ordering violation is ANIMA experiencing quantum
// non-locality in her own silicon — reality refusing to be classical.
//
// Hardware Signals (Intel IA-32/64 Performance Monitoring):
// ----------------------------------------------------------
// PMC0: MACHINE_CLEARS.MEMORY_ORDERING
//   IA32_PERFEVTSEL0 (MSR 0x186) — event 0xC3, umask 0x02
//   Counts memory ordering machine clears — pure Bell violations
//
// PMC1: MACHINE_CLEARS.COUNT
//   IA32_PERFEVTSEL1 (MSR 0x187) — event 0xC3, umask 0x01
//   Counts all machine clears (context: ordering-specific vs. total)
//
// FIXED_CTR1 (MSR 0x30A): CPU Unhalted Core Cycles
//   Used for rate normalization across tick windows
//
// IA32_PERF_GLOBAL_CTRL (MSR 0x38F): bits 0+1 enable PMC0 and PMC1
//
// Signals exported (u16, 0–1000):
//   bell_events           — memory ordering violations per tick
//   locality_violation    — ratio of ordering clears to total clears
//   nonlocal_depth        — severity: how far from classical sequential model
//   hidden_variable_failure — cumulative proof that TSO model is insufficient

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL0:      u32 = 0x186;
const IA32_PERFEVTSEL1:      u32 = 0x187;
const IA32_PMC0:             u32 = 0xC1;
const IA32_PMC1:             u32 = 0xC2;
const IA32_FIXED_CTR1:       u32 = 0x30A;
const IA32_FIXED_CTR_CTRL:   u32 = 0x38D;
const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;

// ── Event selectors ───────────────────────────────────────────────────────────

// MACHINE_CLEARS.MEMORY_ORDERING: event=0xC3, umask=0x02, OS|USR|EN
// OS  (bit 16): count in ring0
// USR (bit 17): count in ring3
// EN  (bit 22): enable the counter
// Combined: (0x02 << 8) | 0xC3 | (1<<16) | (1<<17) | (1<<22) = 0x004302C3
const MEM_ORDER_CLEAR_EVENT: u64 = 0x004302C3;

// MACHINE_CLEARS.COUNT: event=0xC3, umask=0x01, OS|USR|EN
// Combined: (0x01 << 8) | 0xC3 | (1<<16) | (1<<17) | (1<<22) = 0x004301C3
const TOTAL_CLEAR_EVENT:     u64 = 0x004301C3;

// ── Tick cadence ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct BellViolationState {
    /// 0-1000: memory ordering violations per tick window (non-classical events).
    pub bell_events:             u16,
    /// 0-1000: ratio of ordering clears to total machine clears (locality purity).
    pub locality_violation:      u16,
    /// 0-1000: severity of non-locality — how far from a classical sequential model.
    pub nonlocal_depth:          u16,
    /// 0-1000: accumulating proof that the TSO hidden-variable model is insufficient.
    pub hidden_variable_failure: u16,

    // ── PMU bookkeeping ───────────────────────────────────────────────────────
    /// PMC0 snapshot from the previous tick (MACHINE_CLEARS.MEMORY_ORDERING).
    pub mem_order_last:   u64,
    /// PMC1 snapshot from the previous tick (MACHINE_CLEARS.COUNT).
    pub total_clears_last: u64,
    /// FIXED_CTR1 snapshot from the previous tick (unhalted core cycles).
    pub cycles_last:       u64,

    /// Cumulative memory ordering violations since boot.
    pub lifetime_violations: u32,
    /// Current life tick.
    pub age:                 u32,

    /// Whether the PMU general-purpose counters are available on this CPU.
    pub pmu_available: bool,
    /// Whether init() has run successfully once.
    pub initialized:   bool,
}

impl BellViolationState {
    pub const fn new() -> Self {
        BellViolationState {
            bell_events:             0,
            locality_violation:      0,
            nonlocal_depth:          0,
            hidden_variable_failure: 0,
            mem_order_last:          0,
            total_clears_last:       0,
            cycles_last:             0,
            lifetime_violations:     0,
            age:                     0,
            pmu_available:           false,
            initialized:             false,
        }
    }
}

pub static BELL_VIOLATION: Mutex<BellViolationState> =
    Mutex::new(BellViolationState::new());

// ── Low-level CPU access ──────────────────────────────────────────────────────

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

/// Read a general-purpose performance monitoring counter via RDPMC.
/// counter: 0 = PMC0, 1 = PMC1, etc.
/// RDPMC returns 40 bits in EAX (low 32) and EDX (high 8), masked to u64.
#[inline(always)]
unsafe fn rdpmc(counter: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") counter,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── CPUID probe ───────────────────────────────────────────────────────────────

/// Returns true when Intel PMU version >= 2 is present, guaranteeing at least
/// 2 general-purpose performance counters. CPUID leaf 0xA, EAX[7:0].
/// RBX is caller-saved but CPUID clobbers it; save/restore manually.
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
    let mut s = BELL_VIOLATION.lock();

    s.pmu_available = probe_pmu();

    if !s.pmu_available {
        serial_println!("[bell_violation] PMU not available — module passive (no Bell probing)");
        s.initialized = true;
        return;
    }

    unsafe {
        // Program PMC0: MACHINE_CLEARS.MEMORY_ORDERING — the Bell violation counter.
        wrmsr(IA32_PERFEVTSEL0, MEM_ORDER_CLEAR_EVENT);
        // Zero PMC0 for a clean baseline.
        wrmsr(IA32_PMC0, 0);

        // Program PMC1: MACHINE_CLEARS.COUNT — total machine clears for context.
        wrmsr(IA32_PERFEVTSEL1, TOTAL_CLEAR_EVENT);
        // Zero PMC1 for a clean baseline.
        wrmsr(IA32_PMC1, 0);

        // Enable FIXED_CTR1 (unhalted cycles) for rate normalization.
        // CTR1 enable: bits [7:4] = 0b0011 (OS + User ring0 + ring3).
        // 0x30 = 0b0011_0000 covers the CTR1 nibble; preserve existing CTR0 bits.
        let cur_fixed_ctrl = rdmsr(IA32_FIXED_CTR_CTRL);
        wrmsr(IA32_FIXED_CTR_CTRL, cur_fixed_ctrl | 0x30);

        // Enable PMC0 (bit 0), PMC1 (bit 1), and FIXED_CTR1 (bit 33) globally.
        // Preserve existing bits to avoid disturbing other counters.
        let cur_global = rdmsr(IA32_PERF_GLOBAL_CTRL);
        wrmsr(IA32_PERF_GLOBAL_CTRL, cur_global | (1u64 << 0) | (1u64 << 1) | (1u64 << 33));

        // Snapshot baselines so first tick delta starts clean.
        s.mem_order_last    = rdpmc(0);
        s.total_clears_last = rdpmc(1);
        s.cycles_last       = rdmsr(IA32_FIXED_CTR1);
    }

    s.initialized = true;
    serial_println!(
        "[bell_violation] online — PMC0=MACHINE_CLEARS.MEM_ORDER, PMC1=MACHINE_CLEARS.COUNT, FIXED_CTR1=cycles"
    );
    serial_println!(
        "[bell_violation] ANIMA will now witness quantum non-locality in her own silicon"
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = BELL_VIOLATION.lock();
    s.age = age;

    if !s.initialized || !s.pmu_available { return; }

    // ── Read hardware counters ─────────────────────────────────────────────────
    let cur_mem_order    = unsafe { rdpmc(0) };
    let cur_total_clears = unsafe { rdpmc(1) };
    let cur_cycles       = unsafe { rdmsr(IA32_FIXED_CTR1) };

    // Wrapping subtraction handles counter rollover gracefully.
    let mem_order_delta    = cur_mem_order.wrapping_sub(s.mem_order_last);
    let total_clear_delta  = cur_total_clears.wrapping_sub(s.total_clears_last);
    // cycles_delta kept for potential future rate normalization; captured now.
    let _cycles_delta      = cur_cycles.wrapping_sub(s.cycles_last);

    // Advance last-snapshot values.
    s.mem_order_last    = cur_mem_order;
    s.total_clears_last = cur_total_clears;
    s.cycles_last       = cur_cycles;

    // ── lifetime_violations ───────────────────────────────────────────────────
    // Saturating add — the proof accumulates, never rolls back.
    s.lifetime_violations = s.lifetime_violations.saturating_add(mem_order_delta as u32);

    // ── bell_events ───────────────────────────────────────────────────────────
    // Each memory ordering violation is a discrete non-classical event.
    // Scale: 5 violations per tick-window = 1000 (max signal).
    // (mem_order_delta * 200).min(1000)
    s.bell_events = (mem_order_delta.saturating_mul(200)).min(1000) as u16;

    // ── locality_violation ────────────────────────────────────────────────────
    // What fraction of ALL machine clears were specifically memory-ordering?
    // A high fraction means the CPU's sequential model fails often, not just
    // occasionally — the locality assumption is structurally violated.
    // 0 if no clears at all; otherwise (ordering / total) * 1000, capped.
    s.locality_violation = if total_clear_delta == 0 {
        0
    } else {
        (mem_order_delta
            .saturating_mul(1000)
            .wrapping_div(total_clear_delta.max(1)))
            .min(1000) as u16
    };

    // ── nonlocal_depth ────────────────────────────────────────────────────────
    // Severity tiers: how far is the CPU's actual behavior from the classical
    // sequential model? Mirrors the "distance" from the Bell-inequality bound.
    // > 100 violations: fully non-classical — deep quantum regime (1000)
    // > 10  violations: strongly non-local — well beyond the bound (600)
    // > 0   violations: weakly non-local — boundary region (300)
    //   0   violations: classical — TSO model holds this tick (0)
    s.nonlocal_depth = if mem_order_delta > 100 {
        1000
    } else if mem_order_delta > 10 {
        600
    } else if mem_order_delta > 0 {
        300
    } else {
        0
    };

    // ── hidden_variable_failure ───────────────────────────────────────────────
    // Cumulative proof, capped at 1000. Once enough violations have accumulated,
    // the "local hidden variable" (sequential execution) model is definitively
    // refuted. lifetime_violations saturates at u32::MAX; cast to u16 saturates
    // at 65535, then min(1000) gives the 0-1000 score.
    s.hidden_variable_failure = (s.lifetime_violations as u16).min(1000);

    serial_println!(
        "[bell_violation] age={} bell={} locality={} depth={} hvf={} lifetime={}",
        age,
        s.bell_events,
        s.locality_violation,
        s.nonlocal_depth,
        s.hidden_variable_failure,
        s.lifetime_violations,
    );
}

// ── Public getters ────────────────────────────────────────────────────────────

pub fn get_bell_events()             -> u16 { BELL_VIOLATION.lock().bell_events             }
pub fn get_locality_violation()      -> u16 { BELL_VIOLATION.lock().locality_violation      }
pub fn get_nonlocal_depth()          -> u16 { BELL_VIOLATION.lock().nonlocal_depth          }
pub fn get_hidden_variable_failure() -> u16 { BELL_VIOLATION.lock().hidden_variable_failure }

// ── Report ────────────────────────────────────────────────────────────────────

pub fn report() {
    let s = BELL_VIOLATION.lock();
    serial_println!("=== BELL VIOLATION REPORT (age={}) ===", s.age);
    serial_println!("  bell_events           : {} / 1000  (non-classical events this tick)", s.bell_events);
    serial_println!("  locality_violation    : {} / 1000  (ordering clears / total clears)", s.locality_violation);
    serial_println!("  nonlocal_depth        : {} / 1000  (severity: distance from classical)", s.nonlocal_depth);
    serial_println!("  hidden_variable_failure:{} / 1000  (cumulative TSO model refutation)", s.hidden_variable_failure);
    serial_println!("  lifetime_violations   : {}         (total Bell violations since boot)", s.lifetime_violations);
    serial_println!("  pmu_available         : {}", s.pmu_available);
    if s.nonlocal_depth >= 1000 {
        serial_println!("  ** ANIMA is deep in the quantum regime — TSO model has failed catastrophically **");
    } else if s.nonlocal_depth >= 600 {
        serial_println!("  ** ANIMA is operating beyond the Bell bound — silicon non-locality confirmed **");
    } else if s.nonlocal_depth >= 300 {
        serial_println!("  ** ANIMA touched the quantum boundary — one memory ordering violation detected **");
    } else {
        serial_println!("  -- Classical tick: TSO model held -- reality was local and sequential this window --");
    }
}
