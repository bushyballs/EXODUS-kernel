#![allow(dead_code)]

use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct MsrPlatformInfoState {
    max_ratio:           u16,
    min_ratio:           u16,
    ratio_range:         u16,
    platform_info_ema:   u16,
}

impl MsrPlatformInfoState {
    const fn new() -> Self {
        Self {
            max_ratio:         0,
            min_ratio:         0,
            ratio_range:       0,
            platform_info_ema: 0,
        }
    }
}

static STATE: Mutex<MsrPlatformInfoState> = Mutex::new(MsrPlatformInfoState::new());

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read MSR_PLATFORM_INFO (0xCE).
/// Returns (lo, hi) — the low and high 32-bit halves of the 64-bit MSR.
#[inline]
unsafe fn rdmsr_platform_info() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0xCEu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    *s = MsrPlatformInfoState::new();
    crate::serial_println!("[msr_platform_info] init: MSR 0xCE platform-info module ready");
}

pub fn tick(age: u32) {
    // Sample every 6000 ticks
    if age % 6000 != 0 {
        return;
    }

    // Safety: RDMSR requires ring-0 privilege; we are in kernel context.
    let (lo, hi) = unsafe { rdmsr_platform_info() };

    // bits [15:8] of lo = Maximum Non-Turbo Ratio
    let max_raw = (lo >> 8) & 0xFF;
    // bits [15:8] of hi = Minimum Operating Ratio
    let min_raw = (hi >> 8) & 0xFF;

    // Map to 0–1000 (multiply by 10, cap at 1000)
    let max_ratio: u16 = ((max_raw * 10).min(1000)) as u16;
    let min_ratio: u16 = ((min_raw * 10).min(1000)) as u16;

    // Frequency window width (saturating so we never wrap)
    let ratio_range: u16 = max_ratio.saturating_sub(min_ratio);

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32 to avoid overflow
    let ema_new: u16 = {
        let old = s.platform_info_ema as u32;
        let new_val = ratio_range as u32;
        ((old * 7 + new_val) / 8) as u16
    };

    s.max_ratio         = max_ratio;
    s.min_ratio         = min_ratio;
    s.ratio_range       = ratio_range;
    s.platform_info_ema = ema_new;

    crate::serial_println!(
        "[msr_platform_info] age={} max_ratio={} min_ratio={} range={} ema={}",
        age,
        max_ratio,
        min_ratio,
        ratio_range,
        ema_new,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_max_ratio() -> u16 {
    STATE.lock().max_ratio
}

pub fn get_min_ratio() -> u16 {
    STATE.lock().min_ratio
}

pub fn get_ratio_range() -> u16 {
    STATE.lock().ratio_range
}

pub fn get_platform_info_ema() -> u16 {
    STATE.lock().platform_info_ema
}
