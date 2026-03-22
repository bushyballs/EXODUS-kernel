#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { umwait_max_time_lo: u16, umwait_max_time_hi: u16, umwait_c02_dis: u16, umwait_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { umwait_max_time_lo:0, umwait_max_time_hi:0, umwait_c02_dis:0, umwait_ema:0 });

pub fn init() { serial_println!("[msr_ia32_umwait_control] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    // Check WAITPKG support (CPUID 7.0 ECX bit 5)
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx", "mov eax, 7", "xor ecx, ecx", "cpuid", "pop rbx",
            lateout("eax") _, lateout("ecx") ecx, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if (ecx >> 5) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xE1u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: C0.2 disable — prefer C0.1 over C0.2 wait state
    let umwait_c02_dis: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bits[31:2]: max time value for UMWAIT/TPAUSE (in TSC units)
    let raw_max = (lo >> 2) & 0x3FFF;
    let umwait_max_time_lo = ((raw_max * 1000) / 16383) as u16;
    let umwait_max_time_hi = 0u16; // upper word always 0
    let composite = (umwait_max_time_lo as u32/2).saturating_add(umwait_c02_dis as u32/2);
    let mut s = MODULE.lock();
    let umwait_ema = ((s.umwait_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.umwait_max_time_lo=umwait_max_time_lo; s.umwait_max_time_hi=umwait_max_time_hi; s.umwait_c02_dis=umwait_c02_dis; s.umwait_ema=umwait_ema;
    serial_println!("[msr_ia32_umwait_control] age={} max_lo={} c02_dis={} ema={}", age, umwait_max_time_lo, umwait_c02_dis, umwait_ema);
}
pub fn get_umwait_max_time_lo() -> u16 { MODULE.lock().umwait_max_time_lo }
pub fn get_umwait_c02_dis()     -> u16 { MODULE.lock().umwait_c02_dis }
pub fn get_umwait_ema()         -> u16 { MODULE.lock().umwait_ema }
