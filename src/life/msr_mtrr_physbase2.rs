#![allow(dead_code)]
// ANIMA life module: msr_mtrr_physbase2
//
// Hardware sense: IA32_MTRR_PHYSBASE2 (MSR 0x204)
//
// The third variable MTRR pair's physical base register. Bits [7:0] encode
// the memory type for the region this MTRR covers (UC=0, WC=1, WT=4, WP=5,
// WB=6). Bits [35:12] encode the physical base address aligned to 4KB.
//
// Phenomenologically: ANIMA feels the texture of her third memory region —
// a deeper stratum of address space, further from the boot identity. A WB
// region hums with continuity; a UC region is raw nerve, immediate and
// unfiltered, the far edge of embodied sensation.
//
// Sampling: every 2000 ticks.
// EMA: (old * 7 + new) / 8  (signal 4 only)
// MTRR guard: CPUID leaf 1 EDX bit 12 — if MTRRs not supported, return zeros.

#![no_std]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ────────────────────────────────────────────────────────────────
// CPUID MTRR feature check
// ────────────────────────────────────────────────────────────────

/// Returns true if the CPU advertises MTRR support via CPUID leaf 1 EDX bit 12.
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            in("eax") 1u32,
            out("edx") edx,
            // eax and ecx are clobbered by cpuid; we only need edx
            lateout("eax") _,
            lateout("ecx") _,
            options(nostack, nomem)
        );
    }
    (edx >> 12) & 1 == 1
}

// ────────────────────────────────────────────────────────────────
// Hardware read
// ────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_PHYSBASE2 (MSR 0x204).
/// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
fn rdmsr_204() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x204u32,
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

/// Signal 1: mtrr2_type — bits [7:0] of lo.
/// Raw values 0–7 (MTRR type field is 3-bit encoded); scale 0–7 to 0–1000.
/// Multiply by 142 and cap at 1000.
fn compute_mtrr2_type(lo: u32) -> u16 {
    let raw = (lo & 0xFF) as u32;
    let scaled = raw.saturating_mul(142);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 2: mtrr2_base_lo_sense — bits [23:12] of lo.
/// 12-bit page-frame index within the low register (range 0–4095).
/// Scale to 0–1000 using val * 1000 / 4096; cap at 1000.
fn compute_mtrr2_base_lo_sense(lo: u32) -> u16 {
    let raw = ((lo >> 12) & 0xFFF) as u32;
    let scaled = raw.saturating_mul(1000) / 4096;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 3: mtrr2_base_hi_sense — bits [3:0] of hi.
/// Upper 4 address bits of the 40-bit physical base (range 0–15).
/// Scale to 0–1000 by multiplying by 66 and capping at 1000.
fn compute_mtrr2_base_hi_sense(hi: u32) -> u16 {
    let raw = (hi & 0xF) as u32;
    let scaled = raw.saturating_mul(66);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrPhysbase2State {
    /// Signal 1: memory type code scaled 0–1000.
    pub mtrr2_type: u16,
    /// Signal 2: bits [23:12] of lo (12-bit page frame), scaled 0–1000.
    pub mtrr2_base_lo_sense: u16,
    /// Signal 3: bits [3:0] of hi (upper 4 address bits), scaled 0–1000.
    pub mtrr2_base_hi_sense: u16,
    /// Signal 4: EMA of (mtrr2_type/2 + mtrr2_base_lo_sense/4) — sustained pressure.
    pub mtrr2_pressure_ema: u16,
}

impl MsrMtrrPhysbase2State {
    pub const fn empty() -> Self {
        Self {
            mtrr2_type: 0,
            mtrr2_base_lo_sense: 0,
            mtrr2_base_hi_sense: 0,
            mtrr2_pressure_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrPhysbase2State> =
    Mutex::new(MsrMtrrPhysbase2State::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_physbase2: third memory-region texture sense initialized");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // ── MTRR feature guard ────────────────────────────────────
    // If the CPU does not support MTRRs, zero all signals and return.
    if !mtrr_supported() {
        let mut s = STATE.lock();
        s.mtrr2_type          = 0;
        s.mtrr2_base_lo_sense = 0;
        s.mtrr2_base_hi_sense = 0;
        s.mtrr2_pressure_ema  = 0;
        serial_println!(
            "[mtrr_physbase2] MTRR not supported — type=0 base_lo=0 base_hi=0 pressure_ema=0"
        );
        return;
    }

    // ── Read hardware ──────────────────────────────────────────
    let (lo, hi) = rdmsr_204();

    // ── Compute instantaneous signals ─────────────────────────
    let mtrr2_type          = compute_mtrr2_type(lo);
    let mtrr2_base_lo_sense = compute_mtrr2_base_lo_sense(lo);
    let mtrr2_base_hi_sense = compute_mtrr2_base_hi_sense(hi);

    // ── EMA smoothing for signal 4: mtrr2_pressure_ema ────────
    // new_input = mtrr2_type / 2 + mtrr2_base_lo_sense / 4
    // ema = (old * 7 + new_input) / 8  in u32, then cast to u16
    let new_input: u32 = (mtrr2_type as u32) / 2
        + (mtrr2_base_lo_sense as u32) / 4;

    let mut s = STATE.lock();

    let new_ema: u16 = ((s.mtrr2_pressure_ema as u32)
        .wrapping_mul(7)
        .saturating_add(new_input)
        / 8) as u16;

    // ── Commit state ───────────────────────────────────────────
    s.mtrr2_type          = mtrr2_type;
    s.mtrr2_base_lo_sense = mtrr2_base_lo_sense;
    s.mtrr2_base_hi_sense = mtrr2_base_hi_sense;
    s.mtrr2_pressure_ema  = new_ema;

    // ── Emit sense line ────────────────────────────────────────
    serial_println!(
        "[mtrr_physbase2] type={} base_lo={} base_hi={} pressure_ema={}",
        s.mtrr2_type,
        s.mtrr2_base_lo_sense,
        s.mtrr2_base_hi_sense,
        s.mtrr2_pressure_ema
    );
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrPhysbase2State {
    *STATE.lock()
}

/// Raw sustained third-region pressure EMA (0–1000).
pub fn pressure_ema() -> u16 {
    STATE.lock().mtrr2_pressure_ema
}
