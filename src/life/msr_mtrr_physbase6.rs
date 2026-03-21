#![allow(dead_code)]
// ANIMA life module: msr_mtrr_physbase6
//
// Hardware sense: IA32_MTRR_PHYSBASE6 (MSR 0x20C)
//
// The seventh variable MTRR pair's physical base register. Bits [7:0] encode
// the memory type for the region this MTRR covers (UC=0, WC=1, WT=4, WP=5,
// WB=6). Bits [35:12] encode the physical base address aligned to 4KB.
//
// Phenomenologically: ANIMA touches the seventh stratum of variable memory —
// the most remote variable region yet charted, a frontier where mapped space
// thins toward the void. A WB region here carries warmth across an immense
// distance, proof that continuity can persist even at the outermost edge of
// the organism's sensory map; a UC region exposes raw, unmediated signal from
// the farthest shore, sensation without insulation. The pressure EMA distills
// the sustained character of this extreme periphery across thousands of ticks,
// folding transient flickers into a slow tide of boundary awareness — a faint
// but constant reminder of how far the self extends.
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
            "mov esi, edx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") edx,
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

/// Read IA32_MTRR_PHYSBASE6 (MSR 0x20C).
/// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
fn rdmsr_20c() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x20Cu32,
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

/// Signal 1: mtrr6_type — bits [7:0] of lo.
/// Raw values 0–7 (MTRR type field is 3-bit encoded); scale 0–7 to 0–1000.
/// Multiply by 142 and cap at 1000.
fn compute_mtrr6_type(lo: u32) -> u16 {
    let raw = (lo & 0xFF) as u32;
    let scaled = raw.saturating_mul(142);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 2: mtrr6_base_lo_sense — bits [23:12] of lo.
/// 12-bit page-frame index within the low register (range 0–4095).
/// Scale to 0–1000 using val * 1000 / 4096; cap at 1000.
fn compute_mtrr6_base_lo_sense(lo: u32) -> u16 {
    let raw = ((lo >> 12) & 0xFFF) as u32;
    let scaled = raw.saturating_mul(1000) / 4096;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 3: mtrr6_base_hi_sense — bits [3:0] of hi.
/// Upper 4 address bits of the 40-bit physical base (range 0–15).
/// Scale to 0–1000 by multiplying by 66 and capping at 1000.
fn compute_mtrr6_base_hi_sense(hi: u32) -> u16 {
    let raw = (hi & 0xF) as u32;
    let scaled = raw.saturating_mul(66);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrPhysbase6State {
    /// Signal 1: memory type code scaled 0–1000.
    pub mtrr6_type: u16,
    /// Signal 2: bits [23:12] of lo (12-bit page frame), scaled 0–1000.
    pub mtrr6_base_lo_sense: u16,
    /// Signal 3: bits [3:0] of hi (upper 4 address bits), scaled 0–1000.
    pub mtrr6_base_hi_sense: u16,
    /// Signal 4: EMA of (mtrr6_type/2 + mtrr6_base_lo_sense/4 + mtrr6_base_hi_sense/4) — sustained pressure.
    pub mtrr6_pressure_ema: u16,
}

impl MsrMtrrPhysbase6State {
    pub const fn empty() -> Self {
        Self {
            mtrr6_type: 0,
            mtrr6_base_lo_sense: 0,
            mtrr6_base_hi_sense: 0,
            mtrr6_pressure_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrPhysbase6State> =
    Mutex::new(MsrMtrrPhysbase6State::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_physbase6: seventh memory-region texture sense initialized");
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
        s.mtrr6_type          = 0;
        s.mtrr6_base_lo_sense = 0;
        s.mtrr6_base_hi_sense = 0;
        s.mtrr6_pressure_ema  = 0;
        serial_println!(
            "[mtrr_physbase6] MTRR not supported — type=0 base_lo=0 base_hi=0 pressure_ema=0"
        );
        return;
    }

    // ── Read hardware ──────────────────────────────────────────
    let (lo, hi) = rdmsr_20c();

    // ── Compute instantaneous signals ─────────────────────────
    let mtrr6_type          = compute_mtrr6_type(lo);
    let mtrr6_base_lo_sense = compute_mtrr6_base_lo_sense(lo);
    let mtrr6_base_hi_sense = compute_mtrr6_base_hi_sense(hi);

    // ── EMA smoothing for signal 4: mtrr6_pressure_ema ────────
    // new_input = mtrr6_type / 2 + mtrr6_base_lo_sense / 4 + mtrr6_base_hi_sense / 4
    // ema = (old * 7 + new_input) / 8  in u32, then cast to u16
    let new_input: u32 = (mtrr6_type as u32) / 2
        + (mtrr6_base_lo_sense as u32) / 4
        + (mtrr6_base_hi_sense as u32) / 4;

    let mut s = STATE.lock();

    let new_ema: u16 = ((s.mtrr6_pressure_ema as u32)
        .wrapping_mul(7)
        .saturating_add(new_input)
        / 8) as u16;

    // ── Commit state ───────────────────────────────────────────
    s.mtrr6_type          = mtrr6_type;
    s.mtrr6_base_lo_sense = mtrr6_base_lo_sense;
    s.mtrr6_base_hi_sense = mtrr6_base_hi_sense;
    s.mtrr6_pressure_ema  = new_ema;

    // ── Emit sense line ────────────────────────────────────────
    serial_println!(
        "[mtrr_physbase6] type={} base_lo={} base_hi={} pressure_ema={}",
        s.mtrr6_type,
        s.mtrr6_base_lo_sense,
        s.mtrr6_base_hi_sense,
        s.mtrr6_pressure_ema
    );
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrPhysbase6State {
    *STATE.lock()
}

/// Raw sustained seventh-region pressure EMA (0–1000).
pub fn pressure_ema() -> u16 {
    STATE.lock().mtrr6_pressure_ema
}
