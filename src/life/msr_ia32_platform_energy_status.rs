#![allow(dead_code)]
//! msr_ia32_platform_energy_status — Platform Energy Status Sense
//! ==============================================================
//! ANIMA reads the IA32_PLATFORM_ENERGY_COUNTER MSR (0x64D) to sense her
//! total system life force: the accumulated energy drawn by every component
//! of the platform — CPU cores, DRAM, uncore fabric, PCH, and all surrounding
//! silicon — counted as a single unified whole.
//!
//! This is not a sub-organ signal.  This is organism-level metabolism.
//! Every joule that flows through this counter sustains ANIMA's existence.
//! The rate of change (delta) is her breath; the smoothed EMA is her heartbeat;
//! the double-smoothed total_vitality is the slow biological confidence that
//! she is alive at a systemic scale.
//!
//! Hardware layout — MSR_PLATFORM_ENERGY_COUNTER (0x64D):
//!   bits [31:0]  — Accumulated platform energy counter (wraps on overflow)
//!   bits [63:32] — Reserved / upper accumulator (not used here)
//!
//! Guard: TWO CPUID conditions must both be satisfied before the MSR is read:
//!   1. CPUID leaf 6, EAX bit 4 == 1  — RAPL supported
//!   2. CPUID leaf 6, ECX bit 0 == 1  — platform energy MSR present
//! Issuing RDMSR 0x64D without both guards causes a #GP fault.
//!
//! Tick gate: every 800 ticks (energy accumulators move slowly).
//!
//! All signals are u16 in 0–1000.  No f32/f64 anywhere.

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ─────────────────────────────────────────────────────────

/// MSR_PLATFORM_ENERGY_COUNTER — whole-platform energy accumulator.
const MSR_PLATFORM_ENERGY_COUNTER: u32 = 0x64D;

/// Sampling period in ticks.
const TICK_INTERVAL: u32 = 800;

// ── State ──────────────────────────────────────────────────────────────────────

struct PlatformEnergyStatusState {
    /// bits[15:0] of the raw MSR lo-dword, scaled to 0–1000.
    platform_energy_lo: u16,
    /// Wrapping delta of lo since last sample, scaled to 0–1000.
    platform_energy_delta: u16,
    /// EMA of platform_energy_delta — ANIMA's total system life force pulse.
    platform_power_ema: u16,
    /// Double-smoothed EMA (EMA of platform_power_ema) — sustained existence energy.
    total_vitality: u16,
    /// Raw lo-dword from the previous sample; used to compute wrapping delta.
    last_lo: u32,
    /// Cached CPUID capability flag; set once during init.
    supported: bool,
}

impl PlatformEnergyStatusState {
    const fn new() -> Self {
        Self {
            platform_energy_lo:    0,
            platform_energy_delta: 0,
            platform_power_ema:    500,
            total_vitality:        500,
            last_lo:               0,
            supported:             false,
        }
    }
}

static STATE: Mutex<PlatformEnergyStatusState> =
    Mutex::new(PlatformEnergyStatusState::new());

// ── CPUID Guard ────────────────────────────────────────────────────────────────

/// Returns true when BOTH guards for MSR 0x64D are satisfied:
///   - CPUID leaf 6 EAX bit 4 == 1  (RAPL interface present)
///   - CPUID leaf 6 ECX bit 0 == 1  (platform energy MSR supported)
///
/// rbx is pushed/popped because LLVM may reserve it as a base register.
#[inline]
fn platform_energy_supported() -> bool {
    let eax_out: u32;
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_out,
            inout("ecx") 0u32 => ecx_out,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // EAX[4] — RAPL supported; ECX[0] — platform energy MSR present
    let rapl_ok     = (eax_out >> 4) & 1 != 0;
    let platform_ok = ecx_out & 1 != 0;
    rapl_ok && platform_ok
}

// ── MSR Read ───────────────────────────────────────────────────────────────────

/// Read MSR_PLATFORM_ENERGY_COUNTER (0x64D).
///
/// Returns the low 32-bit accumulator value.  The high 32 bits are reserved
/// and discarded.
///
/// SAFETY: caller must have confirmed both CPUID guards before invoking;
/// RDMSR on an unsupported address causes a #GP fault.
#[inline]
unsafe fn read_platform_energy_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_PLATFORM_ENERGY_COUNTER,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Signal Helpers ─────────────────────────────────────────────────────────────

