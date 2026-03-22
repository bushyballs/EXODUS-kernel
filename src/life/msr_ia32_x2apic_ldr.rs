#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ldr_set: u16, logical_id: u16, cluster_id: u16, ldr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ldr_set:0, logical_id:0, cluster_id:0, ldr_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_ldr] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x80Du32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let ldr_set: u16 = if lo != 0 { 1000 } else { 0 };
    let log_id_raw = lo & 0xFFFF;
    let logical_id = ((log_id_raw * 1000) / 0xFFFF).min(1000) as u16;
    let cluster_raw = (lo >> 16) & 0xFFFF;
    let cluster_id = ((cluster_raw * 1000) / 0xFFFF).min(1000) as u16;
    let composite = (ldr_set as u32/3).saturating_add(logical_id as u32/3).saturating_add(cluster_id as u32/3);
    let mut s = MODULE.lock();
    let ldr_ema = ((s.ldr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ldr_set=ldr_set; s.logical_id=logical_id; s.cluster_id=cluster_id; s.ldr_ema=ldr_ema;
    serial_println!("[msr_ia32_x2apic_ldr] age={} set={} logid={} cluster={} ema={}", age, ldr_set, logical_id, cluster_id, ldr_ema);
}
pub fn get_ldr_set()     -> u16 { MODULE.lock().ldr_set }
pub fn get_logical_id()  -> u16 { MODULE.lock().logical_id }
pub fn get_cluster_id()  -> u16 { MODULE.lock().cluster_id }
pub fn get_ldr_ema()     -> u16 { MODULE.lock().ldr_ema }
