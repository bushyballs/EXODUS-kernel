#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr3_type: u16, mtrr3_valid: u16, mtrr3_base: u16, mtrr3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr3_type:0, mtrr3_valid:0, mtrr3_base:0, mtrr3_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mtrr_physbase3] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x206u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x207u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    // base bits[2:0]: memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
    let raw_type = base_lo & 0x7;
    let mtrr3_type = (raw_type * 1000 / 6) as u16;
    // mask bit 11: valid — MTRR range active
    let mtrr3_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // base bits[31:12]: physical base address sense
    let mtrr3_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr3_type as u32/3).saturating_add(mtrr3_valid as u32/3).saturating_add(mtrr3_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr3_ema = ((s.mtrr3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr3_type=mtrr3_type; s.mtrr3_valid=mtrr3_valid; s.mtrr3_base=mtrr3_base; s.mtrr3_ema=mtrr3_ema;
    serial_println!("[msr_ia32_mtrr_physbase3] age={} type={} valid={} base={} ema={}", age, mtrr3_type, mtrr3_valid, mtrr3_base, mtrr3_ema);
}
pub fn get_mtrr3_type()  -> u16 { MODULE.lock().mtrr3_type }
pub fn get_mtrr3_valid() -> u16 { MODULE.lock().mtrr3_valid }
pub fn get_mtrr3_base()  -> u16 { MODULE.lock().mtrr3_base }
pub fn get_mtrr3_ema()   -> u16 { MODULE.lock().mtrr3_ema }
