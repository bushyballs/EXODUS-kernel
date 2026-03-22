#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { sgx_key0_lo: u16, sgx_key0_hi: u16, sgx_key_set: u16, sgx_key_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { sgx_key0_lo:0, sgx_key0_hi:0, sgx_key_set:0, sgx_key_ema:0 });

#[inline]
fn has_sgx() -> bool {
    let ebx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            options(nostack, nomem),
        );
    }
    (ebx >> 2) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_sgxlepubkeyhash0] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_sgx() { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x8Cu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // Bits[63:0] of the SGX Launch Enclave public key hash (first 64 bits)
    let sgx_key0_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let sgx_key0_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let sgx_key_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (sgx_key0_lo as u32/3).saturating_add(sgx_key0_hi as u32/3).saturating_add(sgx_key_set as u32/3);
    let mut s = MODULE.lock();
    let sgx_key_ema = ((s.sgx_key_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.sgx_key0_lo=sgx_key0_lo; s.sgx_key0_hi=sgx_key0_hi; s.sgx_key_set=sgx_key_set; s.sgx_key_ema=sgx_key_ema;
    serial_println!("[msr_ia32_sgxlepubkeyhash0] age={} lo={} hi={} set={} ema={}", age, sgx_key0_lo, sgx_key0_hi, sgx_key_set, sgx_key_ema);
}
pub fn get_sgx_key0_lo()  -> u16 { MODULE.lock().sgx_key0_lo }
pub fn get_sgx_key0_hi()  -> u16 { MODULE.lock().sgx_key0_hi }
pub fn get_sgx_key_set()  -> u16 { MODULE.lock().sgx_key_set }
pub fn get_sgx_key_ema()  -> u16 { MODULE.lock().sgx_key_ema }
