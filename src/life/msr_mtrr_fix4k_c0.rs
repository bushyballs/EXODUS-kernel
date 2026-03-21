// msr_mtrr_fix4k_c0.rs — IA32_MTRR_FIX4K_C0000 MSR (0x268)
// Fixed-range MTRR for the 32KB region 0xC0000–0xC7FFF.
// The MSR encodes eight 4KB sub-ranges as one byte each:
//   lo = [C0 C1 C2 C3]  (0xC0000–0xC3FFF, 4 sub-ranges)
//   hi = [C4 C5 C6 C7]  (0xC4000–0xC7FFF, 4 sub-ranges)
// Each byte is a memory type code:
//   0 = UC  (Uncacheable)
//   1 = WC  (Write-Combining)
//   4 = WT  (Write-Through)
//   5 = WP  (Write-Protect)   — typical for ISA/ROM
//   6 = WB  (Write-Back)
//
// Signals (all u16, 0–1000):
//   fix4k_c0_wp  — count of WP (5) bytes across all 8 sub-ranges, scaled *125
//   fix4k_c0_wb  — count of WB (6) bytes across all 8 sub-ranges, scaled *125
//   fix4k_c0_uc  — count of UC (0) bytes across all 8 sub-ranges, scaled *125
//   fix4k_c0_ema — EMA of ((lo ^ hi).count_ones() * 31).min(1000)
//
// Part of the EXODUS kernel — ANIMA life subsystem.
// no_std, no heap, no libc, no floats.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrMtrrFix4kC0State {
    /// Count of WP (type=5) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_c0_wp: u16,
    /// Count of WB (type=6) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_c0_wb: u16,
    /// Count of UC (type=0) bytes across all 8 sub-ranges, scaled 0–1000 (*125).
    pub fix4k_c0_uc: u16,
    /// EMA of ((lo ^ hi).count_ones() * 31).min(1000).
    pub fix4k_c0_ema: u16,
    /// Tick counter (drives sample gate).
    pub age: u32,
}

impl MsrMtrrFix4kC0State {
    pub const fn new() -> Self {
        Self {
            fix4k_c0_wp: 0,
            fix4k_c0_wb: 0,
            fix4k_c0_uc: 0,
            fix4k_c0_ema: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<MsrMtrrFix4kC0State> = Mutex::new(MsrMtrrFix4kC0State::new());

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
// rdmsr helper — reads IA32_MTRR_FIX4K_C0000 (0x268).
// Returns (lo, hi): lo = bits[31:0], hi = bits[63:32].
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr_0x268() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x268u32,
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
        "  life::msr_mtrr_fix4k_c0: ISA/ROM fixed-range MTRR sense (0xC0000-0xC7FFF) initialized"
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
        state.fix4k_c0_wp  = 0;
        state.fix4k_c0_wb  = 0;
        state.fix4k_c0_uc  = 0;
        state.fix4k_c0_ema = 0;
        crate::serial_println!(
            "[msr_mtrr_fix4k_c0] tick={} MTRR not supported — all signals zeroed",
            age
        );
        return;
    }

    // Read MSR 0x268.
    let (lo, hi) = unsafe { rdmsr_0x268() };

    // Extract the 8 memory-type bytes.
    let lanes = extract_lanes(lo, hi);

    // --- fix4k_c0_wp: count WP (5) bytes, scale *125 ---
    let fix4k_c0_wp = count_type(&lanes, 5);

    // --- fix4k_c0_wb: count WB (6) bytes, scale *125 ---
    let fix4k_c0_wb = count_type(&lanes, 6);

    // --- fix4k_c0_uc: count UC (0) bytes, scale *125 ---
    let fix4k_c0_uc = count_type(&lanes, 0);

    // --- fix4k_c0_ema: EMA of ((lo ^ hi).count_ones() * 31).min(1000) ---
    let xor_val: u32 = lo ^ hi;
    let ones: u32 = xor_val.count_ones();
    // ones is in [0, 32]; * 31 = 992 max, comfortably under 1000.
    let instant_ema_input: u16 = ((ones.wrapping_mul(31)).min(1000)) as u16;
    let fix4k_c0_ema = ema_u16(state.fix4k_c0_ema, instant_ema_input);

    // Commit.
    state.fix4k_c0_wp  = fix4k_c0_wp;
    state.fix4k_c0_wb  = fix4k_c0_wb;
    state.fix4k_c0_uc  = fix4k_c0_uc;
    state.fix4k_c0_ema = fix4k_c0_ema;

    crate::serial_println!(
        "[msr_mtrr_fix4k_c0] tick={} wp={} wb={} uc={} ema={} (lo={:#010x} hi={:#010x})",
        age,
        fix4k_c0_wp,
        fix4k_c0_wb,
        fix4k_c0_uc,
        fix4k_c0_ema,
        lo,
        hi,
    );
}

// ---------------------------------------------------------------------------
// Read-only snapshot for other life modules
// ---------------------------------------------------------------------------

pub fn snapshot() -> MsrMtrrFix4kC0State {
    *STATE.lock()
}

/// Convenience accessor — EMA signal only.
pub fn ema() -> u16 {
    STATE.lock().fix4k_c0_ema
}

/// Convenience accessor — WP signal only.
pub fn wp() -> u16 {
    STATE.lock().fix4k_c0_wp
}

/// Convenience accessor — WB signal only.
pub fn wb() -> u16 {
    STATE.lock().fix4k_c0_wb
}

/// Convenience accessor — UC signal only.
pub fn uc() -> u16 {
    STATE.lock().fix4k_c0_uc
}
