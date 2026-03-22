#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { proc_hlt_exit: u16, proc_tsc_offset: u16, proc_density: u16, proc_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { proc_hlt_exit:0, proc_tsc_offset:0, proc_density:0, proc_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_procbased_ctls] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x482u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let proc_hlt_exit: u16 = if (lo >> 7) & 1 != 0 { 1000 } else { 0 };
    let proc_tsc_offset: u16 = if (lo >> 3) & 1 != 0 { 1000 } else { 0 };
    let bits = popcount(lo);
    let proc_density = ((bits * 31).min(1000)) as u16;
    let composite = (proc_hlt_exit as u32/3).saturating_add(proc_tsc_offset as u32/3).saturating_add(proc_density as u32/3);
    let _ = hi;
    let mut s = MODULE.lock();
    let proc_ema = ((s.proc_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.proc_hlt_exit=proc_hlt_exit; s.proc_tsc_offset=proc_tsc_offset; s.proc_density=proc_density; s.proc_ema=proc_ema;
    serial_println!("[msr_ia32_vmx_procbased_ctls] age={} hlt={} tsc_off={} density={} ema={}", age, proc_hlt_exit, proc_tsc_offset, proc_density, proc_ema);
}
pub fn get_proc_hlt_exit()   -> u16 { MODULE.lock().proc_hlt_exit }
pub fn get_proc_tsc_offset() -> u16 { MODULE.lock().proc_tsc_offset }
pub fn get_proc_density()    -> u16 { MODULE.lock().proc_density }
pub fn get_proc_ema()        -> u16 { MODULE.lock().proc_ema }
