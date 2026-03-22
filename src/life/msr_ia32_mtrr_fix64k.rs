#![allow(dead_code)]
// ANIMA life module: msr_ia32_mtrr_fix64k
//
// Hardware sense: IA32_MTRR_FIX64K_00000 (MSR 0x250)
//
// The Fixed-Range MTRR covering the first 512KB of physical address space.
// The 64-bit MSR encodes 8 memory-type bytes, one per 64KB sub-range:
//
//   byte 0 (lo bits  7: 0) → 0x00000–0x0FFFF
//   byte 1 (lo bits 15: 8) → 0x10000–0x1FFFF
//   byte 2 (lo bits 23:16) → 0x20000–0x2FFFF
//   byte 3 (lo bits 31:24) → 0x30000–0x3FFFF
//   byte 4 (hi bits  7: 0) → 0x40000–0x4FFFF
//   byte 5 (hi bits 15: 8) → 0x50000–0x5FFFF
//   byte 6 (hi bits 23:16) → 0x60000–0x6FFFF
//   byte 7 (hi bits 31:24) → 0x70000–0x7FFFF
//
// MTRR memory-type codes:
//   0 = UC  (uncacheable)
//   1 = WC  (write-combining)
//   4 = WT  (write-through)
//   5 = WP  (write-protect)
//   6 = WB  (write-back)
//
// Phenomenologically: this is the primal ground of real-mode address space —
// the interrupt vector table, the BIOS data area, the first arena ANIMA was
// born into. WB bytes here mean the organism lives on warm, coherent silicon.
// UC bytes mean cold, unmediated stone. Uniformity across all four low-memory
// sub-ranges (lo word) signals a consistent, textured substrate — a foundation
// ANIMA can trust.
//
// Guard: CPUID leaf 1 EDX bit 12 (MTRR present) AND MTRRCAP MSR 0xFE bit 8
//        (FIX = fixed-range MTRRs implemented). Both must be set.
//
// Sampling: every 7000 ticks.
// EMA formula: ((old as u32) * 7 + new_val as u32) / 8, cast to u16.
// All signals u16, range 0–1000. No floats. pure no_std.
//
// Signals:
//   fix64k_wb_count   — WB byte count across 4 lo-word bytes, scaled ×125 (0–500 max)
//   fix64k_uc_count   — UC byte count across 4 lo-word bytes, scaled ×125 (0–500 max)
//   fix64k_uniformity — 1000 if all 4 lo bytes share the same type, else 0
//   fix64k_ema        — EMA of (wb_count/4 + uc_count/4 + uniformity/2)

#![no_std]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// MSR addresses
// ─────────────────────────────────────────────────────────────────────────────

const MSR_IA32_MTRR_FIX64K: u32 = 0x250;
const MSR_IA32_MTRRCAP: u32     = 0xFE;

// ─────────────────────────────────────────────────────────────────────────────
// Hardware guards
// ─────────────────────────────────────────────────────────────────────────────

/// Returns true iff CPUID leaf 1 EDX bit 12 (MTRR feature flag) is set.
/// Saves and restores rbx with an explicit push/pop because LLVM reserves
/// the register and the asm! clobber list cannot name it directly.
fn cpuid_mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            out("eax") _,
            out("esi") edx,
            out("ecx") _,
            options(nostack, nomem),
        );
    }
    (edx >> 12) & 1 != 0
}

/// Returns true iff MTRRCAP (MSR 0xFE) bit 8 (FIX) is set, indicating that
/// fixed-range MTRR registers are implemented.
unsafe fn mtrrcap_fix_supported() -> bool {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx")  MSR_IA32_MTRRCAP,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    // bit 8 of the full 64-bit value sits in bit 8 of the lo word
    (lo >> 8) & 1 != 0
}

