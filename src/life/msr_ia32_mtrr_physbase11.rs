#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr11_type: u16, mtrr11_valid: u16, mtrr11_base: u16, mtrr11_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr11_type:0, mtrr11_valid:0, mtrr11_base:0, mtrr11_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase11] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x216u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x217u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr11_type = (raw_type * 1000 / 6) as u16;
    let mtrr11_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr11_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr11_type as u32/3).saturating_add(mtrr11_valid as u32/3).saturating_add(mtrr11_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr11_ema = ((s.mtrr11_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr11_type=mtrr11_type; s.mtrr11_valid=mtrr11_valid; s.mtrr11_base=mtrr11_base; s.mtrr11_ema=mtrr11_ema;
    serial_println!("[msr_ia32_mtrr_physbase11] age={} type={} valid={} base={} ema={}", age, mtrr11_type, mtrr11_valid, mtrr11_base, mtrr11_ema);
}
pub fn get_mtrr11_type()  -> u16 { MODULE.lock().mtrr11_type }
pub fn get_mtrr11_valid() -> u16 { MODULE.lock().mtrr11_valid }
pub fn get_mtrr11_base()  -> u16 { MODULE.lock().mtrr11_base }
pub fn get_mtrr11_ema()   -> u16 { MODULE.lock().mtrr11_ema }
