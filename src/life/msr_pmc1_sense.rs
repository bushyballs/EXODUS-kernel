#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pmc1_lo: u16,
    pmc1_delta: u16,
    pmc1_ema: u16,
    pmc1_event_sense: u16,
    last_lo: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pmc1_lo: 0,
    pmc1_delta: 0,
    pmc1_ema: 0,
    pmc1_event_sense: 0,
    last_lo: 0,
});

#[inline]
fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_val,
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 15) & 1 == 1
}

#[inline]
fn rdmsr_pmc1() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0xC2u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

pub fn init() {
    serial_println!("[msr_pmc1_sense] init");
    if !has_pdcm() { return; }
    let lo = rdmsr_pmc1();
    let mut s = MODULE.lock();
    s.last_lo = lo;
}

pub fn tick(age: u32) {
    if age % 300 != 0 { return; }
    if !has_pdcm() { return; }

    let lo = rdmsr_pmc1();
    let mut s = MODULE.lock();

    let raw_lo = lo & 0xFFFF;
    let pmc1_lo = ((raw_lo as u32).wrapping_mul(1000) / 65536).min(1000) as u16;

    let last16 = s.last_lo & 0xFFFF;
    let delta_raw = raw_lo.wrapping_sub(last16) & 0xFFFF;
    let pmc1_delta = ((delta_raw as u32).wrapping_mul(1000) / 65536).min(1000) as u16;

    let pmc1_ema = ((s.pmc1_ema as u32).wrapping_mul(7).saturating_add(pmc1_delta as u32) / 8) as u16;
    let pmc1_event_sense = ((s.pmc1_event_sense as u32).wrapping_mul(7).saturating_add(pmc1_ema as u32) / 8) as u16;

    s.last_lo = lo;
    s.pmc1_lo = pmc1_lo;
    s.pmc1_delta = pmc1_delta;
    s.pmc1_ema = pmc1_ema;
    s.pmc1_event_sense = pmc1_event_sense;

    serial_println!("[msr_pmc1_sense] age={} pmc1_lo={} delta={} ema={} event_sense={}",
        age, pmc1_lo, pmc1_delta, pmc1_ema, pmc1_event_sense);
}
