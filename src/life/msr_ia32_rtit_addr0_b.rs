#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtit_addr0_b_lo: u16, rtit_addr0_b_hi: u16, rtit_end_set: u16, rtit_end_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtit_addr0_b_lo:0, rtit_addr0_b_hi:0, rtit_end_set:0, rtit_end_ema:0 });

#[inline]
fn has_rtit() -> bool {
    let ebx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            options(nostack, nomem),
        );
    }
    (ebx >> 25) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_rtit_addr0_b] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_rtit() { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x581u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let rtit_addr0_b_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let rtit_addr0_b_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // End address set (nonzero = filter range fully defined)
    let rtit_end_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (rtit_addr0_b_lo as u32/3).saturating_add(rtit_addr0_b_hi as u32/3).saturating_add(rtit_end_set as u32/3);
    let mut s = MODULE.lock();
    let rtit_end_ema = ((s.rtit_end_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.rtit_addr0_b_lo=rtit_addr0_b_lo; s.rtit_addr0_b_hi=rtit_addr0_b_hi; s.rtit_end_set=rtit_end_set; s.rtit_end_ema=rtit_end_ema;
    serial_println!("[msr_ia32_rtit_addr0_b] age={} lo={} hi={} end={} ema={}", age, rtit_addr0_b_lo, rtit_addr0_b_hi, rtit_end_set, rtit_end_ema);
}
pub fn get_rtit_addr0_b_lo() -> u16 { MODULE.lock().rtit_addr0_b_lo }
pub fn get_rtit_addr0_b_hi() -> u16 { MODULE.lock().rtit_addr0_b_hi }
pub fn get_rtit_end_set()    -> u16 { MODULE.lock().rtit_end_set }
pub fn get_rtit_end_ema()    -> u16 { MODULE.lock().rtit_end_ema }
