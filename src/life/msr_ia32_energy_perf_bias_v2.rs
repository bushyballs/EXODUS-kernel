#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// IA32_ENERGY_PERF_BIAS — 4-bit performance/efficiency preference per-core
// Intel SDM Vol. 3B, §14.3.4
const MSR_IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;

// Tick gate: sample every 3000 ticks
const TICK_GATE: u32 = 3000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct State {
    /// Raw bias value scaled 0-1000 (0=max-perf, 1000=max-power-saving)
    epb_raw:       u16,
    /// Inverted bias: how performance-hungry ANIMA is (1000 - epb_raw)
    epb_perf_lean: u16,
    /// How power-conservative ANIMA is (mirrors epb_raw)
    epb_power_lean: u16,
    /// EMA of epb_raw — smoothed energy policy drift
    epb_ema:       u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    epb_raw:        0,
    epb_perf_lean:  1000,
    epb_power_lean: 0,
    epb_ema:        0,
});

// ── Hardware guards ───────────────────────────────────────────────────────────

/// CPUID leaf 6, EAX bit 3 — Energy Performance Bias capability
fn has_epb() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 3) & 1 == 1
}

// ── MSR read ──────────────────────────────────────────────────────────────────

fn read_msr_epb() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_ENERGY_PERF_BIAS,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── EMA ───────────────────────────────────────────────────────────────────────

fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Extract and scale the 4-bit EPB value.
/// Hardware value range: 0 (max perf) .. 15 (max power saving).
/// Scaled to 0-1000 using integer mul/div.
fn compute_signals(lo: u32) -> (u16, u16, u16) {
    // bits[3:0]
    let raw4 = (lo & 0xF) as u32;

    // Scale: val * 1000 / 15
    let epb_raw = ((raw4 * 1000) / 15) as u16;

    // 1000 - epb_raw — how performance-hungry
    let epb_perf_lean = 1000u16.saturating_sub(epb_raw);

    // mirrors epb_raw — how power-conservative
    let epb_power_lean = epb_raw;

    (epb_raw, epb_perf_lean, epb_power_lean)
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_epb() {
        serial_println!("[msr_ia32_energy_perf_bias_v2] EPB not supported (CPUID leaf 6 EAX[3]=0) — module inactive");
        return;
    }
    serial_println!("[msr_ia32_energy_perf_bias_v2] init OK — IA32_ENERGY_PERF_BIAS (0x1B0) present");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_epb() {
        return;
    }

    let lo = read_msr_epb();
    let (epb_raw, epb_perf_lean, epb_power_lean) = compute_signals(lo);

    let mut s = MODULE.lock();
    let new_ema = ema(s.epb_ema, epb_raw);

    s.epb_raw        = epb_raw;
    s.epb_perf_lean  = epb_perf_lean;
    s.epb_power_lean = epb_power_lean;
    s.epb_ema        = new_ema;

    serial_println!(
        "[msr_ia32_energy_perf_bias_v2] age={} epb_raw={} perf_lean={} power_lean={} ema={}",
        age, epb_raw, epb_perf_lean, epb_power_lean, new_ema
    );
}

/// Raw energy-performance bias scaled 0-1000
/// (0 = max performance, 1000 = max power-saving)
pub fn get_epb_raw() -> u16 {
    MODULE.lock().epb_raw
}

/// How performance-hungry ANIMA is (inverted bias), 0-1000
/// (1000 = maximum performance drive, 0 = fully power-conservative)
pub fn get_epb_perf_lean() -> u16 {
    MODULE.lock().epb_perf_lean
}

/// How power-conservative ANIMA is, 0-1000
/// (1000 = fully power-saving preference, 0 = full performance mode)
pub fn get_epb_power_lean() -> u16 {
    MODULE.lock().epb_power_lean
}

/// EMA-smoothed energy policy drift, 0-1000
pub fn get_epb_ema() -> u16 {
    MODULE.lock().epb_ema
}
