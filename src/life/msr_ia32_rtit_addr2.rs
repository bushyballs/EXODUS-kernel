#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { addr2_a_nonzero: u16, addr2_b_nonzero: u16, addr2_active: u16, addr2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { addr2_a_nonzero: 0, addr2_b_nonzero: 0, addr2_active: 0, addr2_ema: 0 });

fn pt_addr2_supported() -> bool {
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x14 { return false; }
    let leaf14_eax: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0x14u32 => leaf14_eax, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
    if leaf14_eax == 0 { return false; }
    let ebx_s1: u32;
    unsafe {
        asm!("push rbx","cpuid","mov {0:e}, ebx","pop rbx",
             out(reg) ebx_s1, inout("eax") 0x14u32 => _, inout("ecx") 1u32 => _, lateout("edx") _,
             options(nostack,nomem));
    }
    (ebx_s1 & 0xF) >= 3
}
pub fn init() { serial_println!("[msr_ia32_rtit_addr2] init"); }
pub fn tick(age: u32) {
    if age % 3500 != 0 { return; }
    if !pt_addr2_supported() { return; }
    let a_lo: u32; let a_hi: u32; let b_lo: u32; let b_hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x584u32, out("eax") a_lo, out("edx") a_hi, options(nostack,nomem));
        asm!("rdmsr", in("ecx") 0x585u32, out("eax") b_lo, out("edx") b_hi, options(nostack,nomem));
    }
    let addr2_a_nonzero: u16 = if a_lo != 0 || a_hi != 0 { 1000 } else { 0 };
    let addr2_b_nonzero: u16 = if b_lo != 0 || b_hi != 0 { 1000 } else { 0 };
    let addr2_active: u16 = if addr2_a_nonzero == 1000 && addr2_b_nonzero == 1000 { 1000 } else { 0 };
    let composite: u16 = (addr2_active/4).saturating_add(addr2_a_nonzero/4).saturating_add(addr2_b_nonzero/2);
    let mut s = MODULE.lock();
    let ema = ((s.addr2_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.addr2_a_nonzero = addr2_a_nonzero; s.addr2_b_nonzero = addr2_b_nonzero; s.addr2_active = addr2_active; s.addr2_ema = ema;
    serial_println!("[msr_ia32_rtit_addr2] age={} active={} a_nz={} b_nz={} ema={}", age, addr2_active, addr2_a_nonzero, addr2_b_nonzero, ema);
}
pub fn get_addr2_a_nonzero() -> u16 { MODULE.lock().addr2_a_nonzero }
pub fn get_addr2_b_nonzero() -> u16 { MODULE.lock().addr2_b_nonzero }
pub fn get_addr2_active() -> u16 { MODULE.lock().addr2_active }
pub fn get_addr2_ema() -> u16 { MODULE.lock().addr2_ema }
