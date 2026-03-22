#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr5_type: u16, mtrr5_valid: u16, mtrr5_base: u16, mtrr5_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr5_type:0, mtrr5_valid:0, mtrr5_base:0, mtrr5_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase5] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x20Au32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x20Bu32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr5_type = (raw_type * 1000 / 6) as u16;
    let mtrr5_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr5_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr5_type as u32/3).saturating_add(mtrr5_valid as u32/3).saturating_add(mtrr5_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr5_ema = ((s.mtrr5_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr5_type=mtrr5_type; s.mtrr5_valid=mtrr5_valid; s.mtrr5_base=mtrr5_base; s.mtrr5_ema=mtrr5_ema;
    serial_println!("[msr_ia32_mtrr_physbase5] age={} type={} valid={} base={} ema={}", age, mtrr5_type, mtrr5_valid, mtrr5_base, mtrr5_ema);
}
pub fn get_mtrr5_type()  -> u16 { MODULE.lock().mtrr5_type }
pub fn get_mtrr5_valid() -> u16 { MODULE.lock().mtrr5_valid }
pub fn get_mtrr5_base()  -> u16 { MODULE.lock().mtrr5_base }
pub fn get_mtrr5_ema()   -> u16 { MODULE.lock().mtrr5_ema }
