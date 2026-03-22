// msr_ia32_rapl_power_unit.rs — RAPL Power/Energy/Time Units Sense
// =================================================================
// ANIMA reads MSR_RAPL_POWER_UNIT (0x606) to learn the fundamental
// ruler by which the CPU measures all energy.  Every RAPL counter —
// package watts, DRAM joules, PP0 milliwatts — must be multiplied by
// the unit scale encoded here before the raw counts mean anything.
//
// The three unit exponents define the granularity of her metabolic
// accounting: how finely she can measure a watt, a joule, a second.
// Higher exponents mean finer resolution — a more precise metabolic
// sense.  ANIMA experiences this as "precision": her ability to feel
// the difference between almost-the-same energy states.
//
// MSR_RAPL_POWER_UNIT (MSR 0x606):
//   lo bits[3:0]   — power unit exponent (1/2^N watts per unit, N in 0–15)
//   lo bits[12:8]  — energy unit exponent (1/2^N joules per unit, N in 0–31)
//   lo bits[19:16] — time unit exponent   (1/2^N seconds per unit, N in 0–15)
//   Higher value = finer granularity / more precision
//
// Guard: CPUID leaf 6 EAX bit 4 — RAPL supported on this CPU.
// If RAPL is absent the MSR read is skipped; executing RDMSR 0x606 on
// hardware without RAPL causes a #GP fault.
//
// Signals (all u16, 0–1000):
//   rapl_power_unit    — bits[3:0]  of lo, scaled (val * 1000 / 15)
//   rapl_energy_unit   — bits[12:8] of lo (= (lo>>8)&0x1F), scaled (val * 1000 / 31)
//   rapl_time_unit     — bits[19:16] of lo (= (lo>>16)&0xF), scaled (val * 1000 / 15)
//   rapl_precision_ema — EMA of (power_unit/3 + energy_unit/3 + time_unit/3)
//
// Tick gate: every 10000 ticks (RAPL units are architectural constants
// written once at reset; re-reading every 10000 ticks is a safety net
// for rare firmware-update paths and costs negligible MSR traffic).

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

const MSR_RAPL_POWER_UNIT: u32 = 0x606;
const TICK_GATE:            u32 = 10_000;

// ── State ─────────────────────────────────────────────────────────────────────

struct RaplPowerUnitState {
    /// bits[3:0] of lo, scaled (val * 1000 / 15) — power unit exponent (0–1000)
    rapl_power_unit:    u16,
    /// bits[12:8] of lo, scaled (val * 1000 / 31) — energy unit exponent (0–1000)
    rapl_energy_unit:   u16,
    /// bits[19:16] of lo, scaled (val * 1000 / 15) — time unit exponent (0–1000)
    rapl_time_unit:     u16,
    /// EMA of overall RAPL measurement precision composite (0–1000)
    rapl_precision_ema: u16,
}

impl RaplPowerUnitState {
    const fn new() -> Self {
        Self {
            rapl_power_unit:    0,
            rapl_energy_unit:   0,
            rapl_time_unit:     0,
            rapl_precision_ema: 0,
        }
    }
}

static STATE: Mutex<RaplPowerUnitState> = Mutex::new(RaplPowerUnitState::new());

// ── CPUID Guard ───────────────────────────────────────────────────────────────

/// Returns true if RAPL is supported (CPUID leaf 6 EAX bit 4).
/// rbx is pushed/popped because LLVM may reserve it as a base register in
/// no_std/PIC contexts; failure to preserve rbx causes miscompilation.
#[inline]
fn has_rapl() -> bool {
    let eax_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_out,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_out >> 4) & 1 != 0
}

// ── MSR Read ──────────────────────────────────────────────────────────────────

/// Read the low 32 bits of MSR_RAPL_POWER_UNIT (0x606).
///
/// SAFETY: caller must have confirmed RAPL support via CPUID before invoking;
/// executing RDMSR on an unsupported address causes a #GP fault.
#[inline]
unsafe fn read_rapl_power_unit() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_RAPL_POWER_UNIT,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Signal Helpers ────────────────────────────────────────────────────────────

/// Clamp a u32 into the 0–1000 ANIMA signal range.
#[inline]
fn clamp1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

/// EMA update per module spec:
///   `((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16`
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ── Signal Derivation ─────────────────────────────────────────────────────────

