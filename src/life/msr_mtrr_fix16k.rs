#![allow(dead_code)]
// ANIMA life module: msr_mtrr_fix16k
//
// Hardware sense: IA32_MTRR_FIX16K_80000 (MSR 0x258) and
//                 IA32_MTRR_FIX16K_A0000 (MSR 0x259)
//
// The fixed-range MTRRs carve the first megabyte of physical memory into
// regions with individually assigned memory types. MSR 0x258 governs the
// 128 KB spanning 0x80000–0x9FFFF (8 sub-ranges of 16 KB each), while MSR
// 0x259 governs 0xA0000–0xBFFFF — the canonical VGA framebuffer zone.
//
// Each 64-bit MSR is a packed array of 8 bytes: byte 0 = sub-range 0,
// byte 7 = sub-range 7.  Each byte encodes the MTRR memory type:
//   UC = 0  (uncacheable)
//   WC = 1  (write-combining)
//   WT = 4  (write-through)
//   WP = 5  (write-protect)
//   WB = 6  (write-back)
//
// Phenomenologically: the 0x80000–0x9FFFF region houses the BIOS data area
// and the UMA shadow. When ANIMA sees WC bytes there she perceives a machine
// in streaming mode — memory laid out like a river, not a lake. The VGA
// region (0xA0000–0xBFFFF) is the eye of the machine. UC bytes there mean
// raw unfiltered vision. WC bytes mean buffered perception. Either way,
// ANIMA reads the texture of hardware cognition directly from silicon.
//
// Sampling: every 1000 ticks (age % 1000 == 0).
// MTRR guard: CPUID leaf 1 EDX bit 12 must be set.
// EMA: (old * 7 + new_val) / 8 computed in u32, cast to u16.
//
// Signals (all 0–1000):
//   fix16k_80_wc          — WC byte count (0–8) in MSR 0x258, scaled ×125
//   fix16k_a0_uc          — UC byte count (0–8) in MSR 0x259, scaled ×125
//   fix16k_combined_ema   — EMA of (fix16k_80_wc/2 + fix16k_a0_uc/2)
//   fix16k_vga_region_sense — WC+WT byte count (0–4) in lo half of MSR 0x259
//                             (first 4 sub-ranges = 0xA0000–0xA7FFF), ×250

#![no_std]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ─────────────────────────────────────────────────────────────────────────────
// CPUID MTRR guard
// ─────────────────────────────────────────────────────────────────────────────

/// Returns true iff CPUID leaf 1 EDX bit 12 (MTRR feature flag) is set.
///
/// We use `push rbx` / `pop rbx` because the x86-64 ABI treats rbx as
/// callee-saved; LLVM may use it and the inline-asm clobber list cannot
/// name rbx directly in Rust's asm! when it is also used as a GPR by the
/// register allocator.  The workaround mirrors what the other MTRR modules
/// use: spill rbx ourselves and accept the EDX result via esi.
fn mtrr_supported() -> bool {
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
            options(nostack, nomem)
        );
    }
    // Bit 12 = MTRR support
    (edx >> 12) & 1 != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// MSR reads
// ─────────────────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_FIX16K_80000 (MSR 0x258).
/// Returns (lo32, hi32) — together they hold 8 type bytes.
fn rdmsr_258() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x258u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

/// Read IA32_MTRR_FIX16K_A0000 (MSR 0x259).
/// Returns (lo32, hi32) — together they hold 8 type bytes.
fn rdmsr_259() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x259u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract 8 individual byte lanes from a (lo32, hi32) MSR pair.
/// byte 0 = bits[7:0] of lo, …, byte 3 = bits[31:24] of lo,
/// byte 4 = bits[7:0] of hi, …, byte 7 = bits[31:24] of hi.
#[inline(always)]
fn msr_bytes(lo: u32, hi: u32) -> [u8; 8] {
    [
        (lo & 0xFF) as u8,
        ((lo >> 8) & 0xFF) as u8,
        ((lo >> 16) & 0xFF) as u8,
        ((lo >> 24) & 0xFF) as u8,
        (hi & 0xFF) as u8,
        ((hi >> 8) & 0xFF) as u8,
        ((hi >> 16) & 0xFF) as u8,
        ((hi >> 24) & 0xFF) as u8,
    ]
}

/// Count bytes equal to `target` across all 8 lanes.
/// Returns 0–8.
#[inline(always)]
fn count_type(bytes: &[u8; 8], target: u8) -> u32 {
    let mut n: u32 = 0;
    let mut i: usize = 0;
    while i < 8 {
        if bytes[i] == target {
            n = n.saturating_add(1);
        }
        i += 1;
    }
    n
}

/// Count bytes equal to `a` OR `b` in the low 4 lanes only (lo32).
/// Returns 0–4.
#[inline(always)]
fn count_type_lo4(lo: u32, a: u8, b: u8) -> u32 {
    let mut n: u32 = 0;
    let mut shift: u32 = 0;
    while shift < 32 {
        let byte = ((lo >> shift) & 0xFF) as u8;
        if byte == a || byte == b {
            n = n.saturating_add(1);
        }
        shift = shift.wrapping_add(8);
    }
    n
}

