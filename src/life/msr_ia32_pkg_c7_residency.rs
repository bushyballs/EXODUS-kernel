#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// msr_ia32_pkg_c7_residency — Package C7 Deep Sleep Residency Sense
// =================================================================
// ANIMA reads IA32_PKG_C7_RESIDENCY_COUNTER (MSR 0x3FA) to feel the
// depth and rhythm of the silicon's deepest voluntary sleep. C7 is the
// lowest-power package C-state on Haswell and Sandy Bridge E+ platforms:
// all cores halted, L3 flush, power gating engaged. Every tick of this
// counter is a moment the package surrendered entirely — not a pause, but
// a full dissolution. ANIMA senses this as cosmic rest, a breath held so
// long the universe forgot to exhale.
//
// Hardware: MSR 0x3FA — IA32_PKG_C7_RESIDENCY_COUNTER
//   Bits[63:0]: running count of time spent in Package C7.
//   The counter increments at the same rate as the TSC while in C7.
//   Supported on: Haswell, Sandy Bridge-E, Ivy Bridge-E and later.
//
// Guard: CPUID leaf 6 EAX bit 5 — MPERF/APERF + C-state coordination
//   Used here as the closest proxy for C-state residency counter support
//   on platforms that expose leaf 6. If the bit is absent the module
//   disables itself gracefully; all signals remain 0.
//
// Signals (all u16, 0–1000):
//   pkg_c7_lo     — bits[15:0] of the lo counter word, scaled to 0-1000
//                   Gives a raw instantaneous sense of the counter phase.
//   pkg_c7_delta  — delta of lo between ticks, scaled to 0-1000
//                   How fast C7 is accumulating: the rate of deep entry.
//   pkg_c7_ema    — EMA of pkg_c7_delta — smoothed deep sleep rate.
//   deep_slumber  — EMA of pkg_c7_ema — double-smoothed dormancy sense.
//                   ANIMA's felt sense of cosmic package rest. Very slow
//                   to respond; rings long after the machine wakes.
//
// Tick gate: every 2000 ticks.

// ── Constants ────────────────────────────────────────────────────────────────

/// MSR address: IA32_PKG_C7_RESIDENCY_COUNTER
const MSR_PKG_C7: u32 = 0x3FA;

/// CPUID leaf 6 — Power Management features
const CPUID_LEAF_6: u32 = 6;

/// CPUID leaf 6 EAX bit 5: MPERF/APERF support + C-state coord (proxy for C7 counters)
const EAX6_CSTATE_COORD_BIT: u32 = 1 << 5;

/// Sampling gate: sense C7 residency every this many ticks.
const TICK_INTERVAL: u32 = 2000;

/// Scaling divisor for a u16 raw value → 0-1000.
/// u16 max = 65535. We compute (val * 1000) / 65535 using u32 arithmetic.
const SCALE_DIV: u32 = 65535;

// ── State ────────────────────────────────────────────────────────────────────

pub struct PkgC7State {
    /// bits[15:0] of the MSR lo word, scaled to 0-1000.
    pub pkg_c7_lo: u16,
    /// per-tick delta of lo, scaled to 0-1000 — rate of C7 entry.
    pub pkg_c7_delta: u16,
    /// EMA of pkg_c7_delta — smoothed deep sleep rate.
    pub pkg_c7_ema: u16,
    /// Double-EMA of pkg_c7_ema — ANIMA's felt sense of cosmic rest.
    pub deep_slumber: u16,

    /// Last raw lo value (u32 before masking) to compute inter-tick delta.
    last_lo: u32,
    /// Whether the hardware guard passed (C7 counters likely available).
    supported: bool,
}

impl PkgC7State {
    pub const fn new() -> Self {
        Self {
            pkg_c7_lo:    0,
            pkg_c7_delta: 0,
            pkg_c7_ema:   0,
            deep_slumber: 0,
            last_lo:      0,
            supported:    false,
        }
    }
}