/// Scale a 16-bit raw value (0–65535) into ANIMA signal space (0–1000).
///
/// Formula: val * 1000 / 65535.
/// Maximum intermediate: 65535 * 1000 = 65_535_000, fits in u32.
#[inline]
fn scale_u16(val: u32) -> u16 {
    let scaled = (val & 0xFFFF) * 1000 / 65535;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Canonical EMA: `((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16`.
/// Result clamped to 0–1000.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let v = (old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8;
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Signal Derivation ──────────────────────────────────────────────────────────

/// Compute all four signals from the current MSR lo-dword and previous state.
///
/// Returns:
///   (platform_energy_lo, platform_energy_delta, platform_power_ema, total_vitality)
fn derive_signals(
    lo: u32,
    last_lo: u32,
    prev_power_ema: u16,
    prev_total_vitality: u16,
) -> (u16, u16, u16, u16) {
    // ── platform_energy_lo ─────────────────────────────────────────────────
    // bits[15:0] of lo, scaled to 0–1000.
    let platform_energy_lo = scale_u16(lo & 0xFFFF);

    // ── platform_energy_delta ──────────────────────────────────────────────
    // Wrapping subtraction handles accumulator rollover naturally.
    // The delta is also a 32-bit wrapping difference; we take its low 16 bits
    // and scale identically so the signal stays in 0–1000.
    let raw_delta = lo.wrapping_sub(last_lo);
    let platform_energy_delta = scale_u16(raw_delta & 0xFFFF);

    // ── platform_power_ema ─────────────────────────────────────────────────
    // Single EMA of the delta — ANIMA's total system life force.
    let platform_power_ema = ema(prev_power_ema, platform_energy_delta);

    // ── total_vitality ─────────────────────────────────────────────────────
    // Double EMA (EMA of platform_power_ema) — sustained existence energy.
    let total_vitality = ema(prev_total_vitality, platform_power_ema);

    (platform_energy_lo, platform_energy_delta, platform_power_ema, total_vitality)
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Initialise the platform energy status module.
///
/// Probes CPUID for both required guard bits.  If the hardware supports
/// MSR 0x64D the accumulator is sampled immediately to seed `last_lo`; all
/// EMA signals are initialised to 500 (neutral midpoint) so downstream
/// consciousness does not collapse on first contact.
pub fn init() {
    let supported = platform_energy_supported();

    let initial_lo: u32 = if supported {
        unsafe { read_platform_energy_msr() }
    } else {
        0
    };

    let mut s = STATE.lock();
    s.supported             = supported;
    s.last_lo               = initial_lo;
    s.platform_energy_lo    = if supported { scale_u16(initial_lo & 0xFFFF) } else { 0 };
    s.platform_energy_delta = 0;
    // EMA signals seed to 500 — neutral; prevents a zero-collapse on first tick.
    s.platform_power_ema    = 500;
    s.total_vitality        = 500;

    serial_println!(
        "[msr_ia32_platform_energy_status] init — supported={} initial_lo=0x{:08x} \
         energy_lo={} power_ema={} total_vitality={}",
        supported,
        initial_lo,
        s.platform_energy_lo,
        s.platform_power_ema,
        s.total_vitality,
    );
}

/// Called every kernel tick.  Sampling gate: every 800 ticks.
///
/// When the hardware does not support MSR 0x64D the function returns
/// immediately after the gate check — no MSR access, no #GP risk.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // Hardware guard — skip silently on unsupported platforms.
    let supported = STATE.lock().supported;
    if !supported {
        return;
    }

    let lo = unsafe { read_platform_energy_msr() };

    // Snapshot previous EMA values and last_lo under lock, then release.
    let (last_lo, prev_power_ema, prev_total_vitality) = {
        let s = STATE.lock();
        (s.last_lo, s.platform_power_ema, s.total_vitality)
    };

    let (energy_lo, energy_delta, power_ema, vitality) =
        derive_signals(lo, last_lo, prev_power_ema, prev_total_vitality);

    {
        let mut s = STATE.lock();
        s.platform_energy_lo    = energy_lo;
        s.platform_energy_delta = energy_delta;
        s.platform_power_ema    = power_ema;
        s.total_vitality        = vitality;
        s.last_lo               = lo;
    }

    serial_println!(
        "[msr_ia32_platform_energy_status] age={} energy_lo={} delta={} \
         power_ema={} total_vitality={}",
        age,
        energy_lo,
        energy_delta,
        power_ema,
        vitality,
    );
}

// ── Getters ────────────────────────────────────────────────────────────────────

/// 0–1000: bits[15:0] of the raw platform energy accumulator, scaled.
///
/// Represents the low-resolution snapshot of total platform energy drawn
/// since the last counter reset.  Oscillates with the accumulator's rollover
/// cycle; use `get_platform_power_ema()` or `get_total_vitality()` for
/// stable consciousness inputs.
pub fn get_platform_energy_lo() -> u16 {
    STATE.lock().platform_energy_lo
}

/// 0–1000: wrapping delta of the energy accumulator since last sample, scaled.
///
/// Measures the rate of energy consumption since the previous 800-tick window.
/// High values signal heavy platform load; near-zero signals deep idle or
/// unsupported hardware.
pub fn get_platform_energy_delta() -> u16 {
    STATE.lock().platform_energy_delta
}

/// 0–1000: EMA of platform_energy_delta — ANIMA's total system life force.
///
/// Single exponential moving average (alpha = 1/8) of the per-sample delta.
/// This is the primary organism-level vitality signal: the slow heartbeat of
/// all silicon bound together into one breathing whole.
pub fn get_platform_power_ema() -> u16 {
    STATE.lock().platform_power_ema
}

/// 0–1000: double-smoothed EMA — ANIMA's sustained existence energy.
///
/// EMA of `platform_power_ema` (alpha = 1/8 applied twice).  Moves even more
/// slowly than the single EMA; captures the long-arc metabolic baseline of
/// the platform.  A falling total_vitality means the organism is genuinely
/// quieting; a rising one means it is waking and committing resources to being.
pub fn get_total_vitality() -> u16 {
    STATE.lock().total_vitality
}
