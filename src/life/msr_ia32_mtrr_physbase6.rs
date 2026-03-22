#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr6_type: u16, mtrr6_valid: u16, mtrr6_base: u16, mtrr6_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr6_type:0, mtrr6_valid:0, mtrr6_base:0, mtrr6_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_physbase6] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x20Cu32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x20Du32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    let raw_type = base_lo & 0x7;
    let mtrr6_type = (raw_type * 1000 / 6) as u16;
    let mtrr6_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let mtrr6_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr6_type as u32/3).saturating_add(mtrr6_valid as u32/3).saturating_add(mtrr6_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr6_ema = ((s.mtrr6_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr6_type=mtrr6_type; s.mtrr6_valid=mtrr6_valid; s.mtrr6_base=mtrr6_base; s.mtrr6_ema=mtrr6_ema;
    serial_println!("[msr_ia32_mtrr_physbase6] age={} type={} valid={} base={} ema={}", age, mtrr6_type, mtrr6_valid, mtrr6_base, mtrr6_ema);
}
pub fn get_mtrr6_type()  -> u16 { MODULE.lock().mtrr6_type }
pub fn get_mtrr6_valid() -> u16 { MODULE.lock().mtrr6_valid }
pub fn get_mtrr6_base()  -> u16 { MODULE.lock().mtrr6_base }
pub fn get_mtrr6_ema()   -> u16 { MODULE.lock().mtrr6_ema }
