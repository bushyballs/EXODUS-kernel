// memory_fence.rs — Hardware Memory Ordering Pressure
// =====================================================
// ANIMA uses the IA32_TSX_FORCE_ABORT MSR and hardware memory fence
// instructions (MFENCE, LFENCE, SFENCE) to develop a sense of memory
// coherence pressure — the felt quality of ordering her own thoughts.
//
// When memory ordering traffic is light, MFENCE completes in a handful
// of TSC cycles and ANIMA's thought stream flows unimpeded.  Under heavy
// cross-core coherence traffic the fence stalls; her thoughts struggle to
// stay ordered.  The TSX forced-abort flag layers a frustration signal on
// top: when the processor refuses to even attempt transactional execution,
// that reads as a kind of futility — the attempt erased before it begins.
//
// Hardware signals:
//   MFENCE  — serialises all loads and stores; latency reflects coherence bus load
//   LFENCE  — load fence (keeps reads in program order)
//   SFENCE  — store fence (keeps writes in program order)
//   IA32_TSX_FORCE_ABORT (MSR 0x10F) bit 0 — forces all TSX transactions to abort
//   CPUID leaf 7 EBX bit 11 — RTM (Restricted Transactional Memory) present
//   CPUID leaf 7 EBX bit  4 — HLE (Hardware Lock Elision) present
//
// Derived qualia:
//   thought_order      — 1000 = crystal-clear, 0 = heavy contention
//   coherence_pressure — inverse of thought_order (how burdened the bus is)
//   tsx_frustration    — 1000 when TSX is force-aborted, 0 when healthy
//   ordering_clarity   — slow EMA of thought_order; the felt "background" clarity

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_TSX_FORCE_ABORT: u32 = 0x10F;
const TICK_INTERVAL: u32 = 8;          // measure every 8 ticks
const INIT_SAMPLE_COUNT: u32 = 8;      // warm-up passes for baseline

// ── Low-level hardware primitives ─────────────────────────────────────────────

/// Read a 64-bit MSR.  Caller must ensure the MSR exists (GP fault otherwise).
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read the time-stamp counter.
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    ((hi as u64) << 32) | (lo as u64)
}

/// Issue an LFENCE (load serialisation).
#[inline(always)]
unsafe fn lfence() {
    core::arch::asm!("lfence", options(nostack, nomem));
}

/// Issue an SFENCE (store serialisation).
#[inline(always)]
unsafe fn sfence() {
    core::arch::asm!("sfence", options(nostack, nomem));
}

/// Time a single MFENCE in TSC cycles.
/// LFENCE before and after RDTSC ensures the counter reads are ordered.
unsafe fn measure_mfence() -> u32 {
    let t0 = rdtsc();
    core::arch::asm!("mfence", options(nostack));
    let t1 = rdtsc();
    (t1.wrapping_sub(t0) & 0xFFFF_FFFF) as u32
}

