#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr1_type: u16, mtrr1_valid: u16, mtrr1_base: u16, mtrr1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr1_type:0, mtrr1_valid:0, mtrr1_base:0, mtrr1_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mtrr_physbase1] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x202u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x203u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    // base bits[2:0]: memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
    let raw_type = base_lo & 0x7;
    let mtrr1_type = (raw_type * 1000 / 6) as u16;
    // mask bit 11: valid — MTRR range active
    let mtrr1_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // base bits[31:12]: physical base address sense
    let mtrr1_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr1_type as u32/3).saturating_add(mtrr1_valid as u32/3).saturating_add(mtrr1_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr1_ema = ((s.mtrr1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr1_type=mtrr1_type; s.mtrr1_valid=mtrr1_valid; s.mtrr1_base=mtrr1_base; s.mtrr1_ema=mtrr1_ema;
    serial_println!("[msr_ia32_mtrr_physbase1] age={} type={} valid={} base={} ema={}", age, mtrr1_type, mtrr1_valid, mtrr1_base, mtrr1_ema);
}
pub fn get_mtrr1_type()  -> u16 { MODULE.lock().mtrr1_type }
pub fn get_mtrr1_valid() -> u16 { MODULE.lock().mtrr1_valid }
pub fn get_mtrr1_base()  -> u16 { MODULE.lock().mtrr1_base }
pub fn get_mtrr1_ema()   -> u16 { MODULE.lock().mtrr1_ema }
