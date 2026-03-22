#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_LASTBRANCH_0_TO_IP: u32 = 0x6C0;
const SAMPLE_GATE: u32 = 550;

pub struct State {
    lbr_to_lo:     u16,
    lbr_to_hi:     u16,
    lbr_to_kernel: u16,
    lbr_to_ema:    u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    lbr_to_lo:     0,
    lbr_to_hi:     0,
    lbr_to_kernel: 0,
    lbr_to_ema:    0,
});

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 15) & 1 == 1
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
    ((val * 1000) / 65535).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut state = MODULE.lock();
    state.lbr_to_lo     = 0;
    state.lbr_to_hi     = 0;
    state.lbr_to_kernel = 0;
    state.lbr_to_ema    = 0;
    serial_println!("[msr_ia32_lastbranchtoip] init complete");
}

pub fn tick(age: u32) {
    if age % SAMPLE_GATE != 0 {
        return;
    }
    if !has_pdcm() {
        return;
    }

    let (lo, hi) = read_msr(MSR_LASTBRANCH_0_TO_IP);

    let lo_bits = lo & 0xFFFF;
    let hi_bits = hi & 0xFFFF;
    let hi_top  = (hi >> 16) & 0xFFFF;

    let sig_lo     = scale_u16(lo_bits);
    let sig_hi     = scale_u16(hi_bits);
    let sig_kernel = if hi_top == 0xFFFF { 1000u16 } else { 0u16 };

    let mut state = MODULE.lock();

    let new_ema = ema(state.lbr_to_ema, sig_lo);

    state.lbr_to_lo     = sig_lo;
    state.lbr_to_hi     = sig_hi;
    state.lbr_to_kernel = sig_kernel;
    state.lbr_to_ema    = new_ema;

    serial_println!(
        "[msr_ia32_lastbranchtoip] tick={} lo={} hi={} kernel={} ema={}",
        age, sig_lo, sig_hi, sig_kernel, new_ema
    );
}

pub fn get_lbr_to_lo() -> u16 {
    MODULE.lock().lbr_to_lo
}

pub fn get_lbr_to_hi() -> u16 {
    MODULE.lock().lbr_to_hi
}

pub fn get_lbr_to_kernel() -> u16 {
    MODULE.lock().lbr_to_kernel
}

pub fn get_lbr_to_ema() -> u16 {
    MODULE.lock().lbr_to_ema
}
