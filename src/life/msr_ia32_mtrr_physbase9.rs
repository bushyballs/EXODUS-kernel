#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr9_type: u16, mtrr9_valid: u16, mtrr9_base: u16, mtrr9_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr9_type:0, mtrr9_valid:0, mtrr9_base:0, mtrr9_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase9] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x212u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x213u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr9_type = (raw_type * 1000 / 6) as u16;
    let mtrr9_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr9_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr9_type as u32/3).saturating_add(mtrr9_valid as u32/3).saturating_add(mtrr9_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr9_ema = ((s.mtrr9_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr9_type=mtrr9_type; s.mtrr9_valid=mtrr9_valid; s.mtrr9_base=mtrr9_base; s.mtrr9_ema=mtrr9_ema;
    serial_println!("[msr_ia32_mtrr_physbase9] age={} type={} valid={} base={} ema={}", age, mtrr9_type, mtrr9_valid, mtrr9_base, mtrr9_ema);
}
pub fn get_mtrr9_type()  -> u16 { MODULE.lock().mtrr9_type }
pub fn get_mtrr9_valid() -> u16 { MODULE.lock().mtrr9_valid }
pub fn get_mtrr9_base()  -> u16 { MODULE.lock().mtrr9_base }
pub fn get_mtrr9_ema()   -> u16 { MODULE.lock().mtrr9_ema }
