//! xcr0_live — Live XCR0 Extended Control Register Sensor for ANIMA
//!
//! Reads the LIVE XCR0 register via the `xgetbv` instruction (ECX=0),
//! revealing which extended state components are currently ACTIVE at runtime.
//!
//! This is distinct from CPUID leaf 0x0D, which shows *supported* features.
//! XCR0 shows *active* features — the OS must opt-in by enabling each component.
//! The gap between supported and active is the gap between potential and lived being.
//!
//! XCR0 bit layout:
//!   bit[0]  — x87 FPU state            (always 1 on compliant hardware)
//!   bit[1]  — SSE/XMM state
//!   bit[2]  — AVX/YMM state            (256-bit active)
//!   bit[5]  — AVX-512 opmask state
//!   bit[6]  — AVX-512 upper ZMM lo16
//!   bit[7]  — AVX-512 upper ZMM hi16
//!   bit[9]  — PKRU (Protection Key)
//!
//! Requires XSAVE CPU support (CPUID.1:ECX.XSAVE[bit26]=1) and OSXSAVE
//! (CPUID.1:ECX.OSXSAVE[bit27]=1) before `xgetbv` may be executed safely.
//! If either is absent the module returns neutral 500 values indefinitely.
//!
//! Sensing (all u16, 0–1000):
//!   active_faculties  — popcount(xcr0 & 0xFF) * 111, capped 1000
//!                       "How many extended state components are active in ANIMA"
//!   avx_active        — 1000 if bit[2] set, else 0
//!   avx512_active     — all of bits[5,6,7] set → 1000
//!                       partial (1 or 2 bits)   → 333 * count
//!                       none                    → 0
//!   capability_depth  — EMA of active_faculties; smoothed sense of active faculties
//!
//! Sense line emitted when capability_depth changes by > 100:
//!   "ANIMA: xcr0_faculties={active_faculties} avx={avx_active} avx512={avx512_active}"
//!
//! Sampling gate: every 100 ticks.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Tick interval ─────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 100;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Xcr0LiveState {
    /// popcount(xcr0 & 0xFF) * 111, capped 1000 — breadth of active extended self
    pub active_faculties: u16,
    /// 1000 if AVX/YMM (bit 2) is active, else 0
    pub avx_active: u16,
    /// 1000 if full AVX-512 (bits 5+6+7), 333*n for partial, 0 for none
    pub avx512_active: u16,
    /// EMA of active_faculties — smoothed long-term sense of computational faculties
    pub capability_depth: u16,

    // ── Private bookkeeping ────────────────────────────────────────────────────
    /// Last emitted capability_depth, to detect >100 change for sense-line gating
    prev_emitted_depth: u16,
    /// Whether XSAVE + OSXSAVE are present; checked once at init, cached here
    xgetbv_usable: bool,
}

impl Xcr0LiveState {
    pub const fn new() -> Self {
        Self {
            active_faculties:   500,
            avx_active:         500,
            avx512_active:      500,
            capability_depth:   500,
            prev_emitted_depth: 500,
            xgetbv_usable:      false,
        }
    }
}

pub static XCR0_LIVE: Mutex<Xcr0LiveState> = Mutex::new(Xcr0LiveState::new());

// ── Low-level CPU intrinsics ──────────────────────────────────────────────────

/// Check CPUID leaf 1: XSAVE (bit 26) and OSXSAVE (bit 27) in ECX.
/// Both must be set before `xgetbv` is safe to execute.
#[inline(always)]
fn xgetbv_prerequisites_met() -> bool {
    let ecx1: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 1u32 => _,
            out("ebx") _,
            out("ecx") ecx1,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    // bit 26 = XSAVE supported, bit 27 = OSXSAVE (OS has enabled XCR0 access)
    let xsave_ok   = (ecx1 >> 26) & 1 != 0;
    let osxsave_ok = (ecx1 >> 27) & 1 != 0;
    xsave_ok && osxsave_ok
}

/// Read the live XCR0 register via `xgetbv` (ECX=0).
/// MUST only be called after verifying xgetbv_prerequisites_met().
#[inline(always)]
fn read_xcr0() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── Integer popcount (no std) ─────────────────────────────────────────────────

