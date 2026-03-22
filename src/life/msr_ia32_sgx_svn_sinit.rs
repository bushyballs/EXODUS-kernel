#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { sgx_svn: u16, sgx_active: u16, sgx_svn_ema: u16, sgx_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { sgx_svn:0, sgx_active:0, sgx_svn_ema:0, sgx_pad:0 });

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
    // CPUID 7.0 EBX bit 2: SGX
    (ebx >> 2) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_sgx_svn_sinit] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_sgx() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x500u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[15:8]: SGX_SVN_SINIT — launch enclave security version number
    let raw_svn = (lo >> 8) & 0xFF;
    let sgx_svn = ((raw_svn * 1000) / 255) as u16;
    // Nonzero SVN = SGX SINIT region is configured
    let sgx_active: u16 = if raw_svn != 0 { 1000 } else { 0 };
    let composite = (sgx_svn as u32/2).saturating_add(sgx_active as u32/2);
    let mut s = MODULE.lock();
    let sgx_svn_ema = ((s.sgx_svn_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.sgx_svn=sgx_svn; s.sgx_active=sgx_active; s.sgx_svn_ema=sgx_svn_ema;
    serial_println!("[msr_ia32_sgx_svn_sinit] age={} svn={} active={} ema={}", age, sgx_svn, sgx_active, sgx_svn_ema);
}
pub fn get_sgx_svn()     -> u16 { MODULE.lock().sgx_svn }
pub fn get_sgx_active()  -> u16 { MODULE.lock().sgx_active }
pub fn get_sgx_svn_ema() -> u16 { MODULE.lock().sgx_svn_ema }
