#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_LASTBRANCH_0_FROM_IP: u32 = 0x680;
const SAMPLE_RATE: u32 = 500;

pub struct State {
    lbr_from_lo:  u16,
    lbr_from_hi:  u16,
    lbr_mispred:  u16,
    lbr_from_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    lbr_from_lo:  0,
    lbr_from_hi:  0,
    lbr_mispred:  0,
    lbr_from_ema: 0,
});

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem)
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
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

fn scale_16(val: u32) -> u16 {
    ((val * 1000) / 65535).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.lbr_from_lo  = 0;
    s.lbr_from_hi  = 0;
    s.lbr_mispred  = 0;
    s.lbr_from_ema = 0;
    serial_println!("[msr_ia32_lastbranchfromip] init");
}

pub fn tick(age: u32) {
    if age % SAMPLE_RATE != 0 {
        return;
    }
    if !has_pdcm() {
        return;
    }

    let (lo, hi) = read_msr(MSR_LASTBRANCH_0_FROM_IP);

    let from_lo_raw  = lo & 0xFFFF;
    let from_hi_raw  = hi & 0xFFFF;
    let mispred_bit  = (hi >> 31) & 1;

    let lbr_from_lo  = scale_16(from_lo_raw);
    let lbr_from_hi  = scale_16(from_hi_raw);
    let lbr_mispred  = if mispred_bit != 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();
    s.lbr_from_lo  = lbr_from_lo;
    s.lbr_from_hi  = lbr_from_hi;
    s.lbr_mispred  = lbr_mispred;
    s.lbr_from_ema = ema(s.lbr_from_ema, lbr_from_lo);

    serial_println!(
        "[msr_ia32_lastbranchfromip] age={} lo={} hi={} mispred={} ema={}",
        age,
        s.lbr_from_lo,
        s.lbr_from_hi,
        s.lbr_mispred,
        s.lbr_from_ema
    );
}

pub fn get_lbr_from_lo() -> u16 {
    MODULE.lock().lbr_from_lo
}

pub fn get_lbr_from_hi() -> u16 {
    MODULE.lock().lbr_from_hi
}

pub fn get_lbr_mispred() -> u16 {
    MODULE.lock().lbr_mispred
}

pub fn get_lbr_from_ema() -> u16 {
    MODULE.lock().lbr_from_ema
}
