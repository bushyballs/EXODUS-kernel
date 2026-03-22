#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { xcomp_bv_lo: u16, xcomp_bv_hi: u16, xstate_compact: u16, xcomp_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { xcomp_bv_lo:0, xcomp_bv_hi:0, xstate_compact:0, xcomp_ema:0 });

pub fn init() { serial_println!("[msr_ia32_xcomp_bv] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // Check XSAVES/XRSTORS support (CPUID 0D.1 EAX bit 3)
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0xDu32 => eax,
            in("ecx") 1u32,
            lateout("edx") _,
            lateout("ecx") _,
            options(nostack, nomem),
        );
    }
    if (eax >> 3) & 1 == 0 { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xDA1u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // XCOMP_BV: compact-format component bitmap (lower 16 bits sense)
    let xcomp_bv_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let xcomp_bv_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // bit 63 (hi bit 31): compact format in use
    let xstate_compact: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let composite = (xcomp_bv_lo as u32/3).saturating_add(xcomp_bv_hi as u32/3).saturating_add(xstate_compact as u32/3);
    let mut s = MODULE.lock();
    let xcomp_ema = ((s.xcomp_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.xcomp_bv_lo=xcomp_bv_lo; s.xcomp_bv_hi=xcomp_bv_hi; s.xstate_compact=xstate_compact; s.xcomp_ema=xcomp_ema;
    serial_println!("[msr_ia32_xcomp_bv] age={} lo={} hi={} compact={} ema={}", age, xcomp_bv_lo, xcomp_bv_hi, xstate_compact, xcomp_ema);
}
pub fn get_xcomp_bv_lo()   -> u16 { MODULE.lock().xcomp_bv_lo }
pub fn get_xcomp_bv_hi()   -> u16 { MODULE.lock().xcomp_bv_hi }
pub fn get_xstate_compact() -> u16 { MODULE.lock().xstate_compact }
pub fn get_xcomp_ema()     -> u16 { MODULE.lock().xcomp_ema }
