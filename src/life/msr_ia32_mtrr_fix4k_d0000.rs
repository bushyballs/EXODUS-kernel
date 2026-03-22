#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { fix4k_d0000_type: u16, fix4k_d0000_wb: u16, fix4k_d0000_uc: u16, fix4k_d0000_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { fix4k_d0000_type:0, fix4k_d0000_wb:0, fix4k_d0000_uc:0, fix4k_d0000_ema:0 });
pub fn init() { serial_println!("[msr_ia32_mtrr_fix4k_d0000] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x26Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut wb_count = 0u32; let mut uc_count = 0u32;
    for shift in [0u32, 8, 16, 24] {
        let t = (lo >> shift) & 0xFF;
        if t == 6 { wb_count += 1; } else if t == 0 { uc_count += 1; }
    }
    let fix4k_d0000_type = ((lo & 0xFF) * 1000 / 6).min(1000) as u16;
    let fix4k_d0000_wb = (wb_count * 250).min(1000) as u16;
    let fix4k_d0000_uc = (uc_count * 250).min(1000) as u16;
    let composite = (fix4k_d0000_type as u32/3).saturating_add(fix4k_d0000_wb as u32/3).saturating_add(fix4k_d0000_uc as u32/3);
    let mut s = MODULE.lock();
    let fix4k_d0000_ema = ((s.fix4k_d0000_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.fix4k_d0000_type=fix4k_d0000_type; s.fix4k_d0000_wb=fix4k_d0000_wb; s.fix4k_d0000_uc=fix4k_d0000_uc; s.fix4k_d0000_ema=fix4k_d0000_ema;
    serial_println!("[msr_ia32_mtrr_fix4k_d0000] age={} type={} wb={} uc={} ema={}", age, fix4k_d0000_type, fix4k_d0000_wb, fix4k_d0000_uc, fix4k_d0000_ema);
}
pub fn get_fix4k_d0000_type() -> u16 { MODULE.lock().fix4k_d0000_type }
pub fn get_fix4k_d0000_wb()   -> u16 { MODULE.lock().fix4k_d0000_wb }
pub fn get_fix4k_d0000_uc()   -> u16 { MODULE.lock().fix4k_d0000_uc }
pub fn get_fix4k_d0000_ema()  -> u16 { MODULE.lock().fix4k_d0000_ema }
