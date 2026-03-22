#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtit_addr_lo: u16, rtit_addr_hi: u16, rtit_filter_span: u16, rtit_addr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtit_addr_lo:0, rtit_addr_hi:0, rtit_filter_span:0, rtit_addr_ema:0 });

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
    // CPUID 7.0 EBX bit 25: Intel Processor Trace
    (ebx >> 25) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_rtit_addr0_a] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_rtit() { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x580u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // Lower 16 bits of VA (address filter range start)
    let rtit_addr_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    // Upper bits from hi (canonical VA bits[47:32])
    let rtit_addr_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Filter span: nonzero address = active filter configured
    let rtit_filter_span: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (rtit_addr_lo as u32/3).saturating_add(rtit_addr_hi as u32/3).saturating_add(rtit_filter_span as u32/3);
    let mut s = MODULE.lock();
    let rtit_addr_ema = ((s.rtit_addr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.rtit_addr_lo=rtit_addr_lo; s.rtit_addr_hi=rtit_addr_hi; s.rtit_filter_span=rtit_filter_span; s.rtit_addr_ema=rtit_addr_ema;
    serial_println!("[msr_ia32_rtit_addr0_a] age={} lo={} hi={} span={} ema={}", age, rtit_addr_lo, rtit_addr_hi, rtit_filter_span, rtit_addr_ema);
}
pub fn get_rtit_addr_lo()     -> u16 { MODULE.lock().rtit_addr_lo }
pub fn get_rtit_addr_hi()     -> u16 { MODULE.lock().rtit_addr_hi }
pub fn get_rtit_filter_span() -> u16 { MODULE.lock().rtit_filter_span }
pub fn get_rtit_addr_ema()    -> u16 { MODULE.lock().rtit_addr_ema }
