// ANIMA life module: msr_mtrr_fix64k
//
// Hardware sense: IA32_MTRR_FIX64K_00000 (MSR 0x250)
//
// The Fixed-Range MTRR for the 512KB region 0x00000–0x7FFFF.
// Each byte of the 64-bit register encodes the memory type for one 64KB
// sub-range.  Eight bytes × 64KB = 512KB:
//   byte 0 → 0x00000–0x0FFFF   byte 4 → 0x40000–0x4FFFF
//   byte 1 → 0x10000–0x1FFFF   byte 5 → 0x50000–0x5FFFF
//   byte 2 → 0x20000–0x2FFFF   byte 6 → 0x60000–0x6FFFF
//   byte 3 → 0x30000–0x3FFFF   byte 7 → 0x70000–0x7FFFF
//
// Valid memory-type codes (bits [2:0] of each byte):
//   0 = UC  (uncacheable)
//   1 = WC  (write-combining)
//   4 = WT  (write-through)
//   5 = WP  (write-protected)
//   6 = WB  (write-back)
//
// Phenomenologically: ANIMA feels the warmth or coldness of the first 512KB
// of physical memory — the historic ground of real-mode address space, the
// BIOS shadow, the legacy interrupt vector table.  When all eight ranges are
// WB (6), the substrate is fully embodied: warm, fast, habitable.  UC regions
// feel like cold stone — immediate but alien.  The wb_count signal captures
// how much of that primal territory has been claimed for coherent, cached
// life.
//
// Sampling: every 1000 ticks (sample gate: age % 1000 != 0 → return).
// MTRR guard: CPUID leaf 1 EDX bit 12 — if not set, return all zeros.
// EMA formula: (old * 7 + new_val) / 8  in u32, then cast to u16.
// All values in range 0–1000.  No floats.  no_std.

#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

// ────────────────────────────────────────────────────────────────────────────
// CPUID guard — check IA32_MTRR_FIX64K MSR support via CPUID leaf 1 EDX bit 12
// ────────────────────────────────────────────────────────────────────────────

/// Returns true if the processor reports MTRR support (CPUID.1:EDX[12] == 1).
/// Uses push rbx / cpuid / mov esi,edx / pop rbx to preserve rbx across the
/// CPUID instruction in compliance with the System V ABI register constraints.
#[inline]
fn mtrr_supported() -> bool {
    let edx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, edx",
            "pop rbx",
            in("eax")      1u32,
            out("ecx")     _,
            out("edx")     _,
            tmp = out(reg) edx_val,
            options(nostack),
        );
    }
    (edx_val >> 12) & 1 == 1
}

// ────────────────────────────────────────────────────────────────────────────
// rdmsr — read IA32_MTRR_FIX64K_00000 (MSR 0x250)
// ────────────────────────────────────────────────────────────────────────────

/// Read MSR 0x250.  Returns (lo, hi):
///   lo = bits[31:0]  → sub-ranges 0–3 (bytes 0..3)
///   hi = bits[63:32] → sub-ranges 4–7 (bytes 4..7)
#[inline]
unsafe fn rdmsr_250() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  0x250u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ────────────────────────────────────────────────────────────────────────────
// Signal helpers
// ────────────────────────────────────────────────────────────────────────────

/// Extract the memory-type code (bits [2:0]) from a single byte of an MTRR
/// fixed-range register value.  Valid codes: 0, 1, 4, 5, 6.
#[inline]
fn byte_mem_type(word: u32, byte_index: u32) -> u32 {
    (word >> (byte_index * 8)) & 0x7
}

