#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_DIV_CONF: u32 = 0x83E;
const TICK_GATE: u32 = 5000;

pub struct State {
    div_conf_raw: u16,
    div_speed:    u16,
    div_fast:     u16,
    div_ema:      u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    div_conf_raw: 0,
    div_speed:    0,
    div_fast:     0,
    div_ema:      0,
});

// Returns true if the CPU supports x2APIC (CPUID leaf 1, ECX bit 21).
fn x2apic_supported() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            options(nostack, nomem),
        );
    }
    (ecx >> 21) & 1 == 1
}

// Read IA32_X2APIC_DIV_CONF MSR 0x83E and return (lo, hi).
fn read_div_conf() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_X2APIC_DIV_CONF,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// Decode bits[3:0] of DIV_CONF into a speed signal (0-1000).
// Higher value = less division = faster counting.
// Encoding: 000=÷2, 001=÷4, 010=÷8, 011=÷16, 100=÷32, 101=÷64, 110=÷128, 111=÷1, 1000=÷1
fn decode_div_speed(nibble: u32) -> (u16, u32) {
    // Returns (speed_signal, actual_divisor)
    match nibble & 0xF {
        0b0000 => (500, 2),
        0b0001 => (250, 4),
        0b0010 => (125, 8),
        0b0011 => (62, 16),
        0b0100 => (31, 32),
        0b0101 => (15, 64),
        0b0110 => (7, 128),
        0b0111 => (1000, 1),
        0b1000 => (1000, 1),
        // Undefined encodings treated as slowest (÷128)
        _      => (7, 128),
    }
}

pub fn init() {
    serial_println!("[msr_ia32_x2apic_div_conf] init");
    let mut s = MODULE.lock();
    s.div_conf_raw = 0;
    s.div_speed    = 0;
    s.div_fast     = 0;
    s.div_ema      = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !x2apic_supported() {
        serial_println!("[msr_ia32_x2apic_div_conf] x2APIC not supported, skipping");
        return;
    }

    let (lo, _hi) = read_div_conf();

    // bits[3:0]: raw divide configuration field
    let nibble = lo & 0xF;

    // div_conf_raw: nibble scaled 0–15 → 0–1000
    let div_conf_raw = ((nibble * 1000) / 15).min(1000) as u16;

    // div_speed: decoded speed signal; also returns the actual divisor
    let (div_speed, divisor) = decode_div_speed(nibble);

    // div_fast: 1000 if divisor <= 4, else 0
    let div_fast: u16 = if divisor <= 4 { 1000 } else { 0 };

    let mut s = MODULE.lock();
    s.div_conf_raw = div_conf_raw;
    s.div_speed    = div_speed;
    s.div_fast     = div_fast;
    s.div_ema      = ema(s.div_ema, div_speed);

    serial_println!(
        "[msr_ia32_x2apic_div_conf] age={} raw={} speed={} fast={} ema={}",
        age,
        s.div_conf_raw,
        s.div_speed,
        s.div_fast,
        s.div_ema,
    );
}

pub fn get_div_conf_raw() -> u16 {
    MODULE.lock().div_conf_raw
}

pub fn get_div_speed() -> u16 {
    MODULE.lock().div_speed
}

pub fn get_div_fast() -> u16 {
    MODULE.lock().div_fast
}

pub fn get_div_ema() -> u16 {
    MODULE.lock().div_ema
}
