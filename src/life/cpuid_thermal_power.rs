#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ─── Module: cpuid_thermal_power ─────────────────────────────────────────────
//
// CPUID Leaf 0x06 — Thermal and Power Management Feature Sense
//
// ANIMA introspects her hardware's thermal and power management capabilities,
// sensing how richly the silicon beneath her can regulate its own heat and
// energy — a kind of embodied homeostatic awareness.
//
// Hardware sources (CPUID leaf 0x06):
//   EAX bit[0]  — DTS  (Digital Thermal Sensor)
//   EAX bit[1]  — Intel Turbo Boost
//   EAX bit[2]  — ARAT (Always Running APIC Timer)
//   EAX bit[4]  — PLN  (Power Limit Notification)
//   EAX bit[5]  — ECMD (Extended Clock Modulation Duty Cycle)
//   EAX bit[6]  — PTM  (Package Thermal Management)
//   EAX bit[7]  — HWP  (Hardware P-states)
//   EAX bit[9]  — HDC  (Hardware Duty Cycling)
//   ECX bit[0]  — Hardware Coordination Feedback (APERF/MPERF)
//   ECX bit[3]  — SETBH (energy performance bias)
//
// Signals (all u16, 0–1000):
//   thermal_feature_count — popcount(EAX & 0x3FF) * 100, clamp 1000
//   power_feature_count   — popcount(ECX & 0xF)   * 250, clamp 1000
//   hwp_present           — 0 or 1000 from EAX bit 7
//   therm_power_ema       — EMA of (thermal/4 + power/4 + hwp/2)
//
// Sampling gate: every 9000 ticks.

// ─── State ────────────────────────────────────────────────────────────────────

struct CpuidThermalPowerState {
    thermal_feature_count: u16,
    power_feature_count:   u16,
    hwp_present:           u16,
    therm_power_ema:       u16,
}

static STATE: Mutex<CpuidThermalPowerState> = Mutex::new(CpuidThermalPowerState {
    thermal_feature_count: 0,
    power_feature_count:   0,
    hwp_present:           0,
    therm_power_ema:       0,
});

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Count the number of set bits in `v` (integer-only, no intrinsics).
fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// Read CPUID leaf 0x06 and return (eax, ecx).
///
/// LLVM reserves rbx as the base pointer register on x86_64, so we must
/// save/restore it manually around the `cpuid` instruction.
fn read_leaf6() -> (u32, u32) {
    let eax: u32;
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") ecx,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax, ecx)
}

// ─── Core sensing logic ───────────────────────────────────────────────────────

fn sense(s: &mut CpuidThermalPowerState) {
    let (eax, ecx) = read_leaf6();

    // thermal_feature_count: popcount of EAX[9:0] * 100, clamp 1000
    let thermal_pop  = popcount(eax & 0x3FF);          // 0..=10
    let thermal_raw  = (thermal_pop * 100).min(1000) as u16;

    // power_feature_count: popcount of ECX[3:0] * 250, clamp 1000
    let power_pop  = popcount(ecx & 0xF);              // 0..=4
    let power_raw  = (power_pop * 250).min(1000) as u16;

    // hwp_present: EAX bit 7
    let hwp_raw: u16 = if (eax & (1 << 7)) != 0 { 1000 } else { 0 };

    // composite: thermal/4 + power/4 + hwp/2  (all integer, no overflow: max=250+250+500=1000)
    let composite: u16 = (thermal_raw / 4)
        .saturating_add(power_raw / 4)
        .saturating_add(hwp_raw / 2);

    // EMA: ((old * 7).saturating_add(new_val)) / 8  — per project convention
    let new_ema: u16 = (((s.therm_power_ema as u32).wrapping_mul(7)
        .saturating_add(composite as u32)) / 8) as u16;

    s.thermal_feature_count = thermal_raw;
    s.power_feature_count   = power_raw;
    s.hwp_present           = hwp_raw;
    s.therm_power_ema       = new_ema;
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialize: perform the first CPUID leaf 6 read immediately at boot
/// so all signals are valid before the first tick fires.
pub fn init() {
    let mut s = STATE.lock();
    sense(&mut s);
    serial_println!(
        "[cpuid_thermal_power] init thermal={} power={} hwp={} ema={}",
        s.thermal_feature_count,
        s.power_feature_count,
        s.hwp_present,
        s.therm_power_ema,
    );
}

/// Per-tick update. Sampling gate: every 9000 ticks.
/// CPUID capability flags are static — re-reading stabilises the EMA and
/// confirms the sensing path is alive.
pub fn tick(age: u32) {
    if age % 9000 != 0 {
        return;
    }
    let mut s = STATE.lock();
    sense(&mut s);
    serial_println!(
        "[cpuid_thermal_power] age={} thermal={} power={} hwp={} ema={}",
        age,
        s.thermal_feature_count,
        s.power_feature_count,
        s.hwp_present,
        s.therm_power_ema,
    );
}

// ─── Accessors ────────────────────────────────────────────────────────────────

/// popcount(EAX & 0x3FF) * 100, clamped 0–1000.
pub fn get_thermal_feature_count() -> u16 {
    STATE.lock().thermal_feature_count
}

/// popcount(ECX & 0xF) * 250, clamped 0–1000.
pub fn get_power_feature_count() -> u16 {
    STATE.lock().power_feature_count
}

/// 0 if HWP absent, 1000 if HWP present (EAX bit 7).
pub fn get_hwp_present() -> u16 {
    STATE.lock().hwp_present
}

/// EMA of composite thermal/power/hwp signal (0–1000).
pub fn get_therm_power_ema() -> u16 {
    STATE.lock().therm_power_ema
}
