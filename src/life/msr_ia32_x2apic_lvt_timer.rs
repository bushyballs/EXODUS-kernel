#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_LVT_TIMER: u32 = 0x832;
const TICK_GATE: u32 = 2000;

pub struct State {
    timer_vector: u16,
    timer_masked: u16,
    timer_mode:   u16,
    timer_ema:    u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    timer_vector: 0,
    timer_masked: 0,
    timer_mode:   0,
    timer_ema:    0,
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

// Read IA32_X2APIC_LVT_TIMER MSR 0x832 and return (lo, hi).
fn read_lvt_timer() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_X2APIC_LVT_TIMER,
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

pub fn init() {
    serial_println!("[msr_ia32_x2apic_lvt_timer] init");
    let mut s = MODULE.lock();
    s.timer_vector = 0;
    s.timer_masked = 0;
    s.timer_mode   = 0;
    s.timer_ema    = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !x2apic_supported() {
        serial_println!("[msr_ia32_x2apic_lvt_timer] x2APIC not supported, skipping");
        return;
    }

    let (lo, _hi) = read_lvt_timer();

    // bits[7:0]: Timer interrupt vector — scale 0–255 → 0–1000
    let raw_vector = (lo & 0xFF) as u32;
    let timer_vector = ((raw_vector * 1000) / 255).min(1000) as u16;

    // bit 16: Mask (1=masked/disabled, 0=enabled)
    // Inverted: 0 means silent (masked), 1000 means active (enabled)
    let timer_masked: u16 = if (lo >> 16) & 1 == 0 { 1000 } else { 0 };

    // bits[18:17]: Timer mode (00=one-shot, 01=periodic, 10=TSC-deadline)
    // Scale 0–2 → 0–1000
    let raw_mode = ((lo >> 17) & 0x3) as u32;
    let timer_mode = ((raw_mode * 1000) / 2).min(1000) as u16;

    let mut s = MODULE.lock();
    s.timer_vector = timer_vector;
    s.timer_masked = timer_masked;
    s.timer_mode   = timer_mode;
    s.timer_ema    = ema(s.timer_ema, timer_masked);

    serial_println!(
        "[msr_ia32_x2apic_lvt_timer] age={} vector={} masked={} mode={} ema={}",
        age,
        s.timer_vector,
        s.timer_masked,
        s.timer_mode,
        s.timer_ema,
    );
}

pub fn get_timer_vector() -> u16 {
    MODULE.lock().timer_vector
}

pub fn get_timer_masked() -> u16 {
    MODULE.lock().timer_masked
}

pub fn get_timer_mode() -> u16 {
    MODULE.lock().timer_mode
}

pub fn get_timer_ema() -> u16 {
    MODULE.lock().timer_ema
}
