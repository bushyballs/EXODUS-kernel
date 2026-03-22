#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr4_type: u16, mtrr4_valid: u16, mtrr4_base: u16, mtrr4_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr4_type:0, mtrr4_valid:0, mtrr4_base:0, mtrr4_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase4] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x208u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x209u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr4_type = (raw_type * 1000 / 6) as u16;
    let mtrr4_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr4_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr4_type as u32/3).saturating_add(mtrr4_valid as u32/3).saturating_add(mtrr4_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr4_ema = ((s.mtrr4_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr4_type=mtrr4_type; s.mtrr4_valid=mtrr4_valid; s.mtrr4_base=mtrr4_base; s.mtrr4_ema=mtrr4_ema;
    serial_println!("[msr_ia32_mtrr_physbase4] age={} type={} valid={} base={} ema={}", age, mtrr4_type, mtrr4_valid, mtrr4_base, mtrr4_ema);
}
pub fn get_mtrr4_type()  -> u16 { MODULE.lock().mtrr4_type }
pub fn get_mtrr4_valid() -> u16 { MODULE.lock().mtrr4_valid }
pub fn get_mtrr4_base()  -> u16 { MODULE.lock().mtrr4_base }
pub fn get_mtrr4_ema()   -> u16 { MODULE.lock().mtrr4_ema }
