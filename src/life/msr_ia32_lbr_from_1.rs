#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_from1_addr: u16, lbr_from1_kern: u16, lbr_from1_mispred: u16, lbr_from1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_from1_addr:0, lbr_from1_kern:0, lbr_from1_mispred:0, lbr_from1_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lbr_from_1] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    // Check LBR support
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx",
            lateout("eax") _, lateout("ecx") _, lateout("edx") edx,
            options(nostack, nomem),
        );
    }
    if (edx >> 19) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x681u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // bit 62 (hi bit 30): mispredicted branch
    let lbr_from1_mispred: u16 = if (hi >> 30) & 1 != 0 { 1000 } else { 0 };
    // bit 63 (hi bit 31): IN_TX (within TSX transaction)
    let lbr_from1_kern: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let lbr_from1_addr = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (lbr_from1_addr as u32/3).saturating_add(lbr_from1_kern as u32/3).saturating_add(lbr_from1_mispred as u32/3);
    let mut s = MODULE.lock();
    let lbr_from1_ema = ((s.lbr_from1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_from1_addr=lbr_from1_addr; s.lbr_from1_kern=lbr_from1_kern; s.lbr_from1_mispred=lbr_from1_mispred; s.lbr_from1_ema=lbr_from1_ema;
    serial_println!("[msr_ia32_lbr_from_1] age={} addr={} kern={} mispred={} ema={}", age, lbr_from1_addr, lbr_from1_kern, lbr_from1_mispred, lbr_from1_ema);
}
pub fn get_lbr_from1_addr()    -> u16 { MODULE.lock().lbr_from1_addr }
pub fn get_lbr_from1_kern()    -> u16 { MODULE.lock().lbr_from1_kern }
pub fn get_lbr_from1_mispred() -> u16 { MODULE.lock().lbr_from1_mispred }
pub fn get_lbr_from1_ema()     -> u16 { MODULE.lock().lbr_from1_ema }