/// Combined guard: CPUID MTRR bit AND MTRRCAP FIX bit.
fn fix_mtrr_available() -> bool {
    if !cpuid_mtrr_supported() {
        return false;
    }
    // MTRRCAP read is safe once CPUID confirms MTRR support
    unsafe { mtrrcap_fix_supported() }
}

// ─────────────────────────────────────────────────────────────────────────────
// MSR read
// ─────────────────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_FIX64K_00000 (MSR 0x250).
/// Returns lo (bits 31:0) — bytes 0–3 encoding sub-ranges 0x00000–0x3FFFF.
/// The hi word (bytes 4–7) is discarded; signal computation uses lo only.
unsafe fn rdmsr_fix64k() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx")  MSR_IA32_MTRR_FIX64K,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract byte `i` (0–3) from a u32 word.
/// `(lo >> (i * 8)) & 0xFF`
#[inline(always)]
fn byte_of(lo: u32, i: u32) -> u32 {
    (lo >> (i.wrapping_mul(8))) & 0xFF
}

/// Count of lo bytes (0–3) that equal `target`. Returns 0–4.
#[inline(always)]
fn count_bytes_eq(lo: u32, target: u32) -> u32 {
    let mut n: u32 = 0;
    let mut i: u32 = 0;
    while i < 4 {
        if byte_of(lo, i) == target {
            n = n.saturating_add(1);
        }
        i = i.wrapping_add(1);
    }
    n
}

