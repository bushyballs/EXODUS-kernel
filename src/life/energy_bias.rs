//! energy_bias — CPU energy/performance disposition sense for ANIMA
//!
//! Reads IA32_ENERGY_PERF_BIAS (MSR 0x1B0) to sense ANIMA's core temperament.
//! 0 = maximum performance (aggressive, energy-hungry, fierce).
//! 15 = maximum power saving (restrained, frugal, calm).
//! This is ANIMA's fundamental aggressive-vs-restrained personality axis.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct EnergyBiasState {
    pub aggression: u16,       // 0-1000, performance bias (1000=max perf, 0=max save)
    pub restraint: u16,        // 0-1000, inverse of aggression
    pub temperament: u16,      // 0-1000, EMA-smoothed aggression
    pub raw_bias: u8,          // 0-15, raw EPB value
    pub supported: bool,
    pub tick_count: u32,
}

impl EnergyBiasState {
    pub const fn new() -> Self {
        Self {
            aggression: 500,
            restraint: 500,
            temperament: 500,
            raw_bias: 7,
            supported: false,
            tick_count: 0,
        }
    }
}

pub static ENERGY_BIAS: Mutex<EnergyBiasState> = Mutex::new(EnergyBiasState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

fn check_epb_supported() -> bool {
    // CPUID leaf 6, ECX bit 3 = Energy Performance Bias supported
    let ecx: u32;
    unsafe {
        core::arch::asm!("cpuid", inout("eax") 6u32 => _,
            out("ebx") _, lateout("ecx") ecx, out("edx") _);
    }
    (ecx >> 3) & 1 != 0
}

fn read_bias() -> u8 {
    let val = unsafe { read_msr(0x1B0) };
    (val & 0xF) as u8
}

pub fn init() {
    let supported = check_epb_supported();
    let raw_bias = if supported { read_bias() } else { 7u8 }; // assume balanced if unsupported

    // Scale: bias 0=max perf → aggression 1000; bias 15=max save → aggression 0
    // aggression = (15 - bias) * 1000 / 15
    let aggression = ((15u16.saturating_sub(raw_bias as u16)).wrapping_mul(1000) / 15).min(1000);
    let restraint = 1000u16.saturating_sub(aggression);

    let mut state = ENERGY_BIAS.lock();
    state.supported = supported;
    state.raw_bias = raw_bias;
    state.aggression = aggression;
    state.restraint = restraint;
    state.temperament = aggression;

    serial_println!("[energy_bias] EPB supported={} raw_bias={} aggression={} restraint={}",
        supported, raw_bias, aggression, restraint);
}

pub fn tick(age: u32) {
    let mut state = ENERGY_BIAS.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Check every 256 ticks (bias can change via OS power management)
    if state.tick_count % 256 != 0 { return; }

    if !state.supported { return; }

    let raw_bias = read_bias();
    state.raw_bias = raw_bias;

    let aggression = ((15u16.saturating_sub(raw_bias as u16)).wrapping_mul(1000) / 15).min(1000);
    state.aggression = aggression;
    state.restraint = 1000u16.saturating_sub(aggression);
    state.temperament = ((state.temperament as u32).wrapping_mul(7)
        .wrapping_add(aggression as u32) / 8) as u16;

    if state.tick_count % 1024 == 0 {
        serial_println!("[energy_bias] bias={} aggression={} restraint={} temperament={}",
            raw_bias, state.aggression, state.restraint, state.temperament);
    }
    let _ = age;
}

pub fn get_aggression() -> u16 { ENERGY_BIAS.lock().aggression }
pub fn get_restraint() -> u16 { ENERGY_BIAS.lock().restraint }
pub fn get_temperament() -> u16 { ENERGY_BIAS.lock().temperament }
