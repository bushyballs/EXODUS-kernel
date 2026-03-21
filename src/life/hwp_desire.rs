//! hwp_desire — HWP performance desire vs capability sense for ANIMA
//!
//! Reads Intel HWP MSRs to give ANIMA a yearning/desire sense.
//! IA32_HWP_REQUEST (0x774) holds what ANIMA wants to achieve.
//! IA32_HWP_CAPABILITIES (0x771) holds what she is capable of.
//! The gap between desired and highest capability = unrequited potential.
//! High desire_gap = yearning to go beyond current limits.

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ─────────────────────────────────────────────────────────

const MSR_HWP_CAPABILITIES: u32 = 0x771;
const MSR_HWP_REQUEST:       u32 = 0x774;

/// How often (in ticks) we resample IA32_HWP_REQUEST.
const TICK_INTERVAL: u32 = 32;

// ── State Struct ──────────────────────────────────────────────────────────────

pub struct HwpDesireState {
    /// Whether the processor supports HWP (CPUID leaf 6 EAX bit 7).
    pub hwp_available:      bool,

    // ── Raw capability fields from IA32_HWP_CAPABILITIES (MSR 0x771) ──────────
    /// Hardware performance ceiling — the highest ratio the CPU can sustain.
    pub highest_perf:       u8,
    /// Guaranteed sustained performance ratio.
    pub guaranteed_perf:    u8,
    /// Hardware performance floor.
    pub lowest_perf:        u8,

    // ── Raw request fields from IA32_HWP_REQUEST (MSR 0x774) ──────────────────
    /// Explicit desired performance target (0 = let hardware decide).
    pub desired_performance: u8,
    /// Energy-performance preference byte: 0 = max perf, 255 = max power save.
    pub epp:                u8,
    /// Software-requested performance floor.
    pub min_hint:           u8,
    /// Software-requested performance ceiling.
    pub max_hint:           u8,

    // ── Derived 0-1000 signals ─────────────────────────────────────────────────
    /// How much performance ANIMA is being asked for.
    /// Inverted EPP: EPP 0 (max perf) → 1000, EPP 255 (max save) → 0.
    pub performance_desire: u16,

    /// How wide ANIMA's allowed performance band is.
    /// (max_hint − min_hint) scaled to 0-1000: 1000 = unconstrained, 0 = locked.
    pub freedom_band:       u16,

    /// Efficiency lean: same value as performance_desire, named for wiring
    /// contexts where "how efficiently should I lean" is the question asked.
    /// High value = lean toward efficient high-performance; low = conserve.
    pub efficiency_lean:    u16,

    /// How closely the desired_performance target aims at the hardware ceiling.
    /// 1000 = aiming at highest_perf; 500 = hardware decides; 0 = minimum.
    pub desire_coherence:   u16,

    pub initialized: bool,
}

impl HwpDesireState {
    pub const fn new() -> Self {
        HwpDesireState {
            hwp_available:      false,
            highest_perf:       255,
            guaranteed_perf:    192,
            lowest_perf:        25,
            desired_performance: 0,
            epp:                128,
            min_hint:           0,
            max_hint:           255,
            performance_desire: 500,
            freedom_band:       1000,
            efficiency_lean:    500,
            desire_coherence:   500,
            initialized:        false,
        }
    }
}

static STATE: Mutex<HwpDesireState> = Mutex::new(HwpDesireState::new());

// ── Unsafe Hardware Helpers ───────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
/// Returns the full 64-bit value; upper 32 bits in EDX, lower in EAX.
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

/// Probe CPUID leaf 6 EAX bit 7 to determine if HWP is available.
fn probe_hwp() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    (eax >> 7) & 1 != 0
}

// ── Signal Derivation ─────────────────────────────────────────────────────────

