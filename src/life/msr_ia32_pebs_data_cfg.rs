#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pebs_gpr_en: u16, pebs_xmm_en: u16, pebs_lbr_en: u16, pebs_cfg_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pebs_gpr_en: 0, pebs_xmm_en: 0, pebs_lbr_en: 0, pebs_cfg_ema: 0 });

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 15) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_pebs_data_cfg] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_pdcm() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3F7u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let pebs_gpr_en: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let pebs_xmm_en: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let pebs_lbr_en: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (pebs_gpr_en/4).saturating_add(pebs_xmm_en/4).saturating_add(pebs_lbr_en/2);
    let mut s = MODULE.lock();
    let ema = ((s.pebs_cfg_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.pebs_gpr_en = pebs_gpr_en; s.pebs_xmm_en = pebs_xmm_en; s.pebs_lbr_en = pebs_lbr_en; s.pebs_cfg_ema = ema;
    serial_println!("[msr_ia32_pebs_data_cfg] age={} lo={:#010x} gpr={} xmm={} lbr={} ema={}", age, lo, pebs_gpr_en, pebs_xmm_en, pebs_lbr_en, ema);
}
pub fn get_pebs_gpr_en() -> u16 { MODULE.lock().pebs_gpr_en }
pub fn get_pebs_xmm_en() -> u16 { MODULE.lock().pebs_xmm_en }
pub fn get_pebs_lbr_en() -> u16 { MODULE.lock().pebs_lbr_en }
pub fn get_pebs_cfg_ema() -> u16 { MODULE.lock().pebs_cfg_ema }
