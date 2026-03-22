#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtit_cr3_lo: u16, rtit_cr3_hi: u16, rtit_filter_active: u16, rtit_cr3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtit_cr3_lo:0, rtit_cr3_hi:0, rtit_filter_active:0, rtit_cr3_ema:0 });

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

pub fn init() { serial_println!("[msr_ia32_rtit_cr3_match] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_rtit() { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x572u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // CR3 bits[63:5] — process-specific trace filter
    let rtit_cr3_lo = ((lo >> 5) & 0x7FF) as u16 * 1000 / 2047;
    let rtit_cr3_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Nonzero CR3 = process filter active
    let rtit_filter_active: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (rtit_cr3_lo as u32/3).saturating_add(rtit_cr3_hi as u32/3).saturating_add(rtit_filter_active as u32/3);
    let mut s = MODULE.lock();
    let rtit_cr3_ema = ((s.rtit_cr3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.rtit_cr3_lo=rtit_cr3_lo; s.rtit_cr3_hi=rtit_cr3_hi; s.rtit_filter_active=rtit_filter_active; s.rtit_cr3_ema=rtit_cr3_ema;
    serial_println!("[msr_ia32_rtit_cr3_match] age={} lo={} hi={} active={} ema={}", age, rtit_cr3_lo, rtit_cr3_hi, rtit_filter_active, rtit_cr3_ema);
}
pub fn get_rtit_cr3_lo()       -> u16 { MODULE.lock().rtit_cr3_lo }
pub fn get_rtit_cr3_hi()       -> u16 { MODULE.lock().rtit_cr3_hi }
pub fn get_rtit_filter_active() -> u16 { MODULE.lock().rtit_filter_active }
pub fn get_rtit_cr3_ema()      -> u16 { MODULE.lock().rtit_cr3_ema }
