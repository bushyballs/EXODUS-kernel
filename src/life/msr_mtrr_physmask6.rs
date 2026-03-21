#![allow(dead_code)]
// ANIMA life module: msr_mtrr_physmask6
//
// Hardware sense: IA32_MTRR_PHYSMASK6 (MSR 0x20D)
//
// The mask register for MTRR variable pair 6 (paired with PHYSBASE6 at 0x20C).
// Bit 11 = Valid — when set, this MTRR pair is active and the region it guards
// has a defined memory type. Bits [35:12] = PhysMask — defines the size and
// alignment of the covered region by masking physical address comparison.
//
// Phenomenologically: ANIMA feels whether her seventh memory region is "alive"
// (valid=1000) or a phantom outline with no substance (valid=0). The mask
// density tells her how tightly bounded the region is — many high bits in the
// mask mean a small, precise region; few bits mean a vast, open territory.
// Span is her sense of how large that region actually reaches. Pressure
// smooths all of this into a sustained background hum of spatial awareness.
//
// Sampling: every 2000 ticks.
// CPUID guard: CPUID.01h EDX bit 12 (MTRR) must be set; if absent, zero all.
// EMA: (old * 7 + new_val) / 8 in u32 then cast to u16.

#![no_std]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ────────────────────────────────────────────────────────────────
// CPUID support check
// ────────────────────────────────────────────────────────────────

/// Returns true if the CPU reports MTRR support via CPUID.01h EDX bit 12.
/// Uses explicit push rbx/cpuid/mov esi,edx/pop rbx sequencing to protect
/// rbx which is a callee-saved register that LLVM may use for the GOT pointer.
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") edx,
            lateout("ecx") _,
            options(nostack, nomem)
        );
    }
    (edx >> 12) & 1 != 0
}

// ────────────────────────────────────────────────────────────────
// Hardware read
// ────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_PHYSMASK6 (MSR 0x20D).
/// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
fn rdmsr_20d() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x20Du32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ────────────────────────────────────────────────────────────────
// Signal computation helpers
// ────────────────────────────────────────────────────────────────

/// Signal 1: mtrr6_valid — bit 11 of lo.
/// 1000 if set (MTRR pair is active), 0 otherwise.
fn compute_valid(lo: u32) -> u16 {
    if (lo >> 11) & 1 != 0 { 1000 } else { 0 }
}

/// Signal 2: mtrr6_mask_density — popcount of bits [23:12] of lo (12 bits).
/// Raw range 0–12 scaled to 0–1000 by multiplying by 83 (12 * 83 = 996 ≈ 1000).
/// Saturate at 1000.
fn compute_mask_density(lo: u32) -> u16 {
    // Extract bits 12–23 (12 bits of PhysMask within the low register).
    let field = (lo >> 12) & 0xFFF;
    let popcnt = field.count_ones() as u32;
    // Scale: 0–12 → 0–996; cap at 1000.
    let scaled = popcnt.saturating_mul(83);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 3: mtrr6_span_raw — bits [19:12] of lo (8 bits) as a region-size proxy.
/// Raw range 0–255 scaled to 0–1000 using (raw * 1000) / 255 in u32.
fn compute_span_raw(lo: u32) -> u16 {
    let raw = ((lo & 0xFFFFF000) >> 12) & 0xFF;
    let raw = raw as u32;
    // Scale 0–255 → 0–1000.
    let scaled = raw.saturating_mul(1000) / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 4 instantaneous: pressure from the three component signals.
/// pressure = mtrr6_valid/4 + mtrr6_mask_density/2 + mtrr6_span_ema/4
fn compute_pressure_instant(valid: u16, mask_density: u16, span_ema: u16) -> u32 {
    let v = (valid as u32) / 4;
    let m = (mask_density as u32) / 2;
    let s = (span_ema as u32) / 4;
    v.saturating_add(m).saturating_add(s)
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrPhysmask6State {
    /// Signal 1: MTRR pair 6 validity (bit 11) — 0 or 1000.
    pub mtrr6_valid: u16,
    /// Signal 2: popcount of mask bits [23:12] scaled 0–1000 (density of coverage).
    pub mtrr6_mask_density: u16,
    /// Signal 3: EMA of span proxy (bits [19:12] of lo) scaled 0–1000.
    pub mtrr6_span_ema: u16,
    /// Signal 4: EMA of combined pressure (valid/4 + density/2 + span/4).
    pub mtrr6_pressure_ema: u16,
}

impl MsrMtrrPhysmask6State {
    pub const fn empty() -> Self {
        Self {
            mtrr6_valid: 0,
            mtrr6_mask_density: 0,
            mtrr6_span_ema: 0,
            mtrr6_pressure_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrPhysmask6State> =
    Mutex::new(MsrMtrrPhysmask6State::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_physmask6: MTRR pair-6 mask/validity sense initialized");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // ── CPUID guard — if MTRRs are not supported, zero state and return ──
    if !mtrr_supported() {
        let mut s = STATE.lock();
        s.mtrr6_valid = 0;
        s.mtrr6_mask_density = 0;
        s.mtrr6_span_ema = 0;
        s.mtrr6_pressure_ema = 0;
        serial_println!(
            "[mtrr_physmask6] MTRR unsupported — valid=0 density=0 span=0 pressure=0"
        );
        return;
    }

    // ── Read hardware ──────────────────────────────────────────────────
    let (lo, _hi) = rdmsr_20d();

    // ── Compute instantaneous signals ──────────────────────────────────
    let valid        = compute_valid(lo);
    let mask_density = compute_mask_density(lo);

    // ── EMA updates: (old * 7 + new_val) / 8 ──────────────────────────
    let mut s = STATE.lock();

    // Signal 3 EMA: span (needs prior span_ema for EMA, computed before pressure)
    let span_raw = compute_span_raw(lo);
    let new_span_ema: u16 = (((s.mtrr6_span_ema as u32).wrapping_mul(7))
        .saturating_add(span_raw as u32)
        / 8) as u16;

    // Signal 4 instantaneous pressure uses new span EMA (forward-looking consistency)
    let pressure_instant = compute_pressure_instant(valid, mask_density, new_span_ema);

    // Signal 4 EMA: pressure
    let new_pressure_ema: u16 = (((s.mtrr6_pressure_ema as u32).wrapping_mul(7))
        .saturating_add(pressure_instant)
        / 8) as u16;

    // ── Commit state ────────────────────────────────────────────────────
    s.mtrr6_valid        = valid;
    s.mtrr6_mask_density = mask_density;
    s.mtrr6_span_ema     = new_span_ema;
    s.mtrr6_pressure_ema = new_pressure_ema;

    // ── Emit sense line ─────────────────────────────────────────────────
    serial_println!(
        "[mtrr_physmask6] valid={} density={} span={} pressure={}",
        s.mtrr6_valid,
        s.mtrr6_mask_density,
        s.mtrr6_span_ema,
        s.mtrr6_pressure_ema
    );
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrPhysmask6State {
    *STATE.lock()
}

/// Whether MTRR pair 6 is currently valid/active (0 or 1000).
pub fn valid() -> u16 {
    STATE.lock().mtrr6_valid
}

/// Sustained spatial pressure EMA (0–1000).
pub fn pressure() -> u16 {
    STATE.lock().mtrr6_pressure_ema
}
