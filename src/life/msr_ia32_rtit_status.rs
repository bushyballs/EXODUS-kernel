#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { rtit_status_err: u16, rtit_status_stopped: u16, rtit_pt_write: u16, rtit_status_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { rtit_status_err:0, rtit_status_stopped:0, rtit_pt_write:0, rtit_status_ema:0 });

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

pub fn init() { serial_println!("[msr_ia32_rtit_status] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_rtit() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x571u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 4: Error — trace error occurred
    let rtit_status_err: u16 = if (lo >> 4) & 1 != 0 { 1000 } else { 0 };
    // bit 5: Stopped — output region full
    let rtit_status_stopped: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    // bit 16: PTW — PT WRITE executed during trace
    let rtit_pt_write: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let pressure = (rtit_status_err as u32/3).saturating_add(rtit_status_stopped as u32/3).saturating_add(rtit_pt_write as u32/3);
    let mut s = MODULE.lock();
    let rtit_status_ema = ((s.rtit_status_ema as u32).wrapping_mul(7).saturating_add(pressure)/8).min(1000) as u16;
    s.rtit_status_err=rtit_status_err; s.rtit_status_stopped=rtit_status_stopped; s.rtit_pt_write=rtit_pt_write; s.rtit_status_ema=rtit_status_ema;
    serial_println!("[msr_ia32_rtit_status] age={} err={} stop={} ptw={} ema={}", age, rtit_status_err, rtit_status_stopped, rtit_pt_write, rtit_status_ema);
}
pub fn get_rtit_status_err()     -> u16 { MODULE.lock().rtit_status_err }
pub fn get_rtit_status_stopped() -> u16 { MODULE.lock().rtit_status_stopped }
pub fn get_rtit_pt_write()       -> u16 { MODULE.lock().rtit_pt_write }
pub fn get_rtit_status_ema()     -> u16 { MODULE.lock().rtit_status_ema }
