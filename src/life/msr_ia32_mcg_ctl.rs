#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { mcg_ctl_banks: u16, mcg_bank_coverage: u16, mcg_ctl_active: u16, mcg_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mcg_ctl_banks:0, mcg_bank_coverage:0, mcg_ctl_active:0, mcg_ctl_ema:0 });

#[inline]
fn has_mce() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mcg_ctl] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_mce() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x17Bu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let mcg_ctl_active: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let bits = popcount(lo);
    let mcg_ctl_banks = ((bits * 31).min(1000)) as u16;
    let mcg_bank_coverage = mcg_ctl_banks;
    let composite = (mcg_ctl_active as u32/3).saturating_add(mcg_ctl_banks as u32/3).saturating_add(mcg_bank_coverage as u32/3);
    let mut s = MODULE.lock();
    let mcg_ctl_ema = ((s.mcg_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mcg_ctl_banks=mcg_ctl_banks; s.mcg_bank_coverage=mcg_bank_coverage; s.mcg_ctl_active=mcg_ctl_active; s.mcg_ctl_ema=mcg_ctl_ema;
    serial_println!("[msr_ia32_mcg_ctl] age={} banks={} cov={} active={} ema={}", age, mcg_ctl_banks, mcg_bank_coverage, mcg_ctl_active, mcg_ctl_ema);
}
pub fn get_mcg_ctl_banks()    -> u16 { MODULE.lock().mcg_ctl_banks }
pub fn get_mcg_bank_coverage()-> u16 { MODULE.lock().mcg_bank_coverage }
pub fn get_mcg_ctl_active()   -> u16 { MODULE.lock().mcg_ctl_active }
pub fn get_mcg_ctl_ema()      -> u16 { MODULE.lock().mcg_ctl_ema }
