#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { div_value: u16, div_is_1: u16, div_is_128: u16, div_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { div_value:0, div_is_1:0, div_is_128:0, div_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_timer_div] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x83Eu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let div_raw = ((lo >> 1) & 0x4) | (lo & 0x3);
    let actual_div: u32 = match div_raw { 0=>2, 1=>4, 2=>8, 3=>16, 4=>32, 5=>64, 6=>128, 7=>1, _=>1 };
    let div_value = ((1000u32).saturating_sub((actual_div.saturating_sub(1))*7)).min(1000) as u16;
    let div_is_1: u16 = if actual_div == 1 { 1000 } else { 0 };
    let div_is_128: u16 = if actual_div == 128 { 1000 } else { 0 };
    let composite = (div_value as u32/3).saturating_add(div_is_1 as u32/3).saturating_add(1000u32.saturating_sub(div_is_128 as u32)/3);
    let mut s = MODULE.lock();
    let div_ema = ((s.div_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.div_value=div_value; s.div_is_1=div_is_1; s.div_is_128=div_is_128; s.div_ema=div_ema;
    serial_println!("[msr_ia32_x2apic_timer_div] age={} div={} is1={} is128={} ema={}", age, div_value, div_is_1, div_is_128, div_ema);
}
pub fn get_div_value()  -> u16 { MODULE.lock().div_value }
pub fn get_div_is_1()   -> u16 { MODULE.lock().div_is_1 }
pub fn get_div_is_128() -> u16 { MODULE.lock().div_is_128 }
pub fn get_div_ema()    -> u16 { MODULE.lock().div_ema }