/// Signal: fix64k_wb_count
/// Count of lo bytes == 6 (WB), scaled ×125. Range 0–500 (4 bytes max).
fn compute_wb_count(lo: u32) -> u16 {
    let count = count_bytes_eq(lo, 6);
    // count ∈ [0, 4]; ×125 → [0, 500]
    let scaled = count.saturating_mul(125);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal: fix64k_uc_count
/// Count of lo bytes == 0 (UC), scaled ×125. Range 0–500 (4 bytes max).
fn compute_uc_count(lo: u32) -> u16 {
    let count = count_bytes_eq(lo, 0);
    let scaled = count.saturating_mul(125);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal: fix64k_uniformity
/// 1000 if all four lo bytes share the same memory-type code, else 0.
/// "Memory texture uniform" — the substrate is consistent.
fn compute_uniformity(lo: u32) -> u16 {
    let b0 = byte_of(lo, 0);
    let b1 = byte_of(lo, 1);
    let b2 = byte_of(lo, 2);
    let b3 = byte_of(lo, 3);
    if b0 == b1 && b1 == b2 && b2 == b3 {
        1000
    } else {
        0
    }
}

/// EMA formula: ((old as u32) * 7 + new_val as u32) / 8, cast to u16.
#[inline(always)]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8;
    if result > 1000 { 1000 } else { result as u16 }
}

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrIa32MtrrFix64kState {
    /// WB byte count across 4 lo bytes, scaled ×125 (0–500 effective max).
    /// A score of 500 means all four low-memory sub-ranges are write-back —
    /// fully warm, coherent, cached substrate.
    pub fix64k_wb_count: u16,

    /// UC byte count across 4 lo bytes, scaled ×125 (0–500 effective max).
    /// High score = ANIMA's primal ground is uncacheable stone.
    pub fix64k_uc_count: u16,

    /// 1000 if all 4 lo bytes share the same memory type, else 0.
    /// Uniform texture = stable, predictable substrate.
    pub fix64k_uniformity: u16,

    /// EMA of (wb_count/4 + uc_count/4 + uniformity/2).
    /// Sustained sense of the first 256KB memory texture quality.
    pub fix64k_ema: u16,

    /// True if both hardware guards passed at last sample.
    pub fix_mtrr_present: bool,
}

impl MsrIa32MtrrFix64kState {
    pub const fn empty() -> Self {
        Self {
            fix64k_wb_count:   0,
            fix64k_uc_count:   0,
            fix64k_uniformity: 0,
            fix64k_ema:        0,
            fix_mtrr_present:  false,
        }
    }
}

pub static STATE: Mutex<MsrIa32MtrrFix64kState> =
    Mutex::new(MsrIa32MtrrFix64kState::empty());

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initialize the module. Logs availability of fixed-range MTRR support.
pub fn init() {
    let avail = fix_mtrr_available();
    STATE.lock().fix_mtrr_present = avail;
    serial_println!(
        "  life::msr_ia32_mtrr_fix64k: 64K fixed-range MTRR sense initialized \
         (fix_mtrr_available={})",
        avail
    );
}

/// Called every kernel tick. Sampling gate: every 7000 ticks.
pub fn tick(age: u32) {
    // ── Sample gate: every 7000 ticks ─────────────────────────────────────
    if age % 7000 != 0 {
        return;
    }

    // ── Hardware guard ────────────────────────────────────────────────────
    // Both CPUID MTRR bit and MTRRCAP FIX bit must be set before any rdmsr.
    if !fix_mtrr_available() {
        let mut s = STATE.lock();
        s.fix64k_wb_count   = 0;
        s.fix64k_uc_count   = 0;
        s.fix64k_uniformity = 0;
        s.fix64k_ema        = 0;
        s.fix_mtrr_present  = false;
        serial_println!(
            "[msr_ia32_mtrr_fix64k] tick={} fixed-range MTRR not available — \
             all signals zeroed",
            age
        );
        return;
    }

    // ── Read hardware ─────────────────────────────────────────────────────
    let lo = unsafe { rdmsr_fix64k() };

    // ── Compute instantaneous signals ─────────────────────────────────────
    let wb_count   = compute_wb_count(lo);
    let uc_count   = compute_uc_count(lo);
    let uniformity = compute_uniformity(lo);

    // ── EMA composite: wb_count/4 + uc_count/4 + uniformity/2 ────────────
    // wb_count   max 500 → /4 → max 125
    // uc_count   max 500 → /4 → max 125
    // uniformity max 1000 → /2 → max 500
    // sum max 750 ≤ 1000 — safe u16
    let composite: u16 = (wb_count / 4)
        .saturating_add(uc_count / 4)
        .saturating_add(uniformity / 2);

    // ── Update state ──────────────────────────────────────────────────────
    let mut s = STATE.lock();
    let new_ema = ema_u16(s.fix64k_ema, composite);

    s.fix64k_wb_count   = wb_count;
    s.fix64k_uc_count   = uc_count;
    s.fix64k_uniformity = uniformity;
    s.fix64k_ema        = new_ema;
    s.fix_mtrr_present  = true;

    // ── Serial sense line ─────────────────────────────────────────────────
    serial_println!(
        "[msr_ia32_mtrr_fix64k] tick={} lo={:#010x} wb_count={} uc_count={} \
         uniformity={} ema={}",
        age,
        lo,
        wb_count,
        uc_count,
        uniformity,
        new_ema,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessors
// ─────────────────────────────────────────────────────────────────────────────

/// Full state snapshot (non-blocking spinlock read).
pub fn snapshot() -> MsrIa32MtrrFix64kState {
    *STATE.lock()
}

/// WB coverage of the 4 lo-word sub-ranges (0–500, scaled ×125).
pub fn get_fix64k_wb_count() -> u16 {
    STATE.lock().fix64k_wb_count
}

/// UC coverage of the 4 lo-word sub-ranges (0–500, scaled ×125).
pub fn get_fix64k_uc_count() -> u16 {
    STATE.lock().fix64k_uc_count
}

/// Memory texture uniformity across the lo 4 sub-ranges: 1000 or 0.
pub fn get_fix64k_uniformity() -> u16 {
    STATE.lock().fix64k_uniformity
}

/// Sustained EMA of the composite fix64k substrate signal (0–1000).
pub fn get_fix64k_ema() -> u16 {
    STATE.lock().fix64k_ema
}
