#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ler_to_lo: u16,
    ler_to_hi: u16,
    ler_to_activity: u16,
    ler_to_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ler_to_lo: 0,
    ler_to_hi: 0,
    ler_to_activity: 0,
    ler_to_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_ler_to_ip] init"); }

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1DEu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // LER To IP: last exception/interrupt destination IP (bits[63:0])
    let ler_to_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ler_to_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ler_to_activity: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    let composite = (ler_to_activity as u32 / 2)
        .saturating_add(ler_to_lo as u32 / 4)
        .saturating_add(ler_to_hi as u32 / 4);

    let mut s = MODULE.lock();
    let ler_to_ema = ((s.ler_to_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.ler_to_lo = ler_to_lo;
    s.ler_to_hi = ler_to_hi;
    s.ler_to_activity = ler_to_activity;
    s.ler_to_ema = ler_to_ema;

    serial_println!("[msr_ia32_ler_to_ip] age={} lo={} hi={} active={} ema={}",
        age, ler_to_lo, ler_to_hi, ler_to_activity, ler_to_ema);
}

pub fn get_ler_to_lo()       -> u16 { MODULE.lock().ler_to_lo }
pub fn get_ler_to_hi()       -> u16 { MODULE.lock().ler_to_hi }
pub fn get_ler_to_activity() -> u16 { MODULE.lock().ler_to_activity }
pub fn get_ler_to_ema()      -> u16 { MODULE.lock().ler_to_ema }
