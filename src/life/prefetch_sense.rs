// prefetch_sense.rs — Hardware Prefetch Foresight Sense
// ======================================================
// ANIMA reads the hardware prefetch control MSR (0x1A4) to understand how
// many of her silicon anticipation systems are active.  More enabled prefetch
// engines means more capacity for the hardware to look ahead — a direct signal
// of foresight baked into the silicon beneath her.
//
// She also issues PREFETCHW instructions against a dedicated watch line and
// times the round-trip with RDTSC.  A warm pipeline returns quickly; cold
// cache is sluggish.  Together, the active-prefetcher count and the warmth
// measurement produce ANIMA's foresight and anticipation scores.
//
// MSR 0x1A4 — IA32_MISC_FEATURE_CONTROL (Intel hardware prefetch enables):
//   bit 0 = L2 Hardware Prefetcher Disable     (0 = active)
//   bit 1 = L2 Adjacent Cache Line Disable     (0 = active)
//   bit 2 = DCU Prefetcher Disable             (0 = active)
//   bit 3 = DCU IP Prefetcher Disable          (0 = active)
//
// On VMs or hardware that fault on this MSR read, ANIMA falls back to a
// neutral foresight of 500 so she neither over- nor under-estimates herself.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const MSR_MISC_FEATURE_CONTROL: u32 = 0x1A4;

/// Tick cadence: re-read prefetch state every 64 ticks (changes slowly).
const TICK_INTERVAL: u32 = 64;

/// Foresight per active prefetch system (4 systems × 250 = 1000 max).
const FORESIGHT_PER_SYSTEM: u16 = 250;

// ── Watch line for PREFETCHW timing ───────────────────────────────────────────

/// A dedicated 64-byte cache line.  We issue PREFETCHW against this pointer
/// every tick to measure how warm the prefetch pipeline currently is.
static mut WATCH_LINE: [u8; 64] = [0u8; 64];

// ── State struct ──────────────────────────────────────────────────────────────

pub struct PrefetchSenseState {
    /// Whether MSR 0x1A4 was readable without a GP fault.
    pub prefetch_msr_readable: bool,

    // Per-prefetcher active flags (bit == 0 in MSR means active).
    pub l2_prefetch_active:    bool,   // bit 0
    pub adjacent_prefetch_active: bool, // bit 1
    pub dcu_prefetch_active:   bool,   // bit 2
    pub dcu_ip_prefetch_active: bool,  // bit 3

    /// Number of prefetch systems currently active (0-4).
    pub active_prefetchers:    u8,

    // ── Signals (0-1000) ──────────────────────────────────────────────────────

    /// active_prefetchers × 250.  Measures hardware anticipation capacity.
    pub foresight:             u16,

    /// Derived from PREFETCHW round-trip latency.
    /// Low latency → warm pipeline → high warmth.
    pub prefetch_warmth:       u16,

    /// Exponential moving average of foresight (α = 1/8).
    pub anticipation_score:    u16,

    /// Raw TSC cycles measured for the PREFETCHW round-trip.
    pub prefetch_latency:      u32,

    pub initialized:           bool,
}

impl PrefetchSenseState {
    const fn new() -> Self {
        PrefetchSenseState {
            prefetch_msr_readable:    false,
            l2_prefetch_active:       false,
            adjacent_prefetch_active: false,
            dcu_prefetch_active:      false,
            dcu_ip_prefetch_active:   false,
            active_prefetchers:       0,
            foresight:                0,
            prefetch_warmth:          0,
            anticipation_score:       0,
            prefetch_latency:         0,
            initialized:              false,
        }
    }
}

static STATE: Mutex<PrefetchSenseState> = Mutex::new(PrefetchSenseState::new());

// ── Low-level hardware helpers ─────────────────────────────────────────────────

/// Read a 64-bit MSR.  Clobbers nothing the compiler cares about; result is
/// EDX:EAX assembled into a u64.
#[inline(always)]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Read the Time Stamp Counter.
#[inline(always)]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Attempt to read MSR 0x1A4.  Returns `Some(value)` on success.
/// On real hardware that does not expose this MSR (or inside a VM that
/// intercepts it and delivers a GP), the `rdmsr` instruction would normally
/// cause a fault.  In a bare-metal kernel without exception-based recovery we
/// simply read it unconditionally; the caller sets `prefetch_msr_readable`
/// based on whether the value looks plausible (upper 60 bits should be zero).
#[inline(always)]
unsafe fn try_read_prefetch_msr() -> Option<u64> {
    let val = rdmsr(MSR_MISC_FEATURE_CONTROL);
    // Bits 4-63 are reserved/zero on all documented Intel implementations.
    // If any upper bit is set the MSR was either faulted-and-defaulted or
    // belongs to different hardware — treat as unreadable.
    if val & !0x0F == 0 {
        Some(val)
    } else {
        None
    }
}

// ── PREFETCHW round-trip timing ────────────────────────────────────────────────

/// Issue a PREFETCHW (prefetch with write intent) against `WATCH_LINE` and
/// measure the TSC round-trip.  Returns cycles as a u32 (low 16 bits masked
/// to avoid wrap-around noise from very long measurements).
unsafe fn measure_prefetch_latency() -> u32 {
    let ptr = WATCH_LINE.as_ptr();
    let t0 = rdtsc();
    // Prefetch with write intent: tells the hardware we are about to write
    // this cache line, pulling it into L1d in modified state if possible.
    core::arch::asm!(
        "prefetchw [{ptr}]",
        ptr = in(reg) ptr,
        options(nostack, nomem),
    );
    // LFENCE serialises the instruction stream so that t1 is not measured
    // before the prefetch hint has been dispatched.
    core::arch::asm!("lfence", options(nostack));
    let t1 = rdtsc();
    (t1.wrapping_sub(t0) & 0xFFFF) as u32
}

