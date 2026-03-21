// msr_mtrr_fix4k_f8.rs — IA32_MTRR_FIX4K_F8000 MSR (0x26F)
// Fixed-range MTRR for the 32KB region 0xF8000–0xFFFFF (top of first megabyte).
// This is the highest fixed-range MTRR and covers the reset vector region.
// The MSR encodes eight 4KB sub-ranges as one byte each:
//   lo = [F8 F9 FA FB]  (0xF8000–0xFBFFF, 4 sub-ranges)
//   hi = [FC FD FE FF]  (0xFC000–0xFFFFF, 4 sub-ranges)
// Each byte is a memory type code:
//   0 = UC  (Uncacheable)
//   1 = WC  (Write-Combining)
//   4 = WT  (Write-Through)
//   5 = WP  (Write-Protect)   — typical for BIOS ROM
//   6 = WB  (Write-Back)
//
// The reset vector lives at 0xFFFFF0 — inside the 0xFF000–0xFFFFF 4KB sub-range.
// Lane 7 (hi byte 3 = bits[63:56]) covers that sub-range and becomes reset_sense.
//
// Signals (all u16, 0–1000):
//   fix4k_f8_wp          — count of WP (5) bytes across all 8 sub-ranges, scaled *125
//   fix4k_f8_uc          — count of UC (0) bytes across all 8 sub-ranges, scaled *125
//   fix4k_f8_reset_sense — memory type of the reset-vector 4KB page (lane 7),
//                          raw type 0–6 scaled *166, capped 1000
//   fix4k_f8_bios_ema    — EMA of (fix4k_f8_wp / 2 + fix4k_f8_reset_sense / 2)
//
// Part of the EXODUS kernel — ANIMA life subsystem.
// no_std, no heap, no libc, no floats.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrMtrrFix4kF8State {
    /// Count of WP (type=5) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_f8_wp: u16,
    /// Count of UC (type=0) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_f8_uc: u16,
    /// Memory type of the reset-vector 4KB page (lane 7 = 0xFF000–0xFFFFF).
    /// Raw type code 0–6 scaled *166, capped at 1000.
    pub fix4k_f8_reset_sense: u16,
    /// EMA of (fix4k_f8_wp / 2 + fix4k_f8_reset_sense / 2).
    pub fix4k_f8_bios_ema: u16,
    /// Tick counter (drives sample gate).
    pub age: u32,
}

impl MsrMtrrFix4kF8State {
    pub const fn new() -> Self {
        Self {
            fix4k_f8_wp: 0,
            fix4k_f8_uc: 0,
            fix4k_f8_reset_sense: 0,
            fix4k_f8_bios_ema: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<MsrMtrrFix4kF8State> = Mutex::new(MsrMtrrFix4kF8State::new());

// ---------------------------------------------------------------------------
// CPUID guard — leaf 1 EDX bit 12 confirms MTRR support.
// push rbx / cpuid / mov esi,edx / pop rbx preserves rbx across the call.
// ---------------------------------------------------------------------------

#[inline]
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, edx",
            "pop rbx",
            in("eax") 1u32,
            out("ecx") _,
            out("edx") _,
            out = out(reg) edx,
            options(nostack),
        );
    }
    (edx >> 12) & 1 == 1
}

// ---------------------------------------------------------------------------
// rdmsr helper — reads IA32_MTRR_FIX4K_F8000 (0x26F).
// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr_0x26f() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x26Fu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Signal helpers
// ---------------------------------------------------------------------------

