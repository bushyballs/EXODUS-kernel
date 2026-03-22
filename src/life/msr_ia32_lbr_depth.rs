#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_depth_val: u16, lbr_max_depth: u16, lbr_depth_ratio: u16, lbr_depth_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_depth_val: 0, lbr_max_depth: 0, lbr_depth_ratio: 0, lbr_depth_ema: 0 });

pub fn init() { serial_println!("[msr_ia32_lbr_depth] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x1C { return; }
    let lbr_cap_eax: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0x1Cu32 => lbr_cap_eax, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
    if lbr_cap_eax == 0 { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x14D6u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let depth_val = (lo & 0xFF) as u32;
    let max_depth = (lbr_cap_eax & 0xFF) as u32;
    let lbr_depth_val: u16 = if depth_val == 0 { 0 } else { ((depth_val * 1000) / 512).min(1000) as u16 };
    let lbr_max_depth: u16 = if max_depth == 0 { 0 } else { ((max_depth * 1000) / 512).min(1000) as u16 };
    let lbr_depth_ratio: u16 = if max_depth == 0 { 0 } else { ((depth_val * 1000) / max_depth).min(1000) as u16 };
    let composite: u16 = (lbr_depth_val/4).saturating_add(lbr_max_depth/4).saturating_add(lbr_depth_ratio/2);
    let mut s = MODULE.lock();
    let ema = ((s.lbr_depth_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.lbr_depth_val = lbr_depth_val; s.lbr_max_depth = lbr_max_depth; s.lbr_depth_ratio = lbr_depth_ratio; s.lbr_depth_ema = ema;
    serial_println!("[msr_ia32_lbr_depth] age={} depth={} max={} ratio={} ema={}", age, lbr_depth_val, lbr_max_depth, lbr_depth_ratio, ema);
}
pub fn get_lbr_depth_val() -> u16 { MODULE.lock().lbr_depth_val }
pub fn get_lbr_max_depth() -> u16 { MODULE.lock().lbr_max_depth }
pub fn get_lbr_depth_ratio() -> u16 { MODULE.lock().lbr_depth_ratio }
pub fn get_lbr_depth_ema() -> u16 { MODULE.lock().lbr_depth_ema }