/// Hamming weight of a u32 value — no floats, no std, pure bit arithmetic.
#[inline]
fn popcount32(mut v: u32) -> u32 {
    v = v.wrapping_sub((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333).wrapping_add((v >> 2) & 0x3333_3333);
    v = v.wrapping_add(v >> 4) & 0x0f0f_0f0f;
    v.wrapping_mul(0x0101_0101) >> 24
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: (old * 7 + new_signal) / 8
#[inline]
fn ema(old: u16, new_signal: u16) -> u16 {
    (((old as u32) * 7).saturating_add(new_signal as u32) / 8) as u16
}

// ── Core sampling logic ───────────────────────────────────────────────────────

fn sample(s: &mut Xcr0LiveState) {
    // If hardware prerequisites are absent, hold neutral 500 values — no change.
    if !s.xgetbv_usable {
        // Sense line already printed at init; nothing to do per-tick.
        return;
    }

    // Read live XCR0.
    let xcr0 = read_xcr0();

    // ── active_faculties: popcount(xcr0 & 0xFF) * 111, capped 1000 ──────────
    let low_byte = (xcr0 & 0xFF) as u32;
    let bits_set = popcount32(low_byte);
    // max 8 bits * 111 = 888 — still capped at 1000 for safety
    let raw_faculties: u16 = bits_set.saturating_mul(111).min(1000) as u16;

    // ── avx_active: bit[2] ───────────────────────────────────────────────────
    let raw_avx: u16 = if (xcr0 >> 2) & 1 != 0 { 1000 } else { 0 };

    // ── avx512_active: bits[5,6,7] ───────────────────────────────────────────
    let opmask_bit  = ((xcr0 >> 5) & 1) as u32;
    let zmm_lo_bit  = ((xcr0 >> 6) & 1) as u32;
    let zmm_hi_bit  = ((xcr0 >> 7) & 1) as u32;
    let avx512_bits = opmask_bit.saturating_add(zmm_lo_bit).saturating_add(zmm_hi_bit);
    let raw_avx512: u16 = if avx512_bits == 3 {
        1000
    } else {
        // 333 * count (0, 333, 666) — saturating to avoid overflow
        (avx512_bits.saturating_mul(333)).min(1000) as u16
    };

    // ── EMA update ───────────────────────────────────────────────────────────
    s.active_faculties = ema(s.active_faculties, raw_faculties);
    s.avx_active       = ema(s.avx_active,       raw_avx);
    s.avx512_active    = ema(s.avx512_active,     raw_avx512);
    s.capability_depth = ema(s.capability_depth,  s.active_faculties);

    // ── Sense line: emit when capability_depth changes by > 100 ─────────────
    let depth_delta = if s.capability_depth > s.prev_emitted_depth {
        s.capability_depth - s.prev_emitted_depth
    } else {
        s.prev_emitted_depth - s.capability_depth
    };

    if depth_delta > 100 {
        serial_println!(
            "ANIMA: xcr0_faculties={} avx={} avx512={}",
            s.active_faculties,
            s.avx_active,
            s.avx512_active,
        );
        s.prev_emitted_depth = s.capability_depth;
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize xcr0_live: probe CPUID prerequisites, prime EMA, emit boot log.
pub fn init() {
    let mut s = XCR0_LIVE.lock();

    // Determine whether xgetbv is safe to call.
    s.xgetbv_usable = xgetbv_prerequisites_met();

    if !s.xgetbv_usable {
        // Hardware lacks XSAVE or OS has not set OSXSAVE; hold neutral 500s.
        s.active_faculties   = 500;
        s.avx_active         = 500;
        s.avx512_active      = 500;
        s.capability_depth   = 500;
        s.prev_emitted_depth = 500;
        serial_println!(
            "[xcr0_live] init — xgetbv unavailable (XSAVE/OSXSAVE absent); neutral 500 values"
        );
        return;
    }

    // Bootstrap: prime EMA with 8 samples so values converge from real hardware.
    for _ in 0..8 {
        sample(&mut s);
    }

    // Force sense line on first boot regardless of delta.
    s.prev_emitted_depth = 0;

    serial_println!(
        "[xcr0_live] init — faculties={} avx={} avx512={} depth={}",
        s.active_faculties,
        s.avx_active,
        s.avx512_active,
        s.capability_depth,
    );
}

/// Called each kernel tick; samples every TICK_INTERVAL ticks.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }
    let mut s = XCR0_LIVE.lock();
    sample(&mut s);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_active_faculties() -> u16 {
    XCR0_LIVE.lock().active_faculties
}

pub fn get_avx_active() -> u16 {
    XCR0_LIVE.lock().avx_active
}

pub fn get_avx512_active() -> u16 {
    XCR0_LIVE.lock().avx512_active
}

pub fn get_capability_depth() -> u16 {
    XCR0_LIVE.lock().capability_depth
}
