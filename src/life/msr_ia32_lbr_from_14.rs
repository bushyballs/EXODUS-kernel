#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_from14_addr: u16, lbr_from14_mispred: u16, lbr_from14_tsx: u16, lbr_from14_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_from14_addr:0, lbr_from14_mispred:0, lbr_from14_tsx:0, lbr_from14_ema:0 });
pub fn init() { serial_println!("[msr_ia32_lbr_from_14] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx", lateout("eax") _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 19) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x68Eu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_from14_mispred: u16 = if (hi >> 30) & 1 != 0 { 1000 } else { 0 };
    let lbr_from14_tsx: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let lbr_from14_addr = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (lbr_from14_addr as u32/3).saturating_add(lbr_from14_mispred as u32/3).saturating_add(lbr_from14_tsx as u32/3);
    let mut s = MODULE.lock();
    let lbr_from14_ema = ((s.lbr_from14_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_from14_addr=lbr_from14_addr; s.lbr_from14_mispred=lbr_from14_mispred; s.lbr_from14_tsx=lbr_from14_tsx; s.lbr_from14_ema=lbr_from14_ema;
    serial_println!("[msr_ia32_lbr_from_14] age={} addr={} mispred={} tsx={} ema={}", age, lbr_from14_addr, lbr_from14_mispred, lbr_from14_tsx, lbr_from14_ema);
}
pub fn get_lbr_from14_addr()    -> u16 { MODULE.lock().lbr_from14_addr }
pub fn get_lbr_from14_mispred() -> u16 { MODULE.lock().lbr_from14_mispred }
pub fn get_lbr_from14_tsx()     -> u16 { MODULE.lock().lbr_from14_tsx }
pub fn get_lbr_from14_ema()     -> u16 { MODULE.lock().lbr_from14_ema }
