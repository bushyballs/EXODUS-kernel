#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ler_from_lo: u16,
    ler_from_hi: u16,
    ler_branch_activity: u16,
    ler_from_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ler_from_lo: 0,
    ler_from_hi: 0,
    ler_branch_activity: 0,
    ler_from_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_ler_from_ip] init"); }

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1DDu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // LER From IP: last exception/interrupt return source IP
    let ler_from_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ler_from_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ler_branch_activity: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    let composite = (ler_branch_activity as u32 / 2)
        .saturating_add(ler_from_lo as u32 / 4)
        .saturating_add(ler_from_hi as u32 / 4);

    let mut s = MODULE.lock();
    let ler_from_ema = ((s.ler_from_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.ler_from_lo = ler_from_lo;
    s.ler_from_hi = ler_from_hi;
    s.ler_branch_activity = ler_branch_activity;
    s.ler_from_ema = ler_from_ema;

    serial_println!("[msr_ia32_ler_from_ip] age={} lo={} hi={} active={} ema={}",
        age, ler_from_lo, ler_from_hi, ler_branch_activity, ler_from_ema);
}

pub fn get_ler_from_lo()         -> u16 { MODULE.lock().ler_from_lo }
pub fn get_ler_from_hi()         -> u16 { MODULE.lock().ler_from_hi }
pub fn get_ler_branch_activity() -> u16 { MODULE.lock().ler_branch_activity }
pub fn get_ler_from_ema()        -> u16 { MODULE.lock().ler_from_ema }
