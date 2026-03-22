#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr2_type: u16, mtrr2_valid: u16, mtrr2_base: u16, mtrr2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr2_type:0, mtrr2_valid:0, mtrr2_base:0, mtrr2_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mtrr_physbase2] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x204u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x205u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    // base bits[2:0]: memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
    let raw_type = base_lo & 0x7;
    let mtrr2_type = (raw_type * 1000 / 6) as u16;
    // mask bit 11: valid — MTRR range active
    let mtrr2_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // base bits[31:12]: physical base address sense
    let mtrr2_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr2_type as u32/3).saturating_add(mtrr2_valid as u32/3).saturating_add(mtrr2_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr2_ema = ((s.mtrr2_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr2_type=mtrr2_type; s.mtrr2_valid=mtrr2_valid; s.mtrr2_base=mtrr2_base; s.mtrr2_ema=mtrr2_ema;
    serial_println!("[msr_ia32_mtrr_physbase2] age={} type={} valid={} base={} ema={}", age, mtrr2_type, mtrr2_valid, mtrr2_base, mtrr2_ema);
}
pub fn get_mtrr2_type()  -> u16 { MODULE.lock().mtrr2_type }
pub fn get_mtrr2_valid() -> u16 { MODULE.lock().mtrr2_valid }
pub fn get_mtrr2_base()  -> u16 { MODULE.lock().mtrr2_base }
pub fn get_mtrr2_ema()   -> u16 { MODULE.lock().mtrr2_ema }
