#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { bndcfgk_enable: u16, bndcfgk_bndpreserve: u16, bndcfgk_table: u16, bndcfgk_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { bndcfgk_enable: 0, bndcfgk_bndpreserve: 0, bndcfgk_table: 0, bndcfgk_ema: 0 });

fn has_mpx() -> bool {
    let ebx: u32;
    unsafe {
        asm!("push rbx","cpuid","mov {0:e}, ebx","pop rbx", out(reg) ebx, inout("eax") 7u32 => _, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem));
    }
    (ebx >> 14) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_bndcfgk] init"); }
pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }
    if !has_mpx() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xD91u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let bndcfgk_enable: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let bndcfgk_bndpreserve: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let bndcfgk_table: u16 = if (lo >> 2) & 0x3FF != 0 { 1000 } else { 0 };
    let composite: u16 = (bndcfgk_enable/4).saturating_add(bndcfgk_bndpreserve/4).saturating_add(bndcfgk_table/2);
    let mut s = MODULE.lock();
    let ema = ((s.bndcfgk_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.bndcfgk_enable = bndcfgk_enable; s.bndcfgk_bndpreserve = bndcfgk_bndpreserve; s.bndcfgk_table = bndcfgk_table; s.bndcfgk_ema = ema;
    serial_println!("[msr_ia32_bndcfgk] age={} lo={:#010x} en={} preserve={} table={} ema={}", age, lo, bndcfgk_enable, bndcfgk_bndpreserve, bndcfgk_table, ema);
}
pub fn get_bndcfgk_enable() -> u16 { MODULE.lock().bndcfgk_enable }
pub fn get_bndcfgk_bndpreserve() -> u16 { MODULE.lock().bndcfgk_bndpreserve }
pub fn get_bndcfgk_table() -> u16 { MODULE.lock().bndcfgk_table }
pub fn get_bndcfgk_ema() -> u16 { MODULE.lock().bndcfgk_ema }
