#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ─── RAPL Unit State ─────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
struct RaplPowerUnitState {
    power_unit:     u16,
    energy_unit:    u16,
    time_unit:      u16,
    rapl_unit_ema:  u16,
}

impl RaplPowerUnitState {
    const fn zero() -> Self {
        Self {
            power_unit:    0,
            energy_unit:   0,
            time_unit:     0,
            rapl_unit_ema: 0,
        }
    }
}

static STATE: Mutex<RaplPowerUnitState> = Mutex::new(RaplPowerUnitState::zero());

// ─── CPUID Guard ─────────────────────────────────────────────────────────────

fn has_rapl() -> bool {
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
    (eax_val >> 4) & 1 != 0
}

// ─── MSR Read ────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR via `rdmsr`. Returns (edx:eax) as u64.
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ─── Signal Helpers ──────────────────────────────────────────────────────────

#[inline]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = (old as u32 * 7 + new_val as u32) / 8;
    result as u16
}

// ─── Parse MSR 0x606 ─────────────────────────────────────────────────────────

fn parse_power_unit_msr(raw: u64) -> (u16, u16, u16) {
    let lo = raw as u32;

    // bits [3:0]   → power_unit
    let pu_raw: u32 = (lo & 0xF) as u32;
    let power_unit = cap1000(pu_raw * 62);

    // bits [12:8]  → energy_unit
    let eu_raw: u32 = ((lo >> 8) & 0x1F) as u32;
    let energy_unit = cap1000(eu_raw * 31);

    // bits [19:16] → time_unit
    let tu_raw: u32 = ((lo >> 16) & 0xF) as u32;
    let time_unit = cap1000(tu_raw * 62);

    (power_unit, energy_unit, time_unit)
}

fn compute_composite(power_unit: u16, energy_unit: u16, time_unit: u16) -> u16 {
    // rapl_unit_ema input = power_unit/4 + energy_unit/4 + time_unit/2
    let composite: u32 = (power_unit as u32 / 4)
        + (energy_unit as u32 / 4)
        + (time_unit as u32 / 2);
    cap1000(composite)
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn init() {
    if !has_rapl() {
        crate::serial_println!(
            "[msr_rapl_power_unit] RAPL not supported (CPUID leaf 6 EAX bit 4 = 0)"
        );
        return;
    }

    let raw = unsafe { rdmsr(0x606) };
    let (power_unit, energy_unit, time_unit) = parse_power_unit_msr(raw);
    let composite = compute_composite(power_unit, energy_unit, time_unit);

    let mut s = STATE.lock();
    s.power_unit    = power_unit;
    s.energy_unit   = energy_unit;
    s.time_unit     = time_unit;
    s.rapl_unit_ema = composite; // seed EMA at first sample

    crate::serial_println!(
        "[msr_rapl_power_unit] init: power_u={} energy_u={} time_u={} ema={}",
        s.power_unit,
        s.energy_unit,
        s.time_unit,
        s.rapl_unit_ema,
    );
}

pub fn tick(age: u32) {
    // Sample every 10000 ticks — RAPL units are static after boot
    if age % 10000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let raw = unsafe { rdmsr(0x606) };
    let (power_unit, energy_unit, time_unit) = parse_power_unit_msr(raw);
    let composite = compute_composite(power_unit, energy_unit, time_unit);

    let mut s = STATE.lock();
    s.power_unit    = power_unit;
    s.energy_unit   = energy_unit;
    s.time_unit     = time_unit;
    s.rapl_unit_ema = ema(s.rapl_unit_ema, composite);

    crate::serial_println!(
        "[msr_rapl_power_unit] age={} power_u={} energy_u={} time_u={} ema={}",
        age,
        s.power_unit,
        s.energy_unit,
        s.time_unit,
        s.rapl_unit_ema,
    );
}

// ─── Getters ─────────────────────────────────────────────────────────────────

pub fn get_power_unit() -> u16 {
    STATE.lock().power_unit
}

pub fn get_energy_unit() -> u16 {
    STATE.lock().energy_unit
}

pub fn get_time_unit() -> u16 {
    STATE.lock().time_unit
}

pub fn get_rapl_unit_ema() -> u16 {
    STATE.lock().rapl_unit_ema
}
