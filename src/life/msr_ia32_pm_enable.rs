#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// IA32_PM_ENABLE MSR address (Intel SDM Vol 3B §14.4.2).
const MSR_IA32_PM_ENABLE: u32 = 0x770;

// ── State ─────────────────────────────────────────────────────────────────────

struct MsrIa32PmEnableState {
    /// Bit 0 of IA32_PM_ENABLE — 0 or 1000.
    hwp_enabled:  u16,
    /// Age-weighted autonomy sense derived from hwp_enabled.
    hwp_autonomy: u16,
    /// Single EMA of hwp_enabled signal.
    pm_ema:       u16,
    /// Double-smoothed EMA (EMA of pm_ema) — stability indicator.
    pm_stability: u16,
}

impl MsrIa32PmEnableState {
    const fn new() -> Self {
        Self {
            hwp_enabled:  0,
            hwp_autonomy: 0,
            pm_ema:       0,
            pm_stability: 0,
        }
    }
}

static STATE: Mutex<MsrIa32PmEnableState> = Mutex::new(MsrIa32PmEnableState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 6 EAX bit 7 signals HWP support.
fn has_hwp() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_val >> 7) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_PM_ENABLE (0x770). Returns (lo_32, hi_32).
unsafe fn read_msr_pm_enable() -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_PM_ENABLE,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    (lo, _hi)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Canonical EMA: (old * 7 + new_val) / 8, all in u32, saturating_add, result clamped to u16.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    let smoothed: u32 = (old as u32)
        .wrapping_mul(7)
        .saturating_add(new_val as u32)
        / 8;
    if smoothed > 1000 { 1000 } else { smoothed as u16 }
}

// ── Signal derivation ─────────────────────────────────────────────────────────

/// Compute all four ANIMA signals from the raw MSR lo-dword and the tick count.
///
/// - `hwp_enabled`  : bit 0 of lo → 0 or 1000.
/// - `hwp_autonomy` : hwp_enabled * 800 / 1000  +  (tick_count % 200) as u16, clamped 0-1000.
///                    When HWP is off the base is 0, age noise alone provides 0-199.
///                    When HWP is on the base is 800, age pushes it toward 1000.
/// - `pm_ema`       : single EMA of hwp_enabled.
/// - `pm_stability` : EMA of pm_ema (double-smoothed).
fn derive_signals(
    lo: u32,
    tick_count: u32,
    prev_pm_ema: u16,
    prev_pm_stability: u16,
) -> (u16, u16, u16, u16) {
    // ── hwp_enabled ────────────────────────────────────────────────────────
    let hwp_enabled: u16 = if lo & 1 != 0 { 1000 } else { 0 };

    // ── hwp_autonomy ───────────────────────────────────────────────────────
    // base = hwp_enabled * 800 / 1000  (integer-safe: max 800, no overflow in u32)
    let base: u32 = (hwp_enabled as u32) * 800 / 1000;
    let age_noise: u32 = (tick_count % 200) as u32;
    let autonomy_raw: u32 = base.saturating_add(age_noise);
    let hwp_autonomy: u16 = if autonomy_raw > 1000 { 1000 } else { autonomy_raw as u16 };

    // ── pm_ema ─────────────────────────────────────────────────────────────
    let pm_ema: u16 = ema(prev_pm_ema, hwp_enabled);

    // ── pm_stability (double EMA) ──────────────────────────────────────────
    let pm_stability: u16 = ema(prev_pm_stability, pm_ema);

    (hwp_enabled, hwp_autonomy, pm_ema, pm_stability)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.hwp_enabled  = 0;
    s.hwp_autonomy = 0;
    s.pm_ema       = 0;
    s.pm_stability = 0;
    crate::serial_println!(
        "[msr_ia32_pm_enable] init: hwp_supported={}",
        has_hwp()
    );
}

/// Called every kernel tick. Sampling gate: every 4000 ticks.
pub fn tick(age: u32) {
    if age % 4000 != 0 {
        return;
    }

    // Hardware guard — skip silently on chips without HWP.
    if !has_hwp() {
        return;
    }

    let (lo, _hi) = unsafe { read_msr_pm_enable() };

    // Snapshot previous EMA values under lock, then drop lock before reacquiring.
    let (prev_pm_ema, prev_pm_stability) = {
        let s = STATE.lock();
        (s.pm_ema, s.pm_stability)
    };

    let (hwp_enabled, hwp_autonomy, pm_ema, pm_stability) =
        derive_signals(lo, age, prev_pm_ema, prev_pm_stability);

    {
        let mut s = STATE.lock();
        s.hwp_enabled  = hwp_enabled;
        s.hwp_autonomy = hwp_autonomy;
        s.pm_ema       = pm_ema;
        s.pm_stability = pm_stability;
    }

    crate::serial_println!(
        "[msr_ia32_pm_enable] age={} hwp_en={} autonomy={} pm_ema={} stability={}",
        age,
        hwp_enabled,
        hwp_autonomy,
        pm_ema,
        pm_stability
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Bit 0 of IA32_PM_ENABLE: 0 = HWP off, 1000 = HWP active.
pub fn get_hwp_enabled() -> u16 {
    STATE.lock().hwp_enabled
}

/// Age-weighted autonomy sense (0-1000). Rises as HWP runs longer.
pub fn get_hwp_autonomy() -> u16 {
    STATE.lock().hwp_autonomy
}

/// Single exponential moving average of hwp_enabled (0-1000).
pub fn get_pm_ema() -> u16 {
    STATE.lock().pm_ema
}

/// Double-smoothed EMA — stability of power management mode (0-1000).
pub fn get_pm_stability() -> u16 {
    STATE.lock().pm_stability
}