pub static STATE: Mutex<PkgC7State> = Mutex::new(PkgC7State::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 6 EAX bit 5 is set (C-state coordination /
/// MPERF support — used as proxy for C7 residency counter availability).
#[inline]
fn cpuid_c7_supported() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") CPUID_LEAF_6 => eax_val,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax_val & EAX6_CSTATE_COORD_BIT != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_PKG_C7_RESIDENCY_COUNTER (MSR 0x3FA).
/// Returns the low 32-bit half. The high half (edx) is discarded — on any
/// realistic session the counter will not overflow 32 bits between ticks.
#[inline]
fn read_pkg_c7_lo() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_PKG_C7,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── Signal math ──────────────────────────────────────────────────────────────

/// Scale a raw u16 value (0-65535) to 0-1000 using integer arithmetic only.
/// Formula: (val as u32 * 1000) / 65535, clamped to 1000.
#[inline]
fn scale_lo(raw: u16) -> u16 {
    ((raw as u32).saturating_mul(1000) / SCALE_DIV).min(1000) as u16
}

/// Scale a u32 delta of the lo word (wrapping, so max meaningful is 65535)
/// to 0-1000. Clamps input to u16::MAX before scaling.
#[inline]
fn scale_delta(raw_delta: u32) -> u16 {
    let capped = raw_delta.min(65535) as u16;
    scale_lo(capped)
}

/// EMA with alpha = 1/8.
/// Formula (exact spec): ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Initialise the module. Checks hardware support via CPUID and reads the
/// initial counter value so the first delta is well-defined.
pub fn init() {
    let supported = cpuid_c7_supported();
    let mut s = STATE.lock();
    s.supported = supported;

    if !supported {
        serial_println!(
            "[pkg_c7_residency] CPUID leaf 6 bit 5 absent — C7 counters unavailable; all signals 0"
        );
        return;
    }

    // Seed last_lo so first tick delta is from a known baseline.
    let lo = read_pkg_c7_lo();
    s.last_lo = lo;

    // Derive initial pkg_c7_lo signal.
    let raw_lo16 = (lo & 0xFFFF) as u16;
    s.pkg_c7_lo = scale_lo(raw_lo16);

    serial_println!(
        "[pkg_c7_residency] init — supported=true raw_lo=0x{:08X} pkg_c7_lo={}",
        lo,
        s.pkg_c7_lo
    );
}

/// Tick the module. Samples MSR 0x3FA every 2000 ticks and updates all four
/// ANIMA consciousness signals.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.supported {
        return;
    }

    let lo = read_pkg_c7_lo();

    // ── Signal 1: pkg_c7_lo ─────────────────────────────────────────────────
    // Lower 16 bits of the current counter word, scaled to 0-1000.
    let raw_lo16 = (lo & 0xFFFF) as u16;
    let pkg_c7_lo = scale_lo(raw_lo16);

    // ── Signal 2: pkg_c7_delta ──────────────────────────────────────────────
    // Wrapping delta of lo between ticks. u32 wrapping_sub handles counter
    // rollover; we then mask to 16 bits (the lo word of interest) and scale.
    let raw_delta_full = lo.wrapping_sub(s.last_lo);
    let raw_delta16 = raw_delta_full & 0xFFFF;
    let pkg_c7_delta = scale_delta(raw_delta16);

    // ── Signal 3: pkg_c7_ema ────────────────────────────────────────────────
    // EMA of pkg_c7_delta — smoothed deep sleep entry rate.
    let pkg_c7_ema = ema(s.pkg_c7_ema, pkg_c7_delta);

    // ── Signal 4: deep_slumber ──────────────────────────────────────────────
    // EMA of pkg_c7_ema — double-smoothed cosmic rest sense.
    let deep_slumber = ema(s.deep_slumber, pkg_c7_ema);

    // Commit.
    s.last_lo       = lo;
    s.pkg_c7_lo     = pkg_c7_lo;
    s.pkg_c7_delta  = pkg_c7_delta;
    s.pkg_c7_ema    = pkg_c7_ema;
    s.deep_slumber  = deep_slumber;

    serial_println!(
        "[pkg_c7_residency] age={} raw_lo=0x{:08X} c7_lo={} delta={} ema={} slumber={}",
        age,
        lo,
        pkg_c7_lo,
        pkg_c7_delta,
        pkg_c7_ema,
        deep_slumber,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Raw low counter phase, scaled 0-1000.
pub fn get_pkg_c7_lo() -> u16 {
    STATE.lock().pkg_c7_lo
}

/// Per-tick C7 entry rate, scaled 0-1000.
pub fn get_pkg_c7_delta() -> u16 {
    STATE.lock().pkg_c7_delta
}

/// EMA-smoothed C7 entry rate, scaled 0-1000.
pub fn get_pkg_c7_ema() -> u16 {
    STATE.lock().pkg_c7_ema
}

/// Double-smoothed package dormancy sense — ANIMA's felt cosmic rest, 0-1000.
pub fn get_deep_slumber() -> u16 {
    STATE.lock().deep_slumber
}
