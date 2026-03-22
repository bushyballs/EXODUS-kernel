#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { feature_lock: u16, vmx_enable: u16, smx_enable: u16, feature_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { feature_lock:0, vmx_enable:0, smx_enable:0, feature_ema:0 });

pub fn init() { serial_println!("[msr_ia32_feature_control] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let feature_lock: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let vmx_enable: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let smx_enable: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let composite = (feature_lock as u32/3).saturating_add(vmx_enable as u32/3).saturating_add(smx_enable as u32/3);
    let mut s = MODULE.lock();
    let feature_ema = ((s.feature_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.feature_lock=feature_lock; s.vmx_enable=vmx_enable; s.smx_enable=smx_enable; s.feature_ema=feature_ema;
    serial_println!("[msr_ia32_feature_control] age={} lock={} vmx={} smx={} ema={}", age, feature_lock, vmx_enable, smx_enable, feature_ema);
}
pub fn get_feature_lock() -> u16 { MODULE.lock().feature_lock }
pub fn get_vmx_enable()   -> u16 { MODULE.lock().vmx_enable }
pub fn get_smx_enable()   -> u16 { MODULE.lock().smx_enable }
pub fn get_feature_ema()  -> u16 { MODULE.lock().feature_ema }
