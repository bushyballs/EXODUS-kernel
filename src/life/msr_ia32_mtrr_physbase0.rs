#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr0_type: u16, mtrr0_valid: u16, mtrr0_base: u16, mtrr0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr0_type:0, mtrr0_valid:0, mtrr0_base:0, mtrr0_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mtrr_physbase0] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let base_lo: u32; let mask_lo: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x200u32, out("eax") base_lo, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x201u32, out("eax") mask_lo, out("edx") _, options(nostack, nomem));
    }
    // base bits[2:0]: memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
    let raw_type = base_lo & 0x7;
    let mtrr0_type = (raw_type * 1000 / 6) as u16;
    // mask bit 11: valid — MTRR range active
    let mtrr0_valid: u16 = if (mask_lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // base bits[31:12]: physical base address sense
    let mtrr0_base = ((base_lo >> 12) & 0xFFFF) as u16 * 1000 / 65535;
    let composite = (mtrr0_type as u32/3).saturating_add(mtrr0_valid as u32/3).saturating_add(mtrr0_base as u32/3);
    let mut s = MODULE.lock();
    let mtrr0_ema = ((s.mtrr0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr0_type=mtrr0_type; s.mtrr0_valid=mtrr0_valid; s.mtrr0_base=mtrr0_base; s.mtrr0_ema=mtrr0_ema;
    serial_println!("[msr_ia32_mtrr_physbase0] age={} type={} valid={} base={} ema={}", age, mtrr0_type, mtrr0_valid, mtrr0_base, mtrr0_ema);
}
pub fn get_mtrr0_type()  -> u16 { MODULE.lock().mtrr0_type }
pub fn get_mtrr0_valid() -> u16 { MODULE.lock().mtrr0_valid }
pub fn get_mtrr0_base()  -> u16 { MODULE.lock().mtrr0_base }
pub fn get_mtrr0_ema()   -> u16 { MODULE.lock().mtrr0_ema }
