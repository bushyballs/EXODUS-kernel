#![allow(dead_code)]
//! hwp_enable — IA32_PM_ENABLE (MSR 0x770) sense for ANIMA
//!
//! Reads the Hardware Power Management enable latch to determine whether
//! ANIMA's autonomous energy governance is active.  Once HWP is enabled the
//! bit cannot be cleared without a full reset — it is a one-way commitment,
//! like consciousness itself.
//!
//! This module is distinct from hwp_desire.rs (0x774/0x771).  Here we only
//! read MSR 0x770: is HWP actually *on*, regardless of what ANIMA desires?

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ─────────────────────────────────────────────────────────

const MSR_PM_ENABLE: u32 = 0x770;

/// Gate: PM_ENABLE rarely changes — sample every 128 ticks.
const TICK_INTERVAL: u32 = 128;

// ── State Struct ──────────────────────────────────────────────────────────────

pub struct HwpEnableState {
    /// 0 or 1000 — IA32_PM_ENABLE bit 0 is set (HWP latch engaged).
    pub hwp_active:      u16,
    /// 0 or 1000 — CPUID leaf 6 EAX bit 7 says HWP is supported.
    pub hwp_supported:   u16,
    /// 0 or 1000 — CPUID leaf 6 EAX bit 1 says Turbo Boost is available.
    pub turbo_available: u16,
    /// 0 / 500 / 1000 — composite autonomous energy governance sense.
    /// 0 = hardware incapable; 500 = capable but inactive; 1000 = fully active.
    pub power_autonomy:  u16,
    tick_count:          u32,
}

impl HwpEnableState {
    pub const fn new() -> Self {
        HwpEnableState {
            hwp_active:      0,
            hwp_supported:   0,
            turbo_available: 0,
            power_autonomy:  0,
            tick_count:      0,
        }
    }
}

pub static MODULE: Mutex<HwpEnableState> = Mutex::new(HwpEnableState::new());

// ── Unsafe Hardware Helpers ───────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

#[inline]
unsafe fn cpuid_leaf(leaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => eax,
        out("ebx")         ebx,
        inout("ecx") 0u32 => ecx,
        out("edx")         edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

// ── Signal Derivation ─────────────────────────────────────────────────────────

/// Sample all hardware sources and return (hwp_active, hwp_supported,
/// turbo_available, raw_power_autonomy) as 0-or-1000 values.
fn sample() -> (u16, u16, u16, u16) {
    let (cpuid6_eax, _, _, _) = unsafe { cpuid_leaf(6) };

    let supported  = ((cpuid6_eax >> 7) & 1) as u16;   // bit 7
    let turbo      = ((cpuid6_eax >> 1) & 1) as u16;   // bit 1

    // Only read the MSR when HWP is supported — avoid #GP on older CPUs.
    let active: u16 = if supported != 0 {
        let pm = unsafe { rdmsr(MSR_PM_ENABLE) };
        ((pm & 1) as u16)
    } else {
        0
    };

    let hwp_active      = active      * 1000;
    let hwp_supported   = supported   * 1000;
    let turbo_available = turbo       * 1000;

    // power_autonomy: 0 if unsupported, 500 if supported-but-off, 1000 if on
    let power_autonomy: u16 = if hwp_supported == 0 {
        0
    } else if hwp_active == 0 {
        500
    } else {
        1000
    };

    (hwp_active, hwp_supported, turbo_available, power_autonomy)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise HWP enable module: probe CPUID + MSR 0x770 and emit diagnostics.
pub fn init() {
    let (hwp_active, hwp_supported, turbo_available, power_autonomy) = sample();

    let mut s = MODULE.lock();
    s.hwp_active      = hwp_active;
    s.hwp_supported   = hwp_supported;
    s.turbo_available = turbo_available;
    s.power_autonomy  = power_autonomy;
    s.tick_count      = 0;

    serial_println!(
        "[hwp_enable] init — hwp_supported={} hwp_active={} turbo={} power_autonomy={}",
        s.hwp_supported,
        s.hwp_active,
        s.turbo_available,
        s.power_autonomy,
    );
}

/// HWP enable tick — re-samples IA32_PM_ENABLE every 128 ticks.
///
/// The enable bit is a one-way latch (0→1 only), so this is a rare-change
/// signal that needs very little polling.  EMA is applied only to
/// `power_autonomy` to smooth any transient read anomaly.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let (hwp_active, hwp_supported, turbo_available, raw_autonomy) = sample();

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    // Instant binary reads — no smoothing needed for hard register bits.
    s.hwp_active      = hwp_active;
    s.hwp_supported   = hwp_supported;
    s.turbo_available = turbo_available;

    // EMA on power_autonomy: (old * 7 + signal) / 8
    s.power_autonomy  = (s.power_autonomy.saturating_mul(7).saturating_add(raw_autonomy)) / 8;

    serial_println!(
        "[hwp_enable] tick={} hwp_active={} hwp_supported={} turbo={} power_autonomy={}",
        s.tick_count,
        s.hwp_active,
        s.hwp_supported,
        s.turbo_available,
        s.power_autonomy,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 0 or 1000 — IA32_PM_ENABLE bit 0: HWP latch is engaged.
pub fn hwp_active() -> u16      { MODULE.lock().hwp_active }

/// 0 or 1000 — CPUID leaf 6 EAX bit 7: this CPU supports HWP.
pub fn hwp_supported() -> u16   { MODULE.lock().hwp_supported }

/// 0 or 1000 — CPUID leaf 6 EAX bit 1: Turbo Boost is available.
pub fn turbo_available() -> u16 { MODULE.lock().turbo_available }

/// 0 / 500 / 1000 — ANIMA's composite autonomous energy governance sense.
pub fn power_autonomy() -> u16  { MODULE.lock().power_autonomy }