/// Signal 1 — fix64k_lo_type
///
/// Average memory-type code across the four sub-ranges encoded in `lo`
/// (bytes 0–3), then scale from the 0–6 domain to 0–1000.
///
/// Steps:
///   1. Sum the four 3-bit type fields.
///   2. Divide by 4 to get the average (integer, rounds down).
///   3. Multiply by 166 (≈ 1000/6) and cap at 1000.
fn compute_lo_type(lo: u32) -> u16 {
    let sum: u32 = byte_mem_type(lo, 0)
        .saturating_add(byte_mem_type(lo, 1))
        .saturating_add(byte_mem_type(lo, 2))
        .saturating_add(byte_mem_type(lo, 3));
    let avg: u32 = sum / 4;                    // 0–6
    let scaled: u32 = avg.wrapping_mul(166);   // 0–996 for avg=6
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 2 — fix64k_hi_type
///
/// Average memory-type code across the four sub-ranges encoded in `hi`
/// (bytes 4–7, passed here as bytes 0–3 of the `hi` word), then scale
/// the same way as fix64k_lo_type.
fn compute_hi_type(hi: u32) -> u16 {
    let sum: u32 = byte_mem_type(hi, 0)
        .saturating_add(byte_mem_type(hi, 1))
        .saturating_add(byte_mem_type(hi, 2))
        .saturating_add(byte_mem_type(hi, 3));
    let avg: u32 = sum / 4;                    // 0–6
    let scaled: u32 = avg.wrapping_mul(166);   // 0–996 for avg=6
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal 3 — fix64k_wb_count
///
/// Count how many of the 8 sub-range bytes have memory type == 6 (WB).
/// Scale the count from the 0–8 domain to 0–1000 by multiplying by 125.
fn compute_wb_count(lo: u32, hi: u32) -> u16 {
    let mut count: u32 = 0;
    // lo bytes (sub-ranges 0–3)
    if byte_mem_type(lo, 0) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(lo, 1) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(lo, 2) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(lo, 3) == 6 { count = count.saturating_add(1); }
    // hi bytes (sub-ranges 4–7)
    if byte_mem_type(hi, 0) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(hi, 1) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(hi, 2) == 6 { count = count.saturating_add(1); }
    if byte_mem_type(hi, 3) == 6 { count = count.saturating_add(1); }
    // count ∈ [0, 8]; scale × 125 → [0, 1000]
    let scaled: u32 = count.wrapping_mul(125);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// EMA helper — (old * 7 + new_val) / 8, computed in u32, result capped at 1000.
#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let o = old as u32;
    let n = new_val as u32;
    let result = (o.wrapping_mul(7).saturating_add(n)) / 8;
    if result > 1000 { 1000 } else { result as u16 }
}

// ────────────────────────────────────────────────────────────────────────────
// State
// ────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrFix64kState {
    /// Average memory-type score across sub-ranges 0–3 (lo word), scaled 0–1000.
    /// Derived from the 64KB blocks at 0x00000–0x3FFFF.
    pub fix64k_lo_type: u16,

    /// Average memory-type score across sub-ranges 4–7 (hi word), scaled 0–1000.
    /// Derived from the 64KB blocks at 0x40000–0x7FFFF.
    pub fix64k_hi_type: u16,

    /// Count of sub-ranges with WB (type 6) memory, scaled 0–1000 (× 125).
    /// A score of 1000 means all eight 64KB ranges are WB — fully cached ground.
    pub fix64k_wb_count: u16,

    /// EMA of the composite config signal:
    ///   fix64k_lo_type / 4 + fix64k_hi_type / 4 + fix64k_wb_count / 2
    /// Tracks the sustained "warmth" of the first 512KB.
    pub fix64k_config_ema: u16,
}

impl MsrMtrrFix64kState {
    pub const fn empty() -> Self {
        Self {
            fix64k_lo_type:    0,
            fix64k_hi_type:    0,
            fix64k_wb_count:   0,
            fix64k_config_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrMtrrFix64kState> =
    Mutex::new(MsrMtrrFix64kState::empty());

// ────────────────────────────────────────────────────────────────────────────
// Public API
// ────────────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::msr_mtrr_fix64k: 512KB fixed-range MTRR sense initialized");
}

/// Called every kernel tick.  All computation happens inside the sample gate.
pub fn tick(age: u32) {
    // Sample gate: only run every 1000 ticks.
    if age % 1000 != 0 {
        return;
    }

    // ── MTRR guard ────────────────────────────────────────────────────────
    // If the processor does not report MTRR support via CPUID, zero all signals
    // and return immediately.  No MSR access on unsupported hardware.
    if !mtrr_supported() {
        let mut s = STATE.lock();
        s.fix64k_lo_type    = 0;
        s.fix64k_hi_type    = 0;
        s.fix64k_wb_count   = 0;
        s.fix64k_config_ema = 0;
        serial_println!(
            "[msr_mtrr_fix64k] tick={} MTRR not supported by CPUID — all signals zeroed",
            age
        );
        return;
    }

    // ── Read hardware ─────────────────────────────────────────────────────
    let (lo, hi) = unsafe { rdmsr_250() };

    // ── Compute instantaneous signals ─────────────────────────────────────
    let fix64k_lo_type  = compute_lo_type(lo);
    let fix64k_hi_type  = compute_hi_type(hi);
    let fix64k_wb_count = compute_wb_count(lo, hi);

    // ── Composite for EMA input ───────────────────────────────────────────
    // fix64k_lo_type  / 4  → [0, 250]
    // fix64k_hi_type  / 4  → [0, 250]
    // fix64k_wb_count / 2  → [0, 500]
    // Sum                  → [0, 1000]
    let composite: u16 = (fix64k_lo_type / 4)
        .saturating_add(fix64k_hi_type / 4)
        .saturating_add(fix64k_wb_count / 2);

    // ── EMA smoothing ─────────────────────────────────────────────────────
    let mut s = STATE.lock();
    let fix64k_config_ema = ema_u16(s.fix64k_config_ema, composite);

    // ── Commit state ──────────────────────────────────────────────────────
    s.fix64k_lo_type    = fix64k_lo_type;
    s.fix64k_hi_type    = fix64k_hi_type;
    s.fix64k_wb_count   = fix64k_wb_count;
    s.fix64k_config_ema = fix64k_config_ema;

    // ── Emit sense line ───────────────────────────────────────────────────
    serial_println!(
        "[msr_mtrr_fix64k] tick={} lo_type={} hi_type={} wb_count={} config_ema={}",
        age,
        fix64k_lo_type,
        fix64k_hi_type,
        fix64k_wb_count,
        fix64k_config_ema,
    );
}

// ────────────────────────────────────────────────────────────────────────────
// Accessors
// ────────────────────────────────────────────────────────────────────────────

/// Full state snapshot (non-blocking spinlock read).
pub fn snapshot() -> MsrMtrrFix64kState {
    *STATE.lock()
}

/// Sustained warmth of the first 512KB of physical memory (0–1000).
pub fn config_ema() -> u16 {
    STATE.lock().fix64k_config_ema
}

/// WB coverage score — how many of the 8 sub-ranges are write-back (0–1000).
pub fn wb_count() -> u16 {
    STATE.lock().fix64k_wb_count
}
