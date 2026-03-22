#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { esr_errors: u16, esr_send_err: u16, esr_recv_err: u16, esr_pressure_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { esr_errors:0, esr_send_err:0, esr_recv_err:0, esr_pressure_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_esr] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x828u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let total_bits = popcount(lo & 0xFF);
    let esr_errors = ((total_bits * 125).min(1000)) as u16;
    let esr_send_err: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let esr_recv_err: u16 = if (lo >> 3) & 1 != 0 { 1000 } else { 0 };
    let composite = (esr_errors as u32/3).saturating_add(esr_send_err as u32/3).saturating_add(esr_recv_err as u32/3);
    let mut s = MODULE.lock();
    let esr_pressure_ema = ((s.esr_pressure_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.esr_errors=esr_errors; s.esr_send_err=esr_send_err; s.esr_recv_err=esr_recv_err; s.esr_pressure_ema=esr_pressure_ema;
    serial_println!("[msr_ia32_x2apic_esr] age={} errors={} send_err={} recv_err={} ema={}", age, esr_errors, esr_send_err, esr_recv_err, esr_pressure_ema);
}
pub fn get_esr_errors()        -> u16 { MODULE.lock().esr_errors }
pub fn get_esr_send_err()      -> u16 { MODULE.lock().esr_send_err }
pub fn get_esr_recv_err()      -> u16 { MODULE.lock().esr_recv_err }
pub fn get_esr_pressure_ema()  -> u16 { MODULE.lock().esr_pressure_ema }
