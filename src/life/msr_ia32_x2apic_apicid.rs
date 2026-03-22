#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apic_id: u16, apic_id_valid: u16, apic_cluster: u16, apicid_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apic_id:0, apic_id_valid:0, apic_cluster:0, apicid_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_apicid] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x802u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let apic_id_valid: u16 = if lo != 0 { 1000 } else { 0 };
    let apic_id = (lo & 0xFF) as u16;
    let scaled_id = ((apic_id as u32 * 1000) / 255).min(1000) as u16;
    let apic_cluster = (((lo >> 4) & 0xF) * 66).min(1000) as u16;
    let composite = (apic_id_valid as u32/3).saturating_add(scaled_id as u32/3).saturating_add(apic_cluster as u32/3);
    let mut s = MODULE.lock();
    let apicid_ema = ((s.apicid_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apic_id=scaled_id; s.apic_id_valid=apic_id_valid; s.apic_cluster=apic_cluster; s.apicid_ema=apicid_ema;
    serial_println!("[msr_ia32_x2apic_apicid] age={} id={} valid={} cluster={} ema={}", age, scaled_id, apic_id_valid, apic_cluster, apicid_ema);
}
pub fn get_apic_id()       -> u16 { MODULE.lock().apic_id }
pub fn get_apic_id_valid() -> u16 { MODULE.lock().apic_id_valid }
pub fn get_apic_cluster()  -> u16 { MODULE.lock().apic_cluster }
pub fn get_apicid_ema()    -> u16 { MODULE.lock().apicid_ema }
