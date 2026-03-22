#![allow(dead_code)]

use crate::sync::Mutex;

// ── Constants ──────────────────────────────────────────────────────────────────

/// MSR_PLATFORM_INFO — address 0xCE.
/// lo bits[15:8]  = Maximum Non-Turbo Ratio (base max multiplier).
/// hi bits[15:8]  = Minimum Operating Ratio (bits[47:40] of full 64-bit register).
/// lo bit 28      = PROG_TDP_LIM  — programmable TDP limits supported.
/// lo bit 29      = PROG_RATIO_LIM — programmable ratio limits supported.
const MSR_PLATFORM_INFO_ADDR: u32 = 0xCE;

/// Tick sampling interval — mostly static, so read rarely.
const TICK_GATE: u32 = 6000;

// ── State ─────────────────────────────────────────────────────────────────────

struct MsrIa32PlatformInfo2State {
    /// bits[15:8] of lo, scaled * 1000 / 255 → 0–1000.
    platform_max_ratio: u16,
    /// bits[15:8] of hi (bits[47:40] of full MSR), scaled same.
    platform_min_ratio: u16,
    /// 1000 if PROG_TDP_LIM or PROG_RATIO_LIM is set (lo bits 28–29), else 0.
    platform_prog_support: u16,
    /// EMA of (max_ratio/4 + min_ratio/4 + prog_support/2).
    platform_ratio_ema: u16,
}

impl MsrIa32PlatformInfo2State {
    const fn new() -> Self {
        Self {
            platform_max_ratio:    0,
            platform_min_ratio:    0,
            platform_prog_support: 0,
            platform_ratio_ema:    0,
        }
    }
}

static STATE: Mutex<MsrIa32PlatformInfo2State> =
    Mutex::new(MsrIa32PlatformInfo2State::new());

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read MSR 0xCE — MSR_PLATFORM_INFO.
/// Returns (lo, hi): the low and high 32-bit halves of the 64-bit MSR value.
/// Safety: requires ring-0 privilege; always-readable on modern Intel CPUs.
#[inline]
unsafe fn rdmsr_platform_info() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_PLATFORM_INFO_ADDR,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Scale an 8-bit ratio multiplier to the 0–1000 signal range.
/// Formula: val * 1000 / 255, capped at 1000.
/// Uses u32 intermediate to avoid overflow (max = 255 * 1000 = 255_000).
#[inline]
fn scale_ratio(raw: u32) -> u16 {
    let scaled = raw.wrapping_mul(1000) / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// EMA: ((old * 7) saturating_add new_val) / 8, all in u32 then cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    *s = MsrIa32PlatformInfo2State::new();
    crate::serial_println!(
        "[msr_ia32_platform_info2] init: MSR 0xCE extended platform-info module ready"
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    // Safety: kernel ring-0; MSR 0xCE is always readable on modern Intel.
    let (lo, hi) = unsafe { rdmsr_platform_info() };

    // Maximum Non-Turbo Ratio — bits[15:8] of lo register.
    let max_raw = (lo >> 8) & 0xFF;
    let platform_max_ratio = scale_ratio(max_raw);

    // Minimum Operating Ratio — bits[15:8] of hi register (= bits[47:40] of full MSR).
    let min_raw = (hi >> 8) & 0xFF;
    let platform_min_ratio = scale_ratio(min_raw);

    // Programmable limits support — lo bits 28 (PROG_TDP_LIM) and 29 (PROG_RATIO_LIM).
    let prog_bits = (lo >> 28) & 0x3;
    let platform_prog_support: u16 = if prog_bits != 0 { 1000 } else { 0 };

    // Composite: max_ratio/4 + min_ratio/4 + prog_support/2.
    // All operands are 0–1000, so sum fits in u16 without overflow (max = 250 + 250 + 500 = 1000).
    let composite: u16 = (platform_max_ratio / 4)
        .saturating_add(platform_min_ratio / 4)
        .saturating_add(platform_prog_support / 2);

    let mut s = STATE.lock();

    let platform_ratio_ema = ema(s.platform_ratio_ema, composite);

    s.platform_max_ratio    = platform_max_ratio;
    s.platform_min_ratio    = platform_min_ratio;
    s.platform_prog_support = platform_prog_support;
    s.platform_ratio_ema    = platform_ratio_ema;

    crate::serial_println!(
        "[msr_ia32_platform_info2] age={} max_ratio={} min_ratio={} prog_support={} ratio_ema={}",
        age,
        platform_max_ratio,
        platform_min_ratio,
        platform_prog_support,
        platform_ratio_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Base maximum clock multiplier (bits[15:8] of MSR 0xCE lo), scaled 0–1000.
pub fn get_platform_max_ratio() -> u16 {
    STATE.lock().platform_max_ratio
}

/// Minimum P-state ratio (bits[47:40] of MSR 0xCE, i.e. bits[15:8] of hi), scaled 0–1000.
pub fn get_platform_min_ratio() -> u16 {
    STATE.lock().platform_min_ratio
}

/// 1000 if PROG_TDP_LIM or PROG_RATIO_LIM is set (lo bits 28–29), else 0.
pub fn get_platform_prog_support() -> u16 {
    STATE.lock().platform_prog_support
}

/// EMA of (max_ratio/4 + min_ratio/4 + prog_support/2), updated every 6000 ticks.
pub fn get_platform_ratio_ema() -> u16 {
    STATE.lock().platform_ratio_ema
}
