#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tsx_rtm_disabled: u16, tsx_cpuid_clear: u16, tsx_ctrl_ema: u16, tsx_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tsx_rtm_disabled:0, tsx_cpuid_clear:0, tsx_ctrl_ema:0, tsx_pad:0 });

#[inline]
fn has_tsx_ctrl() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("edx") edx,
            lateout("ecx") _,
            options(nostack, nomem),
        );
    }
    // CPUID 7.0 EDX bit 11: RTM_ALWAYS_ABORT implies TSX_CTRL present
    (edx >> 13) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_tsx_ctrl] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_tsx_ctrl() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x122u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: RTM_DISABLE — disable RTM transactional memory
    let tsx_rtm_disabled: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: TSX_CPUID_CLEAR — hide TSX from CPUID
    let tsx_cpuid_clear: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let composite = (tsx_rtm_disabled as u32/2).saturating_add(tsx_cpuid_clear as u32/2);
    let mut s = MODULE.lock();
    let tsx_ctrl_ema = ((s.tsx_ctrl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.tsx_rtm_disabled=tsx_rtm_disabled; s.tsx_cpuid_clear=tsx_cpuid_clear; s.tsx_ctrl_ema=tsx_ctrl_ema;
    serial_println!("[msr_ia32_tsx_ctrl] age={} rtm_dis={} cpuid_clr={} ema={}", age, tsx_rtm_disabled, tsx_cpuid_clear, tsx_ctrl_ema);
}
pub fn get_tsx_rtm_disabled() -> u16 { MODULE.lock().tsx_rtm_disabled }
pub fn get_tsx_cpuid_clear()  -> u16 { MODULE.lock().tsx_cpuid_clear }
pub fn get_tsx_ctrl_ema()     -> u16 { MODULE.lock().tsx_ctrl_ema }
