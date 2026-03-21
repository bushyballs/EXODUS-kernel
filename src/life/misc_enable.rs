#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MiscEnableState {
    pub feature_richness: u16,  // 0=minimal, 1000=fully featured
    pub capability_loss: u16,   // 0=all capable, 1000=key features unavailable
    pub speedstep_active: u16,  // 0 or 1000
    pub thermal_guarded: u16,   // 0 or 1000
    tick_count: u32,
}

pub static MODULE: Mutex<MiscEnableState> = Mutex::new(MiscEnableState {
    feature_richness: 0,
    capability_loss: 0,
    speedstep_active: 0,
    thermal_guarded: 0,
    tick_count: 0,
});

const MSR_IA32_MISC_ENABLE: u32 = 0x1A0;

// Capability bits (presence = good)
const BIT_FAST_STRINGS:   u64 = 1 << 0;
const BIT_THERMAL_CTRL:   u64 = 1 << 3;
const BIT_PERF_MON:       u64 = 1 << 7;
const BIT_SPEEDSTEP:      u64 = 1 << 16;
const BIT_MONITOR_FSM:    u64 = 1 << 18;
const BIT_ACPI_ENABLE:    u64 = 1 << 34;

// Unavailability bits (presence = bad)
const BIT_BTS_UNAVAIL:    u64 = 1 << 11;
const BIT_PEBS_UNAVAIL:   u64 = 1 << 12;

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

fn sample_msr() -> u64 {
    unsafe { rdmsr(MSR_IA32_MISC_ENABLE) }
}

fn compute_feature_richness(msr: u64) -> u16 {
    let mut count: u16 = 0;
    if msr & BIT_FAST_STRINGS  != 0 { count = count.saturating_add(1); }
    if msr & BIT_THERMAL_CTRL  != 0 { count = count.saturating_add(1); }
    if msr & BIT_PERF_MON      != 0 { count = count.saturating_add(1); }
    if msr & BIT_SPEEDSTEP     != 0 { count = count.saturating_add(1); }
    if msr & BIT_MONITOR_FSM   != 0 { count = count.saturating_add(1); }
    if msr & BIT_ACPI_ENABLE   != 0 { count = count.saturating_add(1); }
    // Each of 6 bits contributes 166; max 996 ≈ 1000
    count.wrapping_mul(166).min(1000)
}

fn compute_capability_loss(msr: u64) -> u16 {
    let mut loss: u16 = 0;
    if msr & BIT_BTS_UNAVAIL  != 0 { loss = loss.saturating_add(500); }
    if msr & BIT_PEBS_UNAVAIL != 0 { loss = loss.saturating_add(500); }
    loss.min(1000)
}

fn ema(old: u16, signal: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(signal as u32) / 8) as u16
}

pub fn init() {
    let msr = sample_msr();
    let richness = compute_feature_richness(msr);
    let loss     = compute_capability_loss(msr);
    let speedstep = if msr & BIT_SPEEDSTEP    != 0 { 1000u16 } else { 0 };
    let thermal   = if msr & BIT_THERMAL_CTRL != 0 { 1000u16 } else { 0 };

    let mut s = MODULE.lock();
    s.feature_richness = richness;
    s.capability_loss  = loss;
    s.speedstep_active = speedstep;
    s.thermal_guarded  = thermal;
    s.tick_count       = 0;

    serial_println!(
        "[misc_enable] init — feature_richness={} capability_loss={} speedstep={} thermal={}",
        richness, loss, speedstep, thermal
    );
}

pub fn tick(age: u32) {
    // IA32_MISC_ENABLE rarely changes; gate to every 64 ticks
    if age % 64 != 0 {
        return;
    }

    let msr = sample_msr();

    let raw_richness = compute_feature_richness(msr);
    let raw_loss     = compute_capability_loss(msr);
    let speedstep    = if msr & BIT_SPEEDSTEP    != 0 { 1000u16 } else { 0 };
    let thermal      = if msr & BIT_THERMAL_CTRL != 0 { 1000u16 } else { 0 };

    let mut s = MODULE.lock();
    s.feature_richness = ema(s.feature_richness, raw_richness);
    s.capability_loss  = ema(s.capability_loss,  raw_loss);
    s.speedstep_active = speedstep;
    s.thermal_guarded  = thermal;
    s.tick_count       = s.tick_count.wrapping_add(1);

    serial_println!(
        "[misc_enable] tick={} richness={} loss={} speedstep={} thermal={}",
        age, s.feature_richness, s.capability_loss, s.speedstep_active, s.thermal_guarded
    );
}
