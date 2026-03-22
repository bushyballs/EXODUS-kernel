#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_to11_addr: u16, lbr_to11_cycles: u16, lbr_to11_set: u16, lbr_to11_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_to11_addr:0, lbr_to11_cycles:0, lbr_to11_set:0, lbr_to11_ema:0 });
pub fn init() { serial_println!("[msr_ia32_lbr_to_11] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx", lateout("eax") _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 19) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6CBu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_to11_addr = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let lbr_to11_cycles = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let lbr_to11_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (lbr_to11_addr as u32/3).saturating_add(lbr_to11_cycles as u32/3).saturating_add(lbr_to11_set as u32/3);
    let mut s = MODULE.lock();
    let lbr_to11_ema = ((s.lbr_to11_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_to11_addr=lbr_to11_addr; s.lbr_to11_cycles=lbr_to11_cycles; s.lbr_to11_set=lbr_to11_set; s.lbr_to11_ema=lbr_to11_ema;
    serial_println!("[msr_ia32_lbr_to_11] age={} addr={} cycles={} set={} ema={}", age, lbr_to11_addr, lbr_to11_cycles, lbr_to11_set, lbr_to11_ema);
}
pub fn get_lbr_to11_addr()   -> u16 { MODULE.lock().lbr_to11_addr }
pub fn get_lbr_to11_cycles() -> u16 { MODULE.lock().lbr_to11_cycles }
pub fn get_lbr_to11_set()    -> u16 { MODULE.lock().lbr_to11_set }
pub fn get_lbr_to11_ema()    -> u16 { MODULE.lock().lbr_to11_ema }
