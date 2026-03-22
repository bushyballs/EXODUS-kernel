#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr8_type: u16, mtrr8_valid: u16, mtrr8_base: u16, mtrr8_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr8_type:0, mtrr8_valid:0, mtrr8_base:0, mtrr8_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase8] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x210u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x211u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr8_type = (raw_type * 1000 / 6) as u16;
    let mtrr8_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr8_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr8_type as u32/3).saturating_add(mtrr8_valid as u32/3).saturating_add(mtrr8_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr8_ema = ((s.mtrr8_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr8_type=mtrr8_type; s.mtrr8_valid=mtrr8_valid; s.mtrr8_base=mtrr8_base; s.mtrr8_ema=mtrr8_ema;
    serial_println!("[msr_ia32_mtrr_physbase8] age={} type={} valid={} base={} ema={}", age, mtrr8_type, mtrr8_valid, mtrr8_base, mtrr8_ema);
}
pub fn get_mtrr8_type()  -> u16 { MODULE.lock().mtrr8_type }
pub fn get_mtrr8_valid() -> u16 { MODULE.lock().mtrr8_valid }
pub fn get_mtrr8_base()  -> u16 { MODULE.lock().mtrr8_base }
pub fn get_mtrr8_ema()   -> u16 { MODULE.lock().mtrr8_ema }