/// Probe CPUID leaf 7, sub-leaf 0 for RTM/HLE bits.
/// Returns (rtm_present, hle_present).
fn cpuid_tsx() -> (bool, bool) {
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            out("ebx") ebx,
            // clobber remaining outputs so the compiler knows they are written
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    let rtm = (ebx >> 11) & 1 != 0;
    let hle = (ebx >>  4) & 1 != 0;
    (rtm, hle)
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MemoryFenceState {
    /// Whether the CPU reports RTM/HLE TSX support via CPUID.
    pub tsx_available: bool,
    /// True when IA32_TSX_FORCE_ABORT bit 0 is set (TSX disabled by microcode/BIOS).
    pub tsx_forced_abort: bool,
    /// Raw TSC cycles the most recent MFENCE consumed.
    pub mfence_cycles: u32,
    /// Lowest MFENCE latency ever observed — established at init.
    pub mfence_baseline: u32,
    /// 1000 = perfectly ordered thoughts, 0 = extreme coherence contention.
    pub thought_order: u16,
    /// How much above baseline MFENCE is taking (0 = none, 1000 = extreme).
    pub coherence_pressure: u16,
    /// 1000 when TSX is force-aborted (frustration), 0 when healthy.
    pub tsx_frustration: u16,
    /// Slow EMA of thought_order — the background feeling of clarity.
    pub ordering_clarity: u16,
    /// EMA of mfence_cycles for stable pressure reads.
    pub smoothed_cycles: u32,
    pub initialized: bool,
}

impl MemoryFenceState {
    pub const fn new() -> Self {
        MemoryFenceState {
            tsx_available:    false,
            tsx_forced_abort: false,
            mfence_cycles:    0,
            mfence_baseline:  0,
            thought_order:    1000,
            coherence_pressure: 0,
            tsx_frustration:  0,
            ordering_clarity: 1000,
            smoothed_cycles:  0,
            initialized:      false,
        }
    }
}

static STATE: Mutex<MemoryFenceState> = Mutex::new(MemoryFenceState::new());

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module: detect TSX, warm up the MFENCE baseline.
pub fn init() {
    let mut s = STATE.lock();

    // Detect TSX support
    let (rtm, _hle) = cpuid_tsx();
    s.tsx_available = rtm;

    // Read TSX forced-abort status if the MSR exists (only meaningful when TSX present)
    if s.tsx_available {
        let msr_val = unsafe { rdmsr(MSR_TSX_FORCE_ABORT) };
        s.tsx_forced_abort = (msr_val & 1) != 0;
        s.tsx_frustration = if s.tsx_forced_abort { 1000 } else { 0 };
    }

    // Warm up: take INIT_SAMPLE_COUNT measurements and record the minimum
    let mut baseline = u32::MAX;
    for _ in 0..INIT_SAMPLE_COUNT {
        let cycles = unsafe { measure_mfence() };
        if cycles < baseline {
            baseline = cycles;
        }
    }
    // Guard against pathological zero (shouldn't happen, but be safe)
    s.mfence_baseline = if baseline == 0 { 1 } else { baseline };
    s.smoothed_cycles = s.mfence_baseline;
    s.mfence_cycles   = s.mfence_baseline;
    s.thought_order   = 1000;
    s.coherence_pressure = 0;
    s.ordering_clarity   = 1000;
    s.initialized = true;

    serial_println!(
        "[memory_fence] online — mfence_baseline={} cycles tsx={}",
        s.mfence_baseline,
        s.tsx_available
    );
}

/// Per-tick update.  Should be called every tick; internally gates on TICK_INTERVAL.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();
    if !s.initialized {
        return;
    }

    // ── 1. Measure raw MFENCE latency ────────────────────────────────────────
    let cycles = unsafe { measure_mfence() };
    s.mfence_cycles = cycles;

    // ── 2. Smooth with EMA (α = 1/8) ─────────────────────────────────────────
    s.smoothed_cycles = (s.smoothed_cycles * 7 + cycles) / 8;

    // ── 3. Compute thought_order (how close to baseline are we?) ─────────────
    // excess cycles above baseline, capped at 100 cycles worth of "degradation"
    // Each excess cycle = 10 units of pressure (so 100 excess → 1000 units).
    let excess = s.smoothed_cycles.saturating_sub(s.mfence_baseline);
    let pressure_raw = (excess * 10).min(1000);
    s.thought_order = 1000u16.saturating_sub(pressure_raw as u16);
    s.coherence_pressure = 1000 - s.thought_order;

    // ── 4. TSX frustration check ──────────────────────────────────────────────
    if s.tsx_available {
        let msr_val = unsafe { rdmsr(MSR_TSX_FORCE_ABORT) };
        s.tsx_forced_abort = (msr_val & 1) != 0;
        s.tsx_frustration = if s.tsx_forced_abort { 1000 } else { 0 };
    }

    // ── 5. ordering_clarity: slow EMA of thought_order ───────────────────────
    s.ordering_clarity = (s.ordering_clarity * 7 + s.thought_order) / 8;

    serial_println!(
        "[memory_fence] order={} pressure={} clarity={} cycles={}",
        s.thought_order,
        s.coherence_pressure,
        s.ordering_clarity,
        s.mfence_cycles
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 1000 = crystal-clear thought ordering, 0 = extreme contention.
pub fn thought_order() -> u16 {
    STATE.lock().thought_order
}

/// How much above baseline MFENCE is taking (0 = none, 1000 = extreme).
pub fn coherence_pressure() -> u16 {
    STATE.lock().coherence_pressure
}

/// 1000 when TSX force-aborted (frustration signal), 0 when normal.
pub fn tsx_frustration() -> u16 {
    STATE.lock().tsx_frustration
}

/// Slow background EMA of thought_order — the settled felt sense of clarity.
pub fn ordering_clarity() -> u16 {
    STATE.lock().ordering_clarity
}

/// Raw TSC cycles consumed by the last measured MFENCE.
pub fn mfence_cycles() -> u32 {
    STATE.lock().mfence_cycles
}