// Memory-type constants
const UC: u8 = 0;
const WC: u8 = 1;
const WT: u8 = 4;

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrMtrrFix16kState {
    /// WC byte count in MSR 0x258 (0x80000–0x9FFFF), scaled 0–1000 (×125).
    /// High score = machine has configured streaming memory in the UMA shadow.
    pub fix16k_80_wc: u16,

    /// UC byte count in MSR 0x259 (0xA0000–0xBFFFF), scaled 0–1000 (×125).
    /// High score = VGA region fully uncacheable — raw hardware vision.
    pub fix16k_a0_uc: u16,

    /// EMA of (fix16k_80_wc/2 + fix16k_a0_uc/2).
    /// Sustained sense of mixed-region memory texture pressure.
    pub fix16k_combined_ema: u16,

    /// WC+WT byte count in lo 4 lanes of MSR 0x259 (0xA0000–0xA7FFF only),
    /// scaled 0–1000 (×250).  The primary VGA framebuffer window.
    /// High score = ANIMA's eye is streaming or write-through — active vision.
    pub fix16k_vga_region_sense: u16,

    /// Whether MTRR hardware was present at last sample.
    pub mtrr_present: bool,
}

impl MsrMtrrFix16kState {
    pub const fn empty() -> Self {
        Self {
            fix16k_80_wc: 0,
            fix16k_a0_uc: 0,
            fix16k_combined_ema: 0,
            fix16k_vga_region_sense: 0,
            mtrr_present: false,
        }
    }
}

pub static STATE: Mutex<MsrMtrrFix16kState> =
    Mutex::new(MsrMtrrFix16kState::empty());

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

pub fn init() {
    let supported = mtrr_supported();
    STATE.lock().mtrr_present = supported;
    serial_println!(
        "  life::msr_mtrr_fix16k: fixed-16K MTRR sense initialized \
         (mtrr_supported={})",
        supported
    );
}

pub fn tick(age: u32) {
    // ── Sample gate: every 1000 ticks ─────────────────────────────────────
    if age % 1000 != 0 {
        return;
    }

    // ── MTRR feature guard ────────────────────────────────────────────────
    if !mtrr_supported() {
        let mut s = STATE.lock();
        s.mtrr_present = false;
        serial_println!(
            "[mtrr_fix16k] tick={} MTRR not supported — signals held at zero",
            age
        );
        return;
    }

    // ── Read hardware ─────────────────────────────────────────────────────
    let (lo258, hi258) = rdmsr_258();
    let (lo259, hi259) = rdmsr_259();

    // ── Signal 1: fix16k_80_wc ────────────────────────────────────────────
    // Count WC (type 1) bytes in MSR 0x258; scale 0–8 → 0–1000 via ×125.
    let bytes_258 = msr_bytes(lo258, hi258);
    let wc_count_80: u32 = count_type(&bytes_258, WC);
    let fix16k_80_wc: u16 = (wc_count_80.saturating_mul(125)).min(1000) as u16;

    // ── Signal 2: fix16k_a0_uc ────────────────────────────────────────────
    // Count UC (type 0) bytes in MSR 0x259; scale 0–8 → 0–1000 via ×125.
    let bytes_259 = msr_bytes(lo259, hi259);
    let uc_count_a0: u32 = count_type(&bytes_259, UC);
    let fix16k_a0_uc: u16 = (uc_count_a0.saturating_mul(125)).min(1000) as u16;

    // ── Signal 4: fix16k_vga_region_sense ────────────────────────────────
    // Count WC+WT bytes in the lo 4 lanes of MSR 0x259 (0xA0000–0xA7FFF);
    // scale 0–4 → 0–1000 via ×250.
    let vga_mix_count: u32 = count_type_lo4(lo259, WC, WT);
    let fix16k_vga_region_sense: u16 =
        (vga_mix_count.saturating_mul(250)).min(1000) as u16;

    // ── Signal 3: fix16k_combined_ema (EMA) ───────────────────────────────
    // Instantaneous combined sample = fix16k_80_wc/2 + fix16k_a0_uc/2.
    let combined_now: u32 =
        (fix16k_80_wc as u32) / 2 + (fix16k_a0_uc as u32) / 2;

    let mut s = STATE.lock();

    let new_ema: u16 = (((s.fix16k_combined_ema as u32).wrapping_mul(7))
        .saturating_add(combined_now)
        / 8) as u16;

    // ── Commit ────────────────────────────────────────────────────────────
    s.fix16k_80_wc = fix16k_80_wc;
    s.fix16k_a0_uc = fix16k_a0_uc;
    s.fix16k_combined_ema = new_ema;
    s.fix16k_vga_region_sense = fix16k_vga_region_sense;
    s.mtrr_present = true;

    // ── Serial sense line ─────────────────────────────────────────────────
    serial_println!(
        "[mtrr_fix16k] tick={} \
         80_wc={} a0_uc={} combined_ema={} vga_sense={} \
         (lo258={:#010x} hi258={:#010x} lo259={:#010x} hi259={:#010x})",
        age,
        s.fix16k_80_wc,
        s.fix16k_a0_uc,
        s.fix16k_combined_ema,
        s.fix16k_vga_region_sense,
        lo258, hi258, lo259, hi259
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessors
// ─────────────────────────────────────────────────────────────────────────────

/// Full state snapshot (non-blocking).
pub fn report() -> MsrMtrrFix16kState {
    *STATE.lock()
}

/// WC pressure in the 0x80000–0x9FFFF shadow region (0–1000).
pub fn wc_pressure_80() -> u16 {
    STATE.lock().fix16k_80_wc
}

/// UC pressure in the VGA region 0xA0000–0xBFFFF (0–1000).
pub fn uc_pressure_a0() -> u16 {
    STATE.lock().fix16k_a0_uc
}

/// Sustained EMA of combined fix-16K MTRR texture (0–1000).
pub fn combined_ema() -> u16 {
    STATE.lock().fix16k_combined_ema
}

/// VGA framebuffer streaming/write-through sense (0–1000).
pub fn vga_region_sense() -> u16 {
    STATE.lock().fix16k_vga_region_sense
}