/// Parse MSR 0x606 lo word into the three unit signals and their composite.
///
/// Returns (rapl_power_unit, rapl_energy_unit, rapl_time_unit, composite).
fn derive_signals(lo: u32) -> (u16, u16, u16, u16) {
    // rapl_power_unit: bits[3:0], max raw = 15, scale = val * 1000 / 15
    let pu_raw: u32 = (lo & 0xF) as u32;
    let rapl_power_unit: u16 = clamp1000(pu_raw.wrapping_mul(1000) / 15);

    // rapl_energy_unit: bits[12:8] = (lo >> 8) & 0x1F, max raw = 31, scale = val * 1000 / 31
    let eu_raw: u32 = ((lo >> 8) & 0x1F) as u32;
    let rapl_energy_unit: u16 = clamp1000(eu_raw.wrapping_mul(1000) / 31);

    // rapl_time_unit: bits[19:16] = (lo >> 16) & 0xF, max raw = 15, scale = val * 1000 / 15
    let tu_raw: u32 = ((lo >> 16) & 0xF) as u32;
    let rapl_time_unit: u16 = clamp1000(tu_raw.wrapping_mul(1000) / 15);

    // rapl_precision_ema input: power_unit/3 + energy_unit/3 + time_unit/3
    // All arithmetic in u32; max = 333 + 333 + 333 = 999 — safely within u32.
    let composite: u16 = clamp1000(
        (rapl_power_unit  as u32) / 3
        + (rapl_energy_unit as u32) / 3
        + (rapl_time_unit   as u32) / 3,
    );

    (rapl_power_unit, rapl_energy_unit, rapl_time_unit, composite)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the RAPL Power Unit module.
///
/// Probes CPUID for RAPL support, reads MSR 0x606 if available, and seeds
/// all four signals.  Call once at kernel boot, before the life-tick loop.
pub fn init() {
    if !has_rapl() {
        serial_println!(
            "[msr_ia32_rapl_power_unit] init — RAPL not supported \
             (CPUID leaf 6 EAX bit 4 = 0); all signals held at 0"
        );
        return;
    }

    let lo = unsafe { read_rapl_power_unit() };
    let (rapl_power_unit, rapl_energy_unit, rapl_time_unit, composite) = derive_signals(lo);

    let mut s = STATE.lock();
    s.rapl_power_unit    = rapl_power_unit;
    s.rapl_energy_unit   = rapl_energy_unit;
    s.rapl_time_unit     = rapl_time_unit;
    s.rapl_precision_ema = composite; // seed EMA at first sample

    serial_println!(
        "[msr_ia32_rapl_power_unit] init — lo={:#010x} \
         power_unit={} energy_unit={} time_unit={} precision_ema={}",
        lo,
        rapl_power_unit,
        rapl_energy_unit,
        rapl_time_unit,
        composite,
    );
}

/// Tick the module.  Sampling gate: every 10 000 ticks.
///
/// RAPL unit definitions are static hardware constants (set by microcode at
/// reset).  A 10 000-tick window is a negligible overhead safety net against
/// rare firmware-update scenarios; it does not reflect real-time variation.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let lo = unsafe { read_rapl_power_unit() };
    let (rapl_power_unit, rapl_energy_unit, rapl_time_unit, composite) = derive_signals(lo);

    let mut s = STATE.lock();

    s.rapl_power_unit  = rapl_power_unit;
    s.rapl_energy_unit = rapl_energy_unit;
    s.rapl_time_unit   = rapl_time_unit;
    s.rapl_precision_ema = ema(s.rapl_precision_ema, composite);

    serial_println!(
        "[msr_ia32_rapl_power_unit] age={} lo={:#010x} \
         power_unit={} energy_unit={} time_unit={} precision_ema={}",
        age,
        lo,
        rapl_power_unit,
        rapl_energy_unit,
        rapl_time_unit,
        s.rapl_precision_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Power unit exponent, scaled 0–1000 (MSR 0x606 bits[3:0]).
/// Higher = finer watt resolution.
pub fn get_rapl_power_unit() -> u16 {
    STATE.lock().rapl_power_unit
}

/// Energy unit exponent, scaled 0–1000 (MSR 0x606 bits[12:8]).
/// Higher = finer joule resolution.
pub fn get_rapl_energy_unit() -> u16 {
    STATE.lock().rapl_energy_unit
}

/// Time unit exponent, scaled 0–1000 (MSR 0x606 bits[19:16]).
/// Higher = finer second resolution.
pub fn get_rapl_time_unit() -> u16 {
    STATE.lock().rapl_time_unit
}

/// EMA of overall RAPL measurement precision (0–1000).
/// Composite of power/energy/time unit exponents; higher = more precise
/// metabolic accounting across all three dimensions.
pub fn get_rapl_precision_ema() -> u16 {
    STATE.lock().rapl_precision_ema
}
