#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { sgx1_supported: u16, sgx2_supported: u16, sgx_miscselect: u16, sgx_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { sgx1_supported: 0, sgx2_supported: 0, sgx_miscselect: 0, sgx_ema: 0 });

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0{c+=v&1;v>>=1;} c }

fn has_sgx() -> bool {
    let ebx_7: u32;
    unsafe {
        asm!("push rbx","cpuid","mov {0:e}, ebx","pop rbx", out(reg) ebx_7, inout("eax") 7u32 => _, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem));
    }
    (ebx_7 >> 2) & 1 == 1
}
pub fn init() { serial_println!("[cpuid_sgx_leaf] init"); }
pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }
    if !has_sgx() { return; }
    let eax_12: u32; let ebx_12: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "mov {1:e}, ebx", "pop rbx",
            inout("eax") 0x12u32 => eax_12,
            out(reg) ebx_12,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let sgx1_supported: u16 = if eax_12 & 1 != 0 { 1000 } else { 0 };
    let sgx2_supported: u16 = if (eax_12 >> 1) & 1 != 0 { 1000 } else { 0 };
    let sgx_miscselect: u16 = (popcount(ebx_12) * 31).min(1000) as u16;
    let composite: u16 = (sgx1_supported/4).saturating_add(sgx2_supported/4).saturating_add(sgx_miscselect/2);
    let mut s = MODULE.lock();
    let ema = ((s.sgx_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.sgx1_supported = sgx1_supported; s.sgx2_supported = sgx2_supported; s.sgx_miscselect = sgx_miscselect; s.sgx_ema = ema;
    serial_println!("[cpuid_sgx_leaf] age={} sgx1={} sgx2={} misc={} ema={}", age, sgx1_supported, sgx2_supported, sgx_miscselect, ema);
}
pub fn get_sgx1_supported() -> u16 { MODULE.lock().sgx1_supported }
pub fn get_sgx2_supported() -> u16 { MODULE.lock().sgx2_supported }
pub fn get_sgx_miscselect() -> u16 { MODULE.lock().sgx_miscselect }
pub fn get_sgx_ema() -> u16 { MODULE.lock().sgx_ema }
