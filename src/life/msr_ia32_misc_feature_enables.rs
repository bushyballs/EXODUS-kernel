#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { misc_feature_sld: u16, misc_feature_cpuid_fault: u16, misc_feature_en: u16, misc_feature_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { misc_feature_sld:0, misc_feature_cpuid_fault:0, misc_feature_en:0, misc_feature_ema:0 });

pub fn init() { serial_println!("[msr_ia32_misc_feature_enables] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x140u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: Split Lock Detection enable
    let misc_feature_sld: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: CPUID Faulting enable (raises #GP on CPUID in user mode)
    let misc_feature_cpuid_fault: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let misc_feature_en: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (misc_feature_sld as u32/3).saturating_add(misc_feature_cpuid_fault as u32/3).saturating_add(misc_feature_en as u32/3);
    let mut s = MODULE.lock();
    let misc_feature_ema = ((s.misc_feature_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.misc_feature_sld=misc_feature_sld; s.misc_feature_cpuid_fault=misc_feature_cpuid_fault; s.misc_feature_en=misc_feature_en; s.misc_feature_ema=misc_feature_ema;
    serial_println!("[msr_ia32_misc_feature_enables] age={} sld={} cpuid_flt={} en={} ema={}", age, misc_feature_sld, misc_feature_cpuid_fault, misc_feature_en, misc_feature_ema);
}
pub fn get_misc_feature_sld()         -> u16 { MODULE.lock().misc_feature_sld }
pub fn get_misc_feature_cpuid_fault() -> u16 { MODULE.lock().misc_feature_cpuid_fault }
pub fn get_misc_feature_en()          -> u16 { MODULE.lock().misc_feature_en }
pub fn get_misc_feature_ema()         -> u16 { MODULE.lock().misc_feature_ema }
