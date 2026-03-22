#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr7_type: u16, mtrr7_valid: u16, mtrr7_base: u16, mtrr7_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr7_type:0, mtrr7_valid:0, mtrr7_base:0, mtrr7_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase7] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x20Eu32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x20Fu32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr7_type = (raw_type * 1000 / 6) as u16;
    let mtrr7_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr7_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr7_type as u32/3).saturating_add(mtrr7_valid as u32/3).saturating_add(mtrr7_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr7_ema = ((s.mtrr7_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr7_type=mtrr7_type; s.mtrr7_valid=mtrr7_valid; s.mtrr7_base=mtrr7_base; s.mtrr7_ema=mtrr7_ema;
    serial_println!("[msr_ia32_mtrr_physbase7] age={} type={} valid={} base={} ema={}", age, mtrr7_type, mtrr7_valid, mtrr7_base, mtrr7_ema);
}
pub fn get_mtrr7_type()  -> u16 { MODULE.lock().mtrr7_type }
pub fn get_mtrr7_valid() -> u16 { MODULE.lock().mtrr7_valid }
pub fn get_mtrr7_base()  -> u16 { MODULE.lock().mtrr7_base }
pub fn get_mtrr7_ema()   -> u16 { MODULE.lock().mtrr7_ema }
