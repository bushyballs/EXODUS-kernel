#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tsc_ratio_valid: u16, crystal_present: u16, ratio_sense: u16, tsc_freq_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tsc_ratio_valid: 0, crystal_present: 0, ratio_sense: 0, tsc_freq_ema: 0 });

pub fn init() { serial_println!("[cpuid_tsc_freq] init"); }
pub fn tick(age: u32) {
    if age % 9000 != 0 { return; }
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x15 { return; }
    let eax_out: u32; let ebx_out: u32; let ecx_out: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "mov {1:e}, ebx", "pop rbx",
            inout("eax") 0x15u32 => eax_out,
            out(reg) ebx_out,
            inout("ecx") 0u32 => ecx_out,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let tsc_ratio_valid: u16 = if ebx_out != 0 && eax_out != 0 { 1000 } else { 0 };
    let crystal_present: u16 = if ecx_out != 0 { 1000 } else { 0 };
    let ratio_sense: u16 = if eax_out == 0 {
        0
    } else {
        ((ebx_out / eax_out).min(4) * 250) as u16
    };
    let composite: u16 = (tsc_ratio_valid/4).saturating_add(crystal_present/4).saturating_add(ratio_sense/2);
    let mut s = MODULE.lock();
    let ema = ((s.tsc_freq_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.tsc_ratio_valid = tsc_ratio_valid; s.crystal_present = crystal_present; s.ratio_sense = ratio_sense; s.tsc_freq_ema = ema;
    serial_println!("[cpuid_tsc_freq] age={} eax={} ebx={} ecx={} ratio_valid={} crystal={} ratio={} ema={}", age, eax_out, ebx_out, ecx_out, tsc_ratio_valid, crystal_present, ratio_sense, ema);
}
pub fn get_tsc_ratio_valid() -> u16 { MODULE.lock().tsc_ratio_valid }
pub fn get_crystal_present() -> u16 { MODULE.lock().crystal_present }
pub fn get_ratio_sense() -> u16 { MODULE.lock().ratio_sense }
pub fn get_tsc_freq_ema() -> u16 { MODULE.lock().tsc_freq_ema }
