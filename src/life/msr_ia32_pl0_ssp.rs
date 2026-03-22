#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pl0_ssp_lo: u16, pl0_ssp_hi: u16, pl0_ssp_set: u16, pl0_ssp_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pl0_ssp_lo:0, pl0_ssp_hi:0, pl0_ssp_set:0, pl0_ssp_ema:0 });

#[inline]
fn has_cet_ss() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "pop rbx",
            lateout("ecx") ecx,
            lateout("eax") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_pl0_ssp] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_cet_ss() { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6A4u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // PL0 (ring-0) shadow stack pointer — virtual address
    let pl0_ssp_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let pl0_ssp_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let pl0_ssp_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (pl0_ssp_lo as u32/3).saturating_add(pl0_ssp_hi as u32/3).saturating_add(pl0_ssp_set as u32/3);
    let mut s = MODULE.lock();
    let pl0_ssp_ema = ((s.pl0_ssp_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pl0_ssp_lo=pl0_ssp_lo; s.pl0_ssp_hi=pl0_ssp_hi; s.pl0_ssp_set=pl0_ssp_set; s.pl0_ssp_ema=pl0_ssp_ema;
    serial_println!("[msr_ia32_pl0_ssp] age={} lo={} hi={} set={} ema={}", age, pl0_ssp_lo, pl0_ssp_hi, pl0_ssp_set, pl0_ssp_ema);
}
pub fn get_pl0_ssp_lo()  -> u16 { MODULE.lock().pl0_ssp_lo }
pub fn get_pl0_ssp_hi()  -> u16 { MODULE.lock().pl0_ssp_hi }
pub fn get_pl0_ssp_set() -> u16 { MODULE.lock().pl0_ssp_set }
pub fn get_pl0_ssp_ema() -> u16 { MODULE.lock().pl0_ssp_ema }
