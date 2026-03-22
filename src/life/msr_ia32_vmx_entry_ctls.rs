#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { entry_load_efer: u16, entry_ia32e: u16, entry_smm: u16, entry_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { entry_load_efer:0, entry_ia32e:0, entry_smm:0, entry_ctl_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_entry_ctls] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x484u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let entry_load_efer: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };
    let entry_ia32e: u16 = if (lo >> 9) & 1 != 0 { 1000 } else { 0 };
    let entry_smm: u16 = if (lo >> 10) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (entry_load_efer as u32/3).saturating_add(entry_ia32e as u32/3).saturating_add(entry_smm as u32/3);
    let mut s = MODULE.lock();
    let entry_ctl_ema = ((s.entry_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.entry_load_efer=entry_load_efer; s.entry_ia32e=entry_ia32e; s.entry_smm=entry_smm; s.entry_ctl_ema=entry_ctl_ema;
    serial_println!("[msr_ia32_vmx_entry_ctls] age={} efer={} ia32e={} smm={} ema={}", age, entry_load_efer, entry_ia32e, entry_smm, entry_ctl_ema);
}
pub fn get_entry_load_efer() -> u16 { MODULE.lock().entry_load_efer }
pub fn get_entry_ia32e()     -> u16 { MODULE.lock().entry_ia32e }
pub fn get_entry_smm()       -> u16 { MODULE.lock().entry_smm }
pub fn get_entry_ctl_ema()   -> u16 { MODULE.lock().entry_ctl_ema }
