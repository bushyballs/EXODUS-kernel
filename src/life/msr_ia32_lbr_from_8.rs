#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_from8_addr: u16, lbr_from8_mispred: u16, lbr_from8_tsx: u16, lbr_from8_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_from8_addr:0, lbr_from8_mispred:0, lbr_from8_tsx:0, lbr_from8_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lbr_from_8] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx", lateout("eax") _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 19) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x688u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_from8_mispred: u16 = if (hi >> 30) & 1 != 0 { 1000 } else { 0 };
    let lbr_from8_tsx: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let lbr_from8_addr = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (lbr_from8_addr as u32/3).saturating_add(lbr_from8_mispred as u32/3).saturating_add(lbr_from8_tsx as u32/3);
    let mut s = MODULE.lock();
    let lbr_from8_ema = ((s.lbr_from8_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_from8_addr=lbr_from8_addr; s.lbr_from8_mispred=lbr_from8_mispred; s.lbr_from8_tsx=lbr_from8_tsx; s.lbr_from8_ema=lbr_from8_ema;
    serial_println!("[msr_ia32_lbr_from_8] age={} addr={} mispred={} tsx={} ema={}", age, lbr_from8_addr, lbr_from8_mispred, lbr_from8_tsx, lbr_from8_ema);
}
pub fn get_lbr_from8_addr()    -> u16 { MODULE.lock().lbr_from8_addr }
pub fn get_lbr_from8_mispred() -> u16 { MODULE.lock().lbr_from8_mispred }
pub fn get_lbr_from8_tsx()     -> u16 { MODULE.lock().lbr_from8_tsx }
pub fn get_lbr_from8_ema()     -> u16 { MODULE.lock().lbr_from8_ema }
