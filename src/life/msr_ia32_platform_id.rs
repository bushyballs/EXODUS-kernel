#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { platform_id: u16, fid_program: u16, platform_locked: u16, platform_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { platform_id:0, fid_program:0, platform_locked:0, platform_ema:0 });

pub fn init() { serial_println!("[msr_ia32_platform_id] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x17u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let plat_id_raw = (hi >> 18) & 0x7;
    let platform_id = ((plat_id_raw * 142).min(1000)) as u16;
    let fid_raw = lo & 0x3F;
    let fid_program = ((fid_raw * 15).min(1000)) as u16;
    let platform_locked: u16 = if (lo >> 27) & 1 != 0 { 1000 } else { 0 };
    let composite = (platform_id as u32/3).saturating_add(fid_program as u32/3).saturating_add(platform_locked as u32/3);
    let mut s = MODULE.lock();
    let platform_ema = ((s.platform_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.platform_id=platform_id; s.fid_program=fid_program; s.platform_locked=platform_locked; s.platform_ema=platform_ema;
    serial_println!("[msr_ia32_platform_id] age={} plat={} fid={} locked={} ema={}", age, platform_id, fid_program, platform_locked, platform_ema);
}
pub fn get_platform_id()     -> u16 { MODULE.lock().platform_id }
pub fn get_fid_program()     -> u16 { MODULE.lock().fid_program }
pub fn get_platform_locked() -> u16 { MODULE.lock().platform_locked }
pub fn get_platform_ema()    -> u16 { MODULE.lock().platform_ema }
