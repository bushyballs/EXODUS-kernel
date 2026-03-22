#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { if_masked: u16, df_masked: u16, fmask_density: u16, fmask_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { if_masked:0, df_masked:0, fmask_density:0, fmask_ema:0 });

pub fn init() { serial_println!("[msr_ia32_fmask] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000084u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let if_masked: u16 = if (lo >> 9) & 1 != 0 { 1000 } else { 0 };
    let df_masked: u16 = if (lo >> 10) & 1 != 0 { 1000 } else { 0 };
    let bits_set = popcount(lo & 0x3FFFF);
    let fmask_density = ((bits_set * 1000) / 18).min(1000) as u16;
    let composite = (if_masked as u32/3).saturating_add(df_masked as u32/3).saturating_add(fmask_density as u32/3);
    let mut s = MODULE.lock();
    let fmask_ema = ((s.fmask_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.if_masked=if_masked; s.df_masked=df_masked; s.fmask_density=fmask_density; s.fmask_ema=fmask_ema;
    serial_println!("[msr_ia32_fmask] age={} if_mask={} df_mask={} density={} ema={}", age, if_masked, df_masked, fmask_density, fmask_ema);
}
pub fn get_if_masked()     -> u16 { MODULE.lock().if_masked }
pub fn get_df_masked()     -> u16 { MODULE.lock().df_masked }
pub fn get_fmask_density() -> u16 { MODULE.lock().fmask_density }
pub fn get_fmask_ema()     -> u16 { MODULE.lock().fmask_ema }
