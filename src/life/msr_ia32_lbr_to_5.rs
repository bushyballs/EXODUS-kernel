#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_to5_addr: u16, lbr_to5_cycles: u16, lbr_to5_set: u16, lbr_to5_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_to5_addr:0, lbr_to5_cycles:0, lbr_to5_set:0, lbr_to5_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lbr_to_5] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx", lateout("eax") _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 19) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6C5u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_to5_addr = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let lbr_to5_cycles = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let lbr_to5_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (lbr_to5_addr as u32/3).saturating_add(lbr_to5_cycles as u32/3).saturating_add(lbr_to5_set as u32/3);
    let mut s = MODULE.lock();
    let lbr_to5_ema = ((s.lbr_to5_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_to5_addr=lbr_to5_addr; s.lbr_to5_cycles=lbr_to5_cycles; s.lbr_to5_set=lbr_to5_set; s.lbr_to5_ema=lbr_to5_ema;
    serial_println!("[msr_ia32_lbr_to_5] age={} addr={} cycles={} set={} ema={}", age, lbr_to5_addr, lbr_to5_cycles, lbr_to5_set, lbr_to5_ema);
}
pub fn get_lbr_to5_addr()   -> u16 { MODULE.lock().lbr_to5_addr }
pub fn get_lbr_to5_cycles() -> u16 { MODULE.lock().lbr_to5_cycles }
pub fn get_lbr_to5_set()    -> u16 { MODULE.lock().lbr_to5_set }
pub fn get_lbr_to5_ema()    -> u16 { MODULE.lock().lbr_to5_ema }
