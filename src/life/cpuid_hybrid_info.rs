#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { hybrid_cpu: u16, core_type: u16, native_model: u16, hybrid_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { hybrid_cpu: 0, core_type: 0, native_model: 0, hybrid_ema: 0 });

pub fn init() { serial_println!("[cpuid_hybrid_info] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x1A { return; }
    let edx_7: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 7u32 => _, inout("ecx") 0u32 => _, lateout("edx") edx_7, options(nostack,nomem)); }
    let is_hybrid = (edx_7 >> 15) & 1 == 1;
    let hybrid_cpu: u16 = if is_hybrid { 1000 } else { 0 };
    let (core_type, native_model): (u16, u16) = if is_hybrid {
        let eax_1a: u32;
        unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0x1Au32 => eax_1a, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
        let core_type_raw = (eax_1a >> 24) & 0xFF;
        let ct: u16 = match core_type_raw { 0x40 => 1000, 0x20 => 500, _ => 250 };
        let nm_raw = eax_1a & 0xFFFFFF;
        let nm: u16 = (((nm_raw & 0x3FF) as u32 * 977) / 0x3FF).min(1000) as u16;
        (ct, nm)
    } else { (0, 0) };
    let composite: u16 = (hybrid_cpu/4).saturating_add(core_type/4).saturating_add(native_model/2);
    let mut s = MODULE.lock();
    let ema = ((s.hybrid_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.hybrid_cpu = hybrid_cpu; s.core_type = core_type; s.native_model = native_model; s.hybrid_ema = ema;
    serial_println!("[cpuid_hybrid_info] age={} hybrid={} core_type={} model={} ema={}", age, hybrid_cpu, core_type, native_model, ema);
}
pub fn get_hybrid_cpu() -> u16 { MODULE.lock().hybrid_cpu }
pub fn get_core_type() -> u16 { MODULE.lock().core_type }
pub fn get_native_model() -> u16 { MODULE.lock().native_model }
pub fn get_hybrid_ema() -> u16 { MODULE.lock().hybrid_ema }
