#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_CUR_COUNT: u32 = 0x839;

pub struct State {
    apic_count_lo:  u16,
    apic_count_hi16: u16,
    apic_counting:  u16,
    apic_count_ema: u16,
    last_lo:        u32,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    apic_count_lo:  0,
    apic_count_hi16: 0,
    apic_counting:  0,
    apic_count_ema: 0,
    last_lo:        0,
});

fn x2apic_supported() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {ecx}, ecx",
            "pop rbx",
            ecx = out(reg) ecx,
            out("eax") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 21) & 1 == 1
}

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

fn scale_u16(val: u32) -> u16 {
    ((val as u64 * 1000 / 65535) as u32).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.apic_count_lo  = 0;
    s.apic_count_hi16 = 0;
    s.apic_counting  = 0;
    s.apic_count_ema = 0;
    s.last_lo        = 0;
    serial_println!("[msr_ia32_x2apic_cur_count] init");
}

pub fn tick(age: u32) {
    if age % 500 != 0 {
        return;
    }

    if !x2apic_supported() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_X2APIC_CUR_COUNT);

    let raw_lo  = lo & 0xFFFF;
    let raw_hi  = (lo >> 16) & 0xFFFF;

    let sig_lo   = scale_u16(raw_lo);
    let sig_hi16 = scale_u16(raw_hi);
    let sig_counting: u16 = if lo != 0 { 1000 } else { 0 };

    let mut s = MODULE.lock();

    let new_ema = ema(s.apic_count_ema, sig_lo);

    s.last_lo        = lo;
    s.apic_count_lo  = sig_lo;
    s.apic_count_hi16 = sig_hi16;
    s.apic_counting  = sig_counting;
    s.apic_count_ema = new_ema;

    serial_println!(
        "[msr_ia32_x2apic_cur_count] age={} lo={} hi16={} counting={} ema={}",
        age, sig_lo, sig_hi16, sig_counting, new_ema
    );
}

pub fn get_apic_count_lo() -> u16 {
    MODULE.lock().apic_count_lo
}

pub fn get_apic_count_hi16() -> u16 {
    MODULE.lock().apic_count_hi16
}

pub fn get_apic_counting() -> u16 {
    MODULE.lock().apic_counting
}

pub fn get_apic_count_ema() -> u16 {
    MODULE.lock().apic_count_ema
}