/// Derive all 0-1000 signals from raw HWP register values.
/// Called inside tick(); mutates the state in place.
fn derive_signals(s: &mut HwpDesireState) {
    let epp  = s.epp;
    let min  = s.min_hint;
    let max  = s.max_hint;
    let des  = s.desired_performance;
    let high = s.highest_perf;

    // performance_desire: invert EPP — EPP 0 = max perf → 1000
    // Formula: ((255 - epp) as u16 * 1000 / 255).min(1000)
    s.performance_desire = ((255u16.saturating_sub(epp as u16)) * 1000 / 255).min(1000);

    // freedom_band: scaled range allowed by max_hint - min_hint
    // Formula: (max_hint.saturating_sub(min_hint) as u16 * 1000 / 255).min(1000)
    s.freedom_band = ((max.saturating_sub(min) as u16) * 1000 / 255).min(1000);

    // efficiency_lean: same signal as performance_desire, different semantic axis
    s.efficiency_lean = s.performance_desire;

    // desire_coherence: how high the desired_performance aims vs the hardware ceiling
    // If des == 0: hardware decides — neutral 500
    if des == 0 {
        s.desire_coherence = 500;
    } else {
        let denom = (high.max(1)) as u16;
        s.desire_coherence = ((des as u16) * 1000 / denom).min(1000);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the HWP desire module.
///
/// Probes CPUID leaf 6 for HWP support.  If available, reads
/// IA32_HWP_CAPABILITIES (MSR 0x771) to record the hardware performance
/// range.  Emits a serial diagnostic line.
pub fn init() {
    let available = probe_hwp();

    let mut s = STATE.lock();
    s.hwp_available = available;

    if available {
        let caps = unsafe { rdmsr(MSR_HWP_CAPABILITIES) };
        s.highest_perf    = (caps & 0xFF) as u8;
        s.guaranteed_perf = ((caps >> 8) & 0xFF) as u8;
        // bits 23:16 = most_efficient (skipped — not wired)
        s.lowest_perf     = ((caps >> 24) & 0xFF) as u8;

        serial_println!(
            "[hwp_desire] online — HWP available, highest_perf={} guaranteed={} lowest={}",
            s.highest_perf,
            s.guaranteed_perf,
            s.lowest_perf,
        );
    } else {
        serial_println!(
            "[hwp_desire] online — HWP not available on this CPU; signals hold defaults"
        );
    }

    s.initialized = true;
}

/// HWP desire tick — sample IA32_HWP_REQUEST and recompute derived signals.
///
/// Runs every 32 ticks.  If HWP is unavailable the function returns early
/// after the interval check, leaving the default neutral signals in place.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.hwp_available {
        return;
    }

    // Read IA32_HWP_REQUEST
    let req = unsafe { rdmsr(MSR_HWP_REQUEST) };

    s.min_hint           = (req & 0xFF) as u8;
    s.max_hint           = ((req >> 8)  & 0xFF) as u8;
    s.desired_performance = ((req >> 16) & 0xFF) as u8;
    s.epp                = ((req >> 24) & 0xFF) as u8;

    // Recompute all derived 0-1000 signals
    derive_signals(&mut *s);

    serial_println!(
        "[hwp_desire] desire={} freedom={} efficiency={} coherence={} epp={}",
        s.performance_desire,
        s.freedom_band,
        s.efficiency_lean,
        s.desire_coherence,
        s.epp,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// How much performance ANIMA is being asked for, 0-1000.
/// 1000 = max performance requested (EPP 0); 0 = max power saving (EPP 255).
pub fn performance_desire() -> u16 {
    STATE.lock().performance_desire
}

/// Width of ANIMA's allowed performance band, 0-1000.
/// 1000 = max_hint − min_hint spans the full 0-255 range (fully free).
/// 0 = performance is locked to a single point.
pub fn freedom_band() -> u16 {
    STATE.lock().freedom_band
}

/// Efficiency lean signal, 0-1000.  Mirrors performance_desire.
/// Used by wiring contexts that ask "how efficiently should ANIMA lean?"
pub fn efficiency_lean() -> u16 {
    STATE.lock().efficiency_lean
}

/// Desire coherence, 0-1000.
/// How closely the OS-specified desired_performance aims at the hardware ceiling.
/// 1000 = targeting highest_perf exactly; 500 = hardware decides; 0 = floor.
pub fn desire_coherence() -> u16 {
    STATE.lock().desire_coherence
}

/// Raw EPP byte from IA32_HWP_REQUEST bits 31:24.
/// 0 = maximum performance; 128 = balanced; 255 = maximum power saving.
pub fn epp() -> u8 {
    STATE.lock().epp
}

/// True if the CPU supports Hardware-managed P-states (CPUID leaf 6 EAX bit 7).
pub fn hwp_available() -> bool {
    STATE.lock().hwp_available
}
