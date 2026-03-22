#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mtrr_fix16k_a0_type: u16, mtrr_fix16k_a0_wb: u16, mtrr_fix16k_a0_uc: u16, mtrr_fix16k_a0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mtrr_fix16k_a0_type:0, mtrr_fix16k_a0_wb:0, mtrr_fix16k_a0_uc:0, mtrr_fix16k_a0_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mtrr_fix16k_a0000] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x259u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut wb_count = 0u32;
    let mut uc_count = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let t = (lo >> shift) & 0xFF;
        if t == 6 { wb_count += 1; }
        else if t == 0 { uc_count += 1; }
    }
    let mtrr_fix16k_a0_type = ((lo & 0xFF) * 1000 / 6).min(1000) as u16;
    let mtrr_fix16k_a0_wb = (wb_count * 250).min(1000) as u16;
    let mtrr_fix16k_a0_uc = (uc_count * 250).min(1000) as u16;
    let composite = (mtrr_fix16k_a0_type as u32/3).saturating_add(mtrr_fix16k_a0_wb as u32/3).saturating_add(mtrr_fix16k_a0_uc as u32/3);
    let mut s = MODULE.lock();
    let mtrr_fix16k_a0_ema = ((s.mtrr_fix16k_a0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mtrr_fix16k_a0_type=mtrr_fix16k_a0_type; s.mtrr_fix16k_a0_wb=mtrr_fix16k_a0_wb; s.mtrr_fix16k_a0_uc=mtrr_fix16k_a0_uc; s.mtrr_fix16k_a0_ema=mtrr_fix16k_a0_ema;
    serial_println!("[msr_ia32_mtrr_fix16k_a0000] age={} type={} wb={} uc={} ema={}", age, mtrr_fix16k_a0_type, mtrr_fix16k_a0_wb, mtrr_fix16k_a0_uc, mtrr_fix16k_a0_ema);
}
pub fn get_mtrr_fix16k_a0_type() -> u16 { MODULE.lock().mtrr_fix16k_a0_type }
pub fn get_mtrr_fix16k_a0_wb()   -> u16 { MODULE.lock().mtrr_fix16k_a0_wb }
pub fn get_mtrr_fix16k_a0_uc()   -> u16 { MODULE.lock().mtrr_fix16k_a0_uc }
pub fn get_mtrr_fix16k_a0_ema()  -> u16 { MODULE.lock().mtrr_fix16k_a0_ema }
