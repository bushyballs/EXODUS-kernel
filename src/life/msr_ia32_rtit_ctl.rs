#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtit_ctl_trace: u16, rtit_ctl_os: u16, rtit_ctl_usr: u16, rtit_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtit_ctl_trace:0, rtit_ctl_os:0, rtit_ctl_usr:0, rtit_ctl_ema:0 });

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

pub fn init() { serial_println!("[msr_ia32_rtit_ctl] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_rtit() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x570u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: TraceEn — trace currently active
    let rtit_ctl_trace: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 2: OS — trace kernel mode
    let rtit_ctl_os: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // bit 3: User — trace user mode
    let rtit_ctl_usr: u16 = if (lo >> 3) & 1 != 0 { 1000 } else { 0 };
    let composite = (rtit_ctl_trace as u32/3).saturating_add(rtit_ctl_os as u32/3).saturating_add(rtit_ctl_usr as u32/3);
    let mut s = MODULE.lock();
    let rtit_ctl_ema = ((s.rtit_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.rtit_ctl_trace=rtit_ctl_trace; s.rtit_ctl_os=rtit_ctl_os; s.rtit_ctl_usr=rtit_ctl_usr; s.rtit_ctl_ema=rtit_ctl_ema;
    serial_println!("[msr_ia32_rtit_ctl] age={} trace={} os={} usr={} ema={}", age, rtit_ctl_trace, rtit_ctl_os, rtit_ctl_usr, rtit_ctl_ema);
}
pub fn get_rtit_ctl_trace() -> u16 { MODULE.lock().rtit_ctl_trace }
pub fn get_rtit_ctl_os()    -> u16 { MODULE.lock().rtit_ctl_os }
pub fn get_rtit_ctl_usr()   -> u16 { MODULE.lock().rtit_ctl_usr }
pub fn get_rtit_ctl_ema()   -> u16 { MODULE.lock().rtit_ctl_ema }
