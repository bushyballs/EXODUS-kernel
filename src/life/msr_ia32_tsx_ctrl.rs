#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtm_disable: u16, tsx_force_abort: u16, tsx_active: u16, tsx_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtm_disable: 0, tsx_force_abort: 0, tsx_active: 0, tsx_ema: 0 });

fn has_rtm() -> bool {
    let ebx: u32;
    unsafe {
        asm!("push rbx","cpuid","mov {0:e}, ebx","pop rbx",
             out(reg) ebx, inout("eax") 7u32 => _, inout("ecx") 0u32 => _, lateout("edx") _,
             options(nostack,nomem));
    }
    (ebx >> 11) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_tsx_ctrl] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_rtm() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x122u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let rtm_disable: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let tsx_force_abort: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let tsx_active: u16 = if lo & 3 == 0 { 1000 } else { 0 };
    let composite: u16 = (rtm_disable/4).saturating_add(tsx_force_abort/4).saturating_add(tsx_active/2);
    let mut s = MODULE.lock();
    let ema = ((s.tsx_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.rtm_disable = rtm_disable; s.tsx_force_abort = tsx_force_abort; s.tsx_active = tsx_active; s.tsx_ema = ema;
    serial_println!("[msr_ia32_tsx_ctrl] age={} lo={:#010x} rtm_dis={} force_abort={} active={} ema={}", age, lo, rtm_disable, tsx_force_abort, tsx_active, ema);
}
pub fn get_rtm_disable() -> u16 { MODULE.lock().rtm_disable }
pub fn get_tsx_force_abort() -> u16 { MODULE.lock().tsx_force_abort }
pub fn get_tsx_active() -> u16 { MODULE.lock().tsx_active }
pub fn get_tsx_ema() -> u16 { MODULE.lock().tsx_ema }