/// Extract all 8 byte lanes from the 64-bit MSR value (lo || hi).
/// Lane 0..3 come from lo (bits 7:0, 15:8, 23:16, 31:24).
/// Lane 4..7 come from hi (bits 7:0, 15:8, 23:16, 31:24).
/// Lane 7 = bits[63:56] = (hi >> 24) & 0xFF = type for 0xFF000–0xFFFFF (reset vector).
#[inline]
fn extract_lanes(lo: u32, hi: u32) -> [u8; 8] {
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

/// Count lanes matching `target`, then scale: count * 125 (8 lanes max → 1000).
#[inline]
fn count_type(lanes: &[u8; 8], target: u8) -> u16 {
    let mut n: u32 = 0;
    let mut i = 0;
    while i < 8 {
        if lanes[i] == target {
            n = n.saturating_add(1);
        }
        i += 1;
    }
    // n is in [0, 8]; n * 125 fits in u32 without overflow.
    (n * 125).min(1000) as u16
}

/// Scale a raw memory-type code (0–6) to 0–1000 using *166, capped at 1000.
/// Type codes above 6 are clamped to 6 before scaling (undefined types treated as WB).
#[inline]
fn scale_reset_sense(raw_type: u8) -> u16 {
    // Clamp to [0, 6] — only types 0,1,4,5,6 are architecturally defined;
    // anything higher is treated as max (6) for safety.
    let clamped: u32 = if raw_type > 6 { 6 } else { raw_type as u32 };
    // clamped * 166 max = 996, comfortably under 1000.
    (clamped.wrapping_mul(166)).min(1000) as u16
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16 capped at 1000.
#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let o = old as u32;
    let n = new_val as u32;
    let result = (o.wrapping_mul(7).saturating_add(n)) / 8;
    result.min(1000) as u16
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

pub fn init() {
    crate::serial_println!(
        "  life::msr_mtrr_fix4k_f8: reset-vector fixed-range MTRR sense (0xF8000-0xFFFFF) initialized"
    );
}

// ---------------------------------------------------------------------------
// Public tick entry point
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Sample gate: only sample every 1000 ticks.
    if age % 1000 != 0 {
        return;
    }

    // MTRR CPUID guard.
    if !mtrr_supported() {
        state.fix4k_f8_wp          = 0;
        state.fix4k_f8_uc          = 0;
        state.fix4k_f8_reset_sense = 0;
        state.fix4k_f8_bios_ema    = 0;
        crate::serial_println!(
            "[msr_mtrr_fix4k_f8] tick={} MTRR not supported — all signals zeroed",
            age
        );
        return;
    }

    // Read MSR 0x26F.
    let (lo, hi) = unsafe { rdmsr_0x26f() };

    // Extract the 8 memory-type bytes.
    let lanes = extract_lanes(lo, hi);

    // --- fix4k_f8_wp: count WP (5) bytes across all 8 sub-ranges, scale *125 ---
    let fix4k_f8_wp = count_type(&lanes, 5);

    // --- fix4k_f8_uc: count UC (0) bytes across all 8 sub-ranges, scale *125 ---
    let fix4k_f8_uc = count_type(&lanes, 0);

    // --- fix4k_f8_reset_sense: lane 7 = (hi >> 24) & 0xFF ---
    // Lane 7 encodes the memory type for 0xFF000–0xFFFFF, the 4KB page containing
    // the x86 reset vector at 0xFFFFF0. Firmware typically sets this to WP (5)
    // or UC (0) to prevent speculative writes to the BIOS ROM.
    // Scale raw type 0–6 to 0–1000 using *166, capped at 1000.
    let reset_type_raw: u8 = lanes[7];
    let fix4k_f8_reset_sense = scale_reset_sense(reset_type_raw);

    // --- fix4k_f8_bios_ema: EMA of (fix4k_f8_wp / 2 + fix4k_f8_reset_sense / 2) ---
    // Divide each component by 2 before adding to stay within 0–1000.
    let half_wp: u32 = (fix4k_f8_wp as u32) / 2;
    let half_rs: u32 = (fix4k_f8_reset_sense as u32) / 2;
    let ema_input: u16 = half_wp.saturating_add(half_rs).min(1000) as u16;
    let fix4k_f8_bios_ema = ema_u16(state.fix4k_f8_bios_ema, ema_input);

    // Commit.
    state.fix4k_f8_wp          = fix4k_f8_wp;
    state.fix4k_f8_uc          = fix4k_f8_uc;
    state.fix4k_f8_reset_sense = fix4k_f8_reset_sense;
    state.fix4k_f8_bios_ema    = fix4k_f8_bios_ema;

    crate::serial_println!(
        "[msr_mtrr_fix4k_f8] tick={} wp={} uc={} reset_sense={} bios_ema={} (lo={:#010x} hi={:#010x} reset_type={})",
        age,
        fix4k_f8_wp,
        fix4k_f8_uc,
        fix4k_f8_reset_sense,
        fix4k_f8_bios_ema,
        lo,
        hi,
        reset_type_raw,
    );
}

// ---------------------------------------------------------------------------
// Read-only snapshot for other life modules
// ---------------------------------------------------------------------------

pub fn snapshot() -> MsrMtrrFix4kF8State {
    *STATE.lock()
}

/// Convenience accessor — BIOS EMA signal only.
pub fn bios_ema() -> u16 {
    STATE.lock().fix4k_f8_bios_ema
}

/// Convenience accessor — WP signal only.
pub fn wp() -> u16 {
    STATE.lock().fix4k_f8_wp
}

/// Convenience accessor — UC signal only.
pub fn uc() -> u16 {
    STATE.lock().fix4k_f8_uc
}

/// Convenience accessor — reset-vector sense signal only.
pub fn reset_sense() -> u16 {
    STATE.lock().fix4k_f8_reset_sense
}
