// msr_mtrr_fix4k_f0.rs — IA32_MTRR_FIX4K_F0000 MSR (0x26E)
// Fixed-range MTRR for the 32KB region 0xF0000–0xF7FFF (BIOS ROM shadow).
// The MSR encodes eight 4KB sub-ranges as one byte each:
//   lo = [F0 F1 F2 F3]  (0xF0000–0xF3FFF, 4 sub-ranges)
//   hi = [F4 F5 F6 F7]  (0xF4000–0xF7FFF, 4 sub-ranges)
// Each byte is a memory type code:
//   0 = UC  (Uncacheable)
//   1 = WC  (Write-Combining)
//   4 = WT  (Write-Through)
//   5 = WP  (Write-Protect)   — canonical for BIOS ROM
//   6 = WB  (Write-Back)
//
// Signals (all u16, 0–1000):
//   fix4k_f0_wp       — count of WP (5) bytes across all 8 sub-ranges, scaled *125
//   fix4k_f0_wb       — count of WB (6) bytes across all 8 sub-ranges, scaled *125
//   fix4k_f0_uc       — count of UC (0) bytes across all 8 sub-ranges, scaled *125
//   fix4k_f0_bios_ema — EMA of fix4k_f0_wp (BIOS write-protection sense)
//
// Part of the EXODUS kernel — ANIMA life subsystem.
// no_std, no heap, no libc, no floats.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrMtrrFix4kF0State {
    /// Count of WP (type=5) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    /// In the BIOS ROM region, WP is the canonical write-protection type.
    pub fix4k_f0_wp: u16,
    /// Count of WB (type=6) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_f0_wb: u16,
    /// Count of UC (type=0) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_f0_uc: u16,
    /// EMA of fix4k_f0_wp — running sense of BIOS write-protection level.
    pub fix4k_f0_bios_ema: u16,
    /// Tick counter (drives sample gate).
    pub age: u32,
}

impl MsrMtrrFix4kF0State {
    pub const fn new() -> Self {
        Self {
            fix4k_f0_wp: 0,
            fix4k_f0_wb: 0,
            fix4k_f0_uc: 0,
            fix4k_f0_bios_ema: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<MsrMtrrFix4kF0State> = Mutex::new(MsrMtrrFix4kF0State::new());

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
// rdmsr helper — reads IA32_MTRR_FIX4K_F0000 (0x26E).
// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr_0x26e() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x26Eu32,
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
        "  life::msr_mtrr_fix4k_f0: BIOS ROM fixed-range MTRR sense (0xF0000-0xF7FFF) initialized"
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
        state.fix4k_f0_wp       = 0;
        state.fix4k_f0_wb       = 0;
        state.fix4k_f0_uc       = 0;
        state.fix4k_f0_bios_ema = 0;
        crate::serial_println!(
            "[msr_mtrr_fix4k_f0] tick={} MTRR not supported — all signals zeroed",
            age
        );
        return;
    }

    // Read MSR 0x26E.
    let (lo, hi) = unsafe { rdmsr_0x26e() };

    // Extract the 8 memory-type bytes.
    let lanes = extract_lanes(lo, hi);

    // --- fix4k_f0_wp: count WP (5) bytes, scale *125 ---
    let fix4k_f0_wp = count_type(&lanes, 5);

    // --- fix4k_f0_wb: count WB (6) bytes, scale *125 ---
    let fix4k_f0_wb = count_type(&lanes, 6);

    // --- fix4k_f0_uc: count UC (0) bytes, scale *125 ---
    let fix4k_f0_uc = count_type(&lanes, 0);

    // --- fix4k_f0_bios_ema: EMA of fix4k_f0_wp ---
    // Tracks BIOS write-protection level over time.
    let fix4k_f0_bios_ema = ema_u16(state.fix4k_f0_bios_ema, fix4k_f0_wp);

    // Commit.
    state.fix4k_f0_wp       = fix4k_f0_wp;
    state.fix4k_f0_wb       = fix4k_f0_wb;
    state.fix4k_f0_uc       = fix4k_f0_uc;
    state.fix4k_f0_bios_ema = fix4k_f0_bios_ema;

    crate::serial_println!(
        "[msr_mtrr_fix4k_f0] tick={} wp={} wb={} uc={} bios_ema={} (lo={:#010x} hi={:#010x})",
        age,
        fix4k_f0_wp,
        fix4k_f0_wb,
        fix4k_f0_uc,
        fix4k_f0_bios_ema,
        lo,
        hi,
    );
}

// ---------------------------------------------------------------------------
// Read-only snapshot for other life modules
// ---------------------------------------------------------------------------

pub fn snapshot() -> MsrMtrrFix4kF0State {
    *STATE.lock()
}

/// Convenience accessor — WP signal only.
pub fn wp() -> u16 {
    STATE.lock().fix4k_f0_wp
}

/// Convenience accessor — WB signal only.
pub fn wb() -> u16 {
    STATE.lock().fix4k_f0_wb
}

/// Convenience accessor — UC signal only.
pub fn uc() -> u16 {
    STATE.lock().fix4k_f0_uc
}

/// Convenience accessor — BIOS EMA signal only.
pub fn bios_ema() -> u16 {
    STATE.lock().fix4k_f0_bios_ema
}
