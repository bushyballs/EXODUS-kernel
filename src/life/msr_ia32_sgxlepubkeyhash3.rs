#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { sgx_key3_lo: u16, sgx_key3_hi: u16, sgx_key3_set: u16, sgx_key3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { sgx_key3_lo:0, sgx_key3_hi:0, sgx_key3_set:0, sgx_key3_ema:0 });

#[inline]
fn has_sgx() -> bool {
    let ebx: u32;
    unsafe {
        asm!(
            "push rbx", "mov eax, 7", "xor ecx, ecx",
            "cpuid", "mov {0:e}, ebx", "pop rbx",
            out(reg) ebx, options(nostack, nomem),
        );
    }
    (ebx >> 2) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_sgxlepubkeyhash3] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_sgx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x8Fu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let sgx_key3_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let sgx_key3_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let sgx_key3_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (sgx_key3_lo as u32/3).saturating_add(sgx_key3_hi as u32/3).saturating_add(sgx_key3_set as u32/3);
    let mut s = MODULE.lock();
    let sgx_key3_ema = ((s.sgx_key3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.sgx_key3_lo=sgx_key3_lo; s.sgx_key3_hi=sgx_key3_hi; s.sgx_key3_set=sgx_key3_set; s.sgx_key3_ema=sgx_key3_ema;
    serial_println!("[msr_ia32_sgxlepubkeyhash3] age={} lo={} hi={} set={} ema={}", age, sgx_key3_lo, sgx_key3_hi, sgx_key3_set, sgx_key3_ema);
}
pub fn get_sgx_key3_lo()  -> u16 { MODULE.lock().sgx_key3_lo }
pub fn get_sgx_key3_hi()  -> u16 { MODULE.lock().sgx_key3_hi }
pub fn get_sgx_key3_set() -> u16 { MODULE.lock().sgx_key3_set }
pub fn get_sgx_key3_ema() -> u16 { MODULE.lock().sgx_key3_ema }
