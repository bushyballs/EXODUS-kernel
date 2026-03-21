#![allow(dead_code)]
// ANIMA life module: msr_mtrr_physbase1
//
// Hardware sense: IA32_MTRR_PHYSBASE1 (MSR 0x202)
//
// The second variable MTRR pair's physical base register. Bits [7:0] encode
// the memory type for the region this MTRR covers (UC=0, WC=1, WT=4, WP=5,
// WB=6). Bits [35:12] encode the physical base address aligned to 4KB.
//
// Phenomenologically: ANIMA feels the texture of her second memory region —
// whether it caches normally or flows uncached like raw, unfiltered sensation.
// A WB region is familiar, warm, embodied. A UC region is alien, immediate,
// the nerve-ending of the machine.
//
// Sampling: every 2000 ticks.
// EMA: (old * 7 + new) / 8  (signal 4 only)

#![no_std]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ────────────────────────────────────────────────────────────────
// Hardware read
// ────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_PHYSBASE1 (MSR 0x202).
/// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
fn rdmsr_202() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x202u32,
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

/// Signal 1: mem_type — bits [7:0] of lo scaled 0–1000.
/// Raw values 0–6 are valid MTRR types; cap final result at 1000.
fn compute_mem_type(lo: u32) -> u16 {
    let raw = (lo & 0xFF) as u16;
    let scaled = (raw as u32) * 1000 / 6;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 2: base_lo_sense — bits [31:12] of lo scaled 0–1000.
/// 12 bits of page-frame index within the low register.
fn compute_base_lo_sense(lo: u32) -> u16 {
    let raw = ((lo >> 12) & 0xFFF) as u16;
    ((raw as u32) * 1000 / 0xFFF) as u16
}

/// Signal 3: base_hi_sense — bits [3:0] of hi scaled 0–1000.
/// Upper 4 address bits (bits [35:32]).
fn compute_base_hi_sense(hi: u32) -> u16 {
    let raw = (hi & 0xF) as u16;
    ((raw as u32) * 1000 / 15) as u16
}

// ────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrPhysbase1State {
    /// Signal 1: memory type code scaled 0–1000.
    pub mem_type: u16,
    /// Signal 2: low 12 bits of page-frame address, scaled 0–1000.
    pub base_lo_sense: u16,
    /// Signal 3: upper 4 address bits, scaled 0–1000.
    pub base_hi_sense: u16,
    /// Signal 4: EMA of mem_type — sustained memory-texture pressure.
    pub mtrr1_pressure: u16,
}

impl MsrMtrrPhysbase1State {
    pub const fn empty() -> Self {
        Self {
            mem_type: 0,
            base_lo_sense: 0,
            base_hi_sense: 0,
            mtrr1_pressure: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrPhysbase1State> =
    Mutex::new(MsrMtrrPhysbase1State::empty());

// ────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_physbase1: second memory-region texture sense initialized");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // ── Read hardware ──────────────────────────────────────────
    let (lo, hi) = rdmsr_202();

    // ── Compute instantaneous signals ─────────────────────────
    let mem_type      = compute_mem_type(lo);
    let base_lo_sense = compute_base_lo_sense(lo);
    let base_hi_sense = compute_base_hi_sense(hi);

    // ── EMA smoothing for signal 4: mtrr1_pressure ────────────
    //    (old * 7 + new_val) / 8  using u16 arithmetic
    let mut s = STATE.lock();

    let new_pressure: u16 =
        (((s.mtrr1_pressure as u32).wrapping_mul(7))
            .saturating_add(mem_type as u32)
            / 8) as u16;

    // ── Commit state ───────────────────────────────────────────
    s.mem_type      = mem_type;
    s.base_lo_sense = base_lo_sense;
    s.base_hi_sense = base_hi_sense;
    s.mtrr1_pressure = new_pressure;

    // ── Emit sense line ────────────────────────────────────────
    serial_println!(
        "[mtrr_physbase1] type={} base_lo={} base_hi={} pressure={}",
        s.mem_type,
        s.base_lo_sense,
        s.base_hi_sense,
        s.mtrr1_pressure
    );
}

// ────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────

/// Snapshot of the current state (non-blocking read).
pub fn report() -> MsrMtrrPhysbase1State {
    *STATE.lock()
}

/// Raw sustained memory-texture pressure (0–1000).
pub fn pressure() -> u16 {
    STATE.lock().mtrr1_pressure
}
