#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ─── State ───────────────────────────────────────────────────────────────────

struct CpuidThermalPowerState {
    turbo_arat:     u16,
    power_features: u16,
    hwp_hdc:        u16,
    thermal_ema:    u16,
}

static STATE: Mutex<CpuidThermalPowerState> = Mutex::new(CpuidThermalPowerState {
    turbo_arat:     0,
    power_features: 0,
    hwp_hdc:        0,
    thermal_ema:    0,
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Count set bits in `count` bits of `v` starting at `shift`.
fn popcount_range(v: u32, shift: u32, count: u32) -> u32 {
    let mut bits = (v >> shift) & ((1u32 << count) - 1);
    let mut c = 0u32;
    while bits != 0 {
        c += bits & 1;
        bits >>= 1;
    }
    c
}

/// Read CPUID leaf 0x06; returns (eax, ecx).
fn read_leaf6() -> (u32, u32) {
    let eax6: u32;
    let ecx6: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax6,
            lateout("ecx") ecx6,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax6, ecx6)
}

/// Compute all four signals from a (eax6, ecx6) reading.
fn compute_signals(eax6: u32, _ecx6: u32, old_ema: u16) -> (u16, u16, u16, u16) {
    // ── turbo_arat: bits 1 and 2 of EAX ──────────────────────────────────
    let turbo_bit = (eax6 >> 1) & 1;   // Turbo Boost
    let arat_bit  = (eax6 >> 2) & 1;   // ARAT
    let turbo_arat = ((turbo_bit * 500) + (arat_bit * 500)) as u16; // 0 | 500 | 1000

    // ── power_features: popcount of bits [10:1] (10 bits) × 100 ──────────
    // bits 1..=10 inclusive → shift=1, count=10
    let pop = popcount_range(eax6, 1, 10); // 0..=10
    let pf_raw = pop * 100;               // 0..=1000
    let power_features = if pf_raw > 1000 { 1000u16 } else { pf_raw as u16 };

    // ── hwp_hdc: bit 7 (HWP) and bit 10 (HDC) ────────────────────────────
    let hwp_bit = (eax6 >> 7)  & 1;
    let hdc_bit = (eax6 >> 10) & 1;
    let hwp_hdc = ((hwp_bit * 500) + (hdc_bit * 500)) as u16;

    // ── thermal_ema: EMA of power_features ───────────────────────────────
    let ema_u32 = ((old_ema as u32) * 7 + (power_features as u32)) / 8;
    let thermal_ema = ema_u32 as u16;

    (turbo_arat, power_features, hwp_hdc, thermal_ema)
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Initialize the module: perform a single CPUID leaf 0x06 read and seed state.
pub fn init() {
    let (eax6, ecx6) = read_leaf6();
    let (turbo_arat, power_features, hwp_hdc, thermal_ema) =
        compute_signals(eax6, ecx6, 0);

    let mut s = STATE.lock();
    s.turbo_arat     = turbo_arat;
    s.power_features = power_features;
    s.hwp_hdc        = hwp_hdc;
    s.thermal_ema    = thermal_ema;

    crate::serial_println!(
        "[cpuid_thermal_power_leaf] init turbo_arat={} features={} hwp_hdc={} ema={}",
        turbo_arat, power_features, hwp_hdc, thermal_ema
    );
}

/// Tick: sample every 9000 ticks.
pub fn tick(age: u32) {
    if age % 9000 != 0 {
        return;
    }

    let (eax6, ecx6) = read_leaf6();

    let old_ema = {
        let s = STATE.lock();
        s.thermal_ema
    };

    let (turbo_arat, power_features, hwp_hdc, thermal_ema) =
        compute_signals(eax6, ecx6, old_ema);

    {
        let mut s = STATE.lock();
        s.turbo_arat     = turbo_arat;
        s.power_features = power_features;
        s.hwp_hdc        = hwp_hdc;
        s.thermal_ema    = thermal_ema;
    }

    crate::serial_println!(
        "[cpuid_thermal_power_leaf] age={} turbo_arat={} features={} hwp_hdc={} ema={}",
        age, turbo_arat, power_features, hwp_hdc, thermal_ema
    );
}

// ─── Getters ─────────────────────────────────────────────────────────────────

pub fn get_turbo_arat() -> u16 {
    STATE.lock().turbo_arat
}

pub fn get_power_features() -> u16 {
    STATE.lock().power_features
}

pub fn get_hwp_hdc() -> u16 {
    STATE.lock().hwp_hdc
}

pub fn get_thermal_ema() -> u16 {
    STATE.lock().thermal_ema
}
