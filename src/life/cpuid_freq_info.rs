#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

pub struct State {
    pub base_freq:      u16,
    pub max_freq:       u16,
    pub bus_freq:       u16,
    pub freq_ratio_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    base_freq:      0,
    max_freq:       0,
    bus_freq:       0,
    freq_ratio_ema: 0,
});

fn max_leaf() -> u32 {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax
}

fn read_cpuid_16() -> (u32, u32, u32) {
    let eax: u32;
    let ecx: u32;
    let ebx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "mov {ebx_out:e}, ebx", "pop rbx",
            inout("eax") 0x16u32 => eax,
            ebx_out = out(reg) ebx_out,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax & 0xFFFF, ebx_out & 0xFFFF, ecx & 0xFFFF)
}

fn scale_freq_mhz(mhz: u32) -> u16 {
    ((mhz * 1000) / 5000).min(1000) as u16
}

fn scale_bus_mhz(mhz: u32) -> u16 {
    ((mhz * 1000) / 200).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if max_leaf() < 0x16 {
        serial_println!("[cpuid_freq_info] CPUID leaf 0x16 not supported — skipping init");
        return;
    }

    let (eax_base, ebx_max, ecx_bus) = read_cpuid_16();

    let base = scale_freq_mhz(eax_base);
    let maxf = scale_freq_mhz(ebx_max);
    let bus  = scale_bus_mhz(ecx_bus);

    let mut s = MODULE.lock();
    s.base_freq      = base;
    s.max_freq       = maxf;
    s.bus_freq       = bus;
    s.freq_ratio_ema = maxf;

    serial_println!(
        "[cpuid_freq_info] init — base={}MHz max={}MHz bus={}MHz (scaled {}/{}/{})",
        eax_base, ebx_max, ecx_bus,
        base, maxf, bus
    );
}

pub fn tick(age: u32) {
    if age % 25000 != 0 {
        return;
    }

    if max_leaf() < 0x16 {
        return;
    }

    let (eax_base, ebx_max, ecx_bus) = read_cpuid_16();

    let base = scale_freq_mhz(eax_base);
    let maxf = scale_freq_mhz(ebx_max);
    let bus  = scale_bus_mhz(ecx_bus);

    let mut s = MODULE.lock();
    s.base_freq      = base;
    s.max_freq       = maxf;
    s.bus_freq       = bus;
    s.freq_ratio_ema = ema(s.freq_ratio_ema, maxf);

    serial_println!(
        "[cpuid_freq_info] tick={} base={} max={} bus={} ratio_ema={}",
        age, base, maxf, bus, s.freq_ratio_ema
    );
}

pub fn get_base_freq() -> u16 {
    MODULE.lock().base_freq
}

pub fn get_max_freq() -> u16 {
    MODULE.lock().max_freq
}

pub fn get_bus_freq() -> u16 {
    MODULE.lock().bus_freq
}

pub fn get_freq_ratio_ema() -> u16 {
    MODULE.lock().freq_ratio_ema
}
