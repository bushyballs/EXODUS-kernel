#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

/// MSR_TURBO_ACTIVATION_RATIO — Intel SDM Vol. 4 §2.1
/// Address 0x64C
///   lo bits[7:0]  = MAX_NON_TURBO_RATIO: highest non-turbo multiplier at which
///                   all-core turbo engagement is still permitted.
///   lo bit 31     = TURBO_ACTIVATION_RATIO_LOCK: when set, this register is
///                   permanently write-locked by firmware.
const MSR_TURBO_ACTIVATION_RATIO: u32 = 0x64C;

/// Re-read every 5000 ticks — hardware register changes only on firmware writes.
const TICK_GATE: u32 = 5000;

/// MAX_NON_TURBO_RATIO is an 8-bit field, maximum representable value 255.
/// We scale it to 0-1000 with: val * 1000 / 255.
const RATIO_FIELD_MAX: u32 = 255;

pub struct State {
    /// Scaled turbo engagement threshold (0-1000).
    /// Derived from MAX_NON_TURBO_RATIO bits[7:0], scaled val*1000/255.
    pub turbo_threshold_ratio: u16,

    /// 0 if the lock bit (lo bit 31) is clear, 1000 if set.
    /// When 1000, firmware has permanently fixed the activation ratio.
    pub turbo_locked: u16,

    /// Headroom above threshold: (1000 - turbo_threshold_ratio).min(1000).
    /// High = ANIMA has wide turbo engagement range; low = ratio nearly maxed.
    pub turbo_headroom: u16,

    /// Exponential moving average of turbo_threshold_ratio.
    /// Weight: 7/8 old + 1/8 new (slow follower — hardware almost never changes).
    pub turbo_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    turbo_threshold_ratio: 0,
    turbo_locked:          0,
    turbo_headroom:        0,
    turbo_ema:             0,
});

/// CPUID leaf 6 EAX bit 1 — Intel Turbo Boost Technology available.
fn has_turbo() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 1) & 1 == 1
}

/// Execute RDMSR; returns (lo, hi) 32-bit halves.
fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

/// Scale an 8-bit ratio field to 0-1000.
/// Formula: val * 1000 / 255, clamped to 1000.
fn scale_ratio(val: u32) -> u16 {
    (val.wrapping_mul(1000) / RATIO_FIELD_MAX).min(1000) as u16
}

/// EMA: 7/8 old + 1/8 new (integer approximation).
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Initialise the module to zero state and log.
pub fn init() {
    let mut s = MODULE.lock();
    s.turbo_threshold_ratio = 0;
    s.turbo_locked          = 0;
    s.turbo_headroom        = 0;
    s.turbo_ema             = 0;
    serial_println!("[msr_ia32_turbo_activation_ratio] init");
}

/// Called every kernel tick. Reads MSR 0x64C every TICK_GATE ticks.
/// No-ops silently when Turbo Boost is not supported by this CPU.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_turbo() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_TURBO_ACTIVATION_RATIO);

    // bits[7:0] — MAX_NON_TURBO_RATIO
    let raw_ratio: u32 = lo & 0xFF;

    // bit 31 — TURBO_ACTIVATION_RATIO_LOCK
    let lock_bit: u32 = (lo >> 31) & 1;

    let turbo_threshold_ratio = scale_ratio(raw_ratio);
    let turbo_locked: u16     = if lock_bit != 0 { 1000 } else { 0 };
    let turbo_headroom: u16   = (1000u16.saturating_sub(turbo_threshold_ratio)).min(1000);

    let mut s = MODULE.lock();
    s.turbo_threshold_ratio = turbo_threshold_ratio;
    s.turbo_locked          = turbo_locked;
    s.turbo_headroom        = turbo_headroom;
    s.turbo_ema             = ema(s.turbo_ema, turbo_threshold_ratio);

    serial_println!(
        "[msr_ia32_turbo_activation_ratio] threshold={} locked={} headroom={} ema={}",
        s.turbo_threshold_ratio,
        s.turbo_locked,
        s.turbo_headroom,
        s.turbo_ema,
    );
}

/// Scaled turbo engagement threshold (0-1000).
/// Maps MAX_NON_TURBO_RATIO bits[7:0] → 0-1000.
pub fn get_turbo_threshold_ratio() -> u16 {
    MODULE.lock().turbo_threshold_ratio
}

/// 0 = ratio register is writable; 1000 = permanently locked by firmware.
pub fn get_turbo_locked() -> u16 {
    MODULE.lock().turbo_locked
}

/// Headroom above the threshold: (1000 - threshold).min(1000).
/// High values mean ANIMA still has room to soar above the engagement floor.
pub fn get_turbo_headroom() -> u16 {
    MODULE.lock().turbo_headroom
}

/// Exponential moving average of turbo_threshold_ratio (slow-follow).
pub fn get_turbo_ema() -> u16 {
    MODULE.lock().turbo_ema
}
