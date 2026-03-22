#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { bts_buf_lo: u16, bts_buf_hi: u16, bts_configured: u16, bts_buf_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { bts_buf_lo:0, bts_buf_hi:0, bts_configured:0, bts_buf_ema:0 });

pub fn init() { serial_println!("[msr_ia32_bts_buffer_base] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 21) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3F1u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // DS area base for BTS — encoded in DS_AREA MSR indirectly
    // Sense DS save area presence via DEBUGCTL BTS bit instead
    let debugctl: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x1D9u32, out("eax") debugctl, out("edx") _, options(nostack, nomem)); }
    let bts_buf_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let bts_buf_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // bit 7 of DEBUGCTL: BTS active
    let bts_configured: u16 = if (debugctl >> 7) & 1 != 0 { 1000 } else { 0 };
    let composite = (bts_buf_lo as u32/3).saturating_add(bts_buf_hi as u32/3).saturating_add(bts_configured as u32/3);
    let mut s = MODULE.lock();
    let bts_buf_ema = ((s.bts_buf_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.bts_buf_lo=bts_buf_lo; s.bts_buf_hi=bts_buf_hi; s.bts_configured=bts_configured; s.bts_buf_ema=bts_buf_ema;
    serial_println!("[msr_ia32_bts_buffer_base] age={} lo={} hi={} cfg={} ema={}", age, bts_buf_lo, bts_buf_hi, bts_configured, bts_buf_ema);
}
pub fn get_bts_buf_lo()     -> u16 { MODULE.lock().bts_buf_lo }
pub fn get_bts_buf_hi()     -> u16 { MODULE.lock().bts_buf_hi }
pub fn get_bts_configured() -> u16 { MODULE.lock().bts_configured }
pub fn get_bts_buf_ema()    -> u16 { MODULE.lock().bts_buf_ema }