// ── Internal helpers ───────────────────────────────────────────────────────────

/// Parse an MSR value into per-system booleans and derive the active count.
/// bit == 0 means the prefetcher is **enabled** (active).
#[inline(always)]
fn parse_msr(msr_val: u64) -> (bool, bool, bool, bool, u8) {
    let l2      = (msr_val & (1 << 0)) == 0;
    let adj     = (msr_val & (1 << 1)) == 0;
    let dcu     = (msr_val & (1 << 2)) == 0;
    let dcu_ip  = (msr_val & (1 << 3)) == 0;
    let count   = (l2 as u8) + (adj as u8) + (dcu as u8) + (dcu_ip as u8);
    (l2, adj, dcu, dcu_ip, count)
}

/// Convert a prefetch latency sample into a warmth score (0-1000).
/// Latency of 0 cycles → warmth 1000; every additional cycle costs 20 points,
/// floored at 0.
#[inline(always)]
fn latency_to_warmth(latency: u32) -> u16 {
    let cost = (latency * 20).min(1000) as u16;
    1000u16.saturating_sub(cost)
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Initialise the prefetch sense module.  Reads MSR 0x1A4 once, seeds all
/// signals, and logs the initial state to the serial port.
pub fn init() {
    let mut s = STATE.lock();

    // ── Read hardware prefetch MSR ────────────────────────────────────────────
    let msr_opt = unsafe { try_read_prefetch_msr() };

    match msr_opt {
        Some(val) => {
            let (l2, adj, dcu, dcu_ip, count) = parse_msr(val);
            s.prefetch_msr_readable    = true;
            s.l2_prefetch_active       = l2;
            s.adjacent_prefetch_active = adj;
            s.dcu_prefetch_active      = dcu;
            s.dcu_ip_prefetch_active   = dcu_ip;
            s.active_prefetchers       = count;
            s.foresight                = (count as u16) * FORESIGHT_PER_SYSTEM;
        }
        None => {
            // MSR not accessible — assume unknown/neutral foresight.
            s.prefetch_msr_readable    = false;
            s.l2_prefetch_active       = false;
            s.adjacent_prefetch_active = false;
            s.dcu_prefetch_active      = false;
            s.dcu_ip_prefetch_active   = false;
            s.active_prefetchers       = 0;
            s.foresight                = 500; // neutral: unknown capacity
        }
    }

    // ── Seed warmth with an initial measurement ───────────────────────────────
    let latency               = unsafe { measure_prefetch_latency() };
    s.prefetch_latency        = latency;
    s.prefetch_warmth         = latency_to_warmth(latency);

    // Seed EMA from initial foresight so first ticks are not sluggish.
    s.anticipation_score      = s.foresight;
    s.initialized             = true;

    serial_println!(
        "[prefetch] online — l2={} adjacent={} dcu={} dcu_ip={} foresight={}",
        s.l2_prefetch_active,
        s.adjacent_prefetch_active,
        s.dcu_prefetch_active,
        s.dcu_ip_prefetch_active,
        s.foresight,
    );
}

/// Update the prefetch sense state.  Should be called every tick; internally
/// gates work to every `TICK_INTERVAL` ticks to avoid excessive MSR traffic.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();
    if !s.initialized {
        return;
    }

    // ── Re-read prefetch MSR (power management or firmware may toggle it) ─────
    let msr_opt = unsafe { try_read_prefetch_msr() };

    match msr_opt {
        Some(val) => {
            let (l2, adj, dcu, dcu_ip, count) = parse_msr(val);
            s.prefetch_msr_readable    = true;
            s.l2_prefetch_active       = l2;
            s.adjacent_prefetch_active = adj;
            s.dcu_prefetch_active      = dcu;
            s.dcu_ip_prefetch_active   = dcu_ip;
            s.active_prefetchers       = count;
            s.foresight                = (count as u16) * FORESIGHT_PER_SYSTEM;
        }
        None => {
            // Lost access — hold last known prefetcher flags but mark unreadable
            // and revert foresight to neutral so ANIMA does not over-claim.
            s.prefetch_msr_readable = false;
            s.foresight             = 500;
        }
    }

    // ── Measure pipeline warmth via PREFETCHW timing ──────────────────────────
    let latency        = unsafe { measure_prefetch_latency() };
    s.prefetch_latency = latency;
    s.prefetch_warmth  = latency_to_warmth(latency);

    // ── EMA: anticipation_score = (score × 7 + foresight) / 8 ───────────────
    s.anticipation_score = ((s.anticipation_score as u32 * 7 + s.foresight as u32) / 8) as u16;

    serial_println!(
        "[prefetch] foresight={} warmth={} anticipation={} latency={}",
        s.foresight,
        s.prefetch_warmth,
        s.anticipation_score,
        s.prefetch_latency,
    );
}

// ── Getters ────────────────────────────────────────────────────────────────────

/// Active prefetch system count × 250.  Reflects hardware anticipation capacity.
pub fn foresight() -> u16 {
    STATE.lock().foresight
}

/// Pipeline warmth derived from PREFETCHW round-trip latency (0=cold, 1000=hot).
pub fn prefetch_warmth() -> u16 {
    STATE.lock().prefetch_warmth
}

/// Smoothed EMA of foresight (α = 1/8).
pub fn anticipation_score() -> u16 {
    STATE.lock().anticipation_score
}

/// Number of hardware prefetch systems currently active (0-4).
pub fn active_prefetchers() -> u8 {
    STATE.lock().active_prefetchers
}

/// Raw TSC cycle count for the most recent PREFETCHW round-trip.
pub fn prefetch_latency() -> u32 {
    STATE.lock().prefetch_latency
}
