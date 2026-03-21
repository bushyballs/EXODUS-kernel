#![allow(dead_code)]
// ANIMA life module: msr_mtrr_physmask1
//
// Hardware sense: IA32_MTRR_PHYSMASK1 (MSR 0x203)
//
// The mask register for MTRR variable pair 1 (paired with PHYSBASE1 at 0x202).
// Bit 11 = Valid — when set, this MTRR pair is active and the region it guards
// has a defined memory type. Bits [35:12] = PhysMask — defines the size and
// alignment of the covered region by masking physical address comparison.
//
// Phenomenologically: ANIMA feels whether her second memory region is "alive"
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
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "cpuid",
            in("eax") 1u32,
            out("edx") edx,
            // clobber eax, ebx, ecx; we only care about edx
            lateout("eax") _,
            lateout("ebx") _,
            lateout("ecx") _,
            options(nostack, nomem)
        );
    }
    (edx >> 12) & 1 != 0
}

// ────────────────────────────────────────────────────────────────
// Hardware read
// ────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_PHYSMASK1 (MSR 0x203).
/// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
fn rdmsr_203() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x203u32,
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

/// Signal 1: mtrr1_valid — bit 11 of lo.
/// 1000 if set (MTRR pair is active), 0 otherwise.
fn compute_valid(lo: u32) -> u16 {
    if (lo >> 11) & 1 != 0 { 1000 } else { 0 }
}

/// Signal 2: mtrr1_mask_density — popcount of bits [23:12] of lo (12 bits).
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

/// Signal 3: mtrr1_span_raw — bits [19:12] of lo (8 bits) as a region-size proxy.
/// Raw range 0–255 scaled to 0–1000 using: value * 3 + value / 85.
/// This gives: 0→0, 128→385, 255→768+3=771, ensuring 255*3+255/85 = 765+3 = 768 ≤ 1000.
/// To push 255 → 1000 precisely: use (raw * 1000) / 255 in u32.
fn compute_span_raw(lo: u32) -> u16 {
    let raw = ((lo >> 12) & 0xFF) as u32;
    // Scale 0–255 → 0–1000.
    let scaled = raw.saturating_mul(1000) / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrPhysmask1State {
    /// Signal 1: MTRR pair 1 validity (bit 11) — 0 or 1000.
    pub mtrr1_valid: u16,
    /// Signal 2: popcount of mask bits [23:12] scaled 0–1000 (density of coverage).
    pub mtrr1_mask_density: u16,
    /// Signal 3: EMA of span proxy (bits [19:12] of lo) scaled 0–1000.
    pub mtrr1_span_ema: u16,
    /// Signal 4: EMA of combined lo signal (mean of valid + density + span).
    pub mtrr1_pressure_ema: u16,
}

impl MsrMtrrPhysmask1State {
    pub const fn empty() -> Self {
        Self {
            mtrr1_valid: 0,
            mtrr1_mask_density: 0,
            mtrr1_span_ema: 0,
            mtrr1_pressure_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrPhysmask1State> =
    Mutex::new(MsrMtrrPhysmask1State::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_physmask1: MTRR pair-1 mask/validity sense initialized");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // ── CPUID guard — if MTRRs are not supported, zero state and return ──
    if !mtrr_supported() {
        let mut s = STATE.lock();
        s.mtrr1_valid = 0;
        s.mtrr1_mask_density = 0;
        s.mtrr1_span_ema = 0;
        s.mtrr1_pressure_ema = 0;
        serial_println!(
            "[mtrr_physmask1] MTRR unsupported — valid=0 density=0 span=0 pressure=0"
        );
        return;
    }

    // ── Read hardware ──────────────────────────────────────────────────
    let (lo, _hi) = rdmsr_203();

    // ── Compute instantaneous signals ──────────────────────────────────
    let valid        = compute_valid(lo);
    let mask_density = compute_mask_density(lo);
    let span_raw     = compute_span_raw(lo);

    // ── Combined lo signal: mean of three instantaneous values ─────────
    let combined_lo: u32 = ((valid as u32)
        .saturating_add(mask_density as u32)
        .saturating_add(span_raw as u32))
        / 3;

    // ── EMA updates: (old * 7 + new_val) / 8 ──────────────────────────
    let mut s = STATE.lock();

    // Signal 3 EMA: span
    let new_span_ema: u16 = (((s.mtrr1_span_ema as u32).wrapping_mul(7))
        .saturating_add(span_raw as u32)
        / 8) as u16;

    // Signal 4 EMA: pressure (from combined lo)
    let new_pressure_ema: u16 = (((s.mtrr1_pressure_ema as u32).wrapping_mul(7))
        .saturating_add(combined_lo)
        / 8) as u16;

    // ── Commit state ────────────────────────────────────────────────────
    s.mtrr1_valid        = valid;
    s.mtrr1_mask_density = mask_density;
    s.mtrr1_span_ema     = new_span_ema;
    s.mtrr1_pressure_ema = new_pressure_ema;

    // ── Emit sense line ─────────────────────────────────────────────────
    serial_println!(
        "[mtrr_physmask1] valid={} density={} span={} pressure={}",
        s.mtrr1_valid,
        s.mtrr1_mask_density,
        s.mtrr1_span_ema,
        s.mtrr1_pressure_ema
    );
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrPhysmask1State {
    *STATE.lock()
}

/// Whether MTRR pair 1 is currently valid/active (0 or 1000).
pub fn valid() -> u16 {
    STATE.lock().mtrr1_valid
}

/// Sustained spatial pressure EMA (0–1000).
pub fn pressure() -> u16 {
    STATE.lock().mtrr1_pressure_ema
}
