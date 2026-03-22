#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pebs_ld_lat_thresh: u16, pebs_ld_lat_en: u16, pebs_ld_lat_ema: u16, pebs_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pebs_ld_lat_thresh:0, pebs_ld_lat_en:0, pebs_ld_lat_ema:0, pebs_pad:0 });

pub fn init() { serial_println!("[msr_ia32_pebs_ld_lat] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 21) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3F6u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[15:0]: load latency threshold in cycles
    let raw_thresh = lo & 0xFFFF;
    let pebs_ld_lat_thresh = ((raw_thresh * 1000) / 65535) as u16;
    // Enabled if nonzero threshold
    let pebs_ld_lat_en: u16 = if raw_thresh > 0 { 1000 } else { 0 };
    let composite = (pebs_ld_lat_thresh as u32/2).saturating_add(pebs_ld_lat_en as u32/2);
    let mut s = MODULE.lock();
    let pebs_ld_lat_ema = ((s.pebs_ld_lat_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pebs_ld_lat_thresh=pebs_ld_lat_thresh; s.pebs_ld_lat_en=pebs_ld_lat_en; s.pebs_ld_lat_ema=pebs_ld_lat_ema;
    serial_println!("[msr_ia32_pebs_ld_lat] age={} thresh={} en={} ema={}", age, pebs_ld_lat_thresh, pebs_ld_lat_en, pebs_ld_lat_ema);
}
pub fn get_pebs_ld_lat_thresh() -> u16 { MODULE.lock().pebs_ld_lat_thresh }
pub fn get_pebs_ld_lat_en()     -> u16 { MODULE.lock().pebs_ld_lat_en }
pub fn get_pebs_ld_lat_ema()    -> u16 { MODULE.lock().pebs_ld_lat_ema }
