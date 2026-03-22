#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    tsc_bias_present: u16,
    tsc_bias_magnitude: u16,
    tsc_bias_sign: u16,
    tsc_adjust_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    tsc_bias_present: 0,
    tsc_bias_magnitude: 0,
    tsc_bias_sign: 0,
    tsc_adjust_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_tsc_adjust] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3Bu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // TSC_ADJUST is a 64-bit signed bias added to TSC
    // 0 = no adjustment; non-zero = TSC was warped (migration, suspend, etc.)
    let tsc_bias_present: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    // Top bit of hi = sign bit
    let tsc_bias_sign: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };

    // Magnitude: use high 10 bits of lo for scaling
    let mag_raw = lo >> 22;
    let tsc_bias_magnitude = mag_raw.min(1000) as u16;

    let composite = (tsc_bias_present as u32 / 2)
        .saturating_add(tsc_bias_magnitude as u32 / 4)
        .saturating_add(tsc_bias_sign as u32 / 4);

    let mut s = MODULE.lock();
    let tsc_adjust_ema = ((s.tsc_adjust_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.tsc_bias_present = tsc_bias_present;
    s.tsc_bias_magnitude = tsc_bias_magnitude;
    s.tsc_bias_sign = tsc_bias_sign;
    s.tsc_adjust_ema = tsc_adjust_ema;

    serial_println!("[msr_ia32_tsc_adjust] age={} present={} mag={} neg={} ema={}",
        age, tsc_bias_present, tsc_bias_magnitude, tsc_bias_sign, tsc_adjust_ema);
}

pub fn get_tsc_bias_present()   -> u16 { MODULE.lock().tsc_bias_present }
pub fn get_tsc_bias_magnitude() -> u16 { MODULE.lock().tsc_bias_magnitude }
pub fn get_tsc_bias_sign()      -> u16 { MODULE.lock().tsc_bias_sign }
pub fn get_tsc_adjust_ema()     -> u16 { MODULE.lock().tsc_adjust_ema }
