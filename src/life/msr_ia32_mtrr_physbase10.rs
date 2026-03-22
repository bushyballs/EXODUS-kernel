#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr10_type: u16, mtrr10_valid: u16, mtrr10_base: u16, mtrr10_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr10_type:0, mtrr10_valid:0, mtrr10_base:0, mtrr10_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase10] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x214u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x215u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr10_type = (raw_type * 1000 / 6) as u16;
    let mtrr10_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr10_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr10_type as u32/3).saturating_add(mtrr10_valid as u32/3).saturating_add(mtrr10_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr10_ema = ((s.mtrr10_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr10_type=mtrr10_type; s.mtrr10_valid=mtrr10_valid; s.mtrr10_base=mtrr10_base; s.mtrr10_ema=mtrr10_ema;
    serial_println!("[msr_ia32_mtrr_physbase10] age={} type={} valid={} base={} ema={}", age, mtrr10_type, mtrr10_valid, mtrr10_base, mtrr10_ema);
}
pub fn get_mtrr10_type()  -> u16 { MODULE.lock().mtrr10_type }
pub fn get_mtrr10_valid() -> u16 { MODULE.lock().mtrr10_valid }
pub fn get_mtrr10_base()  -> u16 { MODULE.lock().mtrr10_base }
pub fn get_mtrr10_ema()   -> u16 { MODULE.lock().mtrr10_ema }
