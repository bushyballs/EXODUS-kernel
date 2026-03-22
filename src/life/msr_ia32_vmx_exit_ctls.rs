#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { exit_save_efer: u16, exit_load_efer: u16, exit_ack_int: u16, exit_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { exit_save_efer:0, exit_load_efer:0, exit_ack_int:0, exit_ctl_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_exit_ctls] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x483u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let exit_ack_int: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };
    let exit_save_efer: u16 = if (lo >> 20) & 1 != 0 { 1000 } else { 0 };
    let exit_load_efer: u16 = if (lo >> 21) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (exit_ack_int as u32/3).saturating_add(exit_save_efer as u32/3).saturating_add(exit_load_efer as u32/3);
    let mut s = MODULE.lock();
    let exit_ctl_ema = ((s.exit_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.exit_save_efer=exit_save_efer; s.exit_load_efer=exit_load_efer; s.exit_ack_int=exit_ack_int; s.exit_ctl_ema=exit_ctl_ema;
    serial_println!("[msr_ia32_vmx_exit_ctls] age={} ack={} save_efer={} load_efer={} ema={}", age, exit_ack_int, exit_save_efer, exit_load_efer, exit_ctl_ema);
}
pub fn get_exit_save_efer()  -> u16 { MODULE.lock().exit_save_efer }
pub fn get_exit_load_efer()  -> u16 { MODULE.lock().exit_load_efer }
pub fn get_exit_ack_int()    -> u16 { MODULE.lock().exit_ack_int }
pub fn get_exit_ctl_ema()    -> u16 { MODULE.lock().exit_ctl_ema }
