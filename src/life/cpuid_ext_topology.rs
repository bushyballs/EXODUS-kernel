#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { smt_shift: u16, threads_per_core: u16, core_shift: u16, topo_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { smt_shift: 0, threads_per_core: 0, core_shift: 0, topo_ema: 0 });

pub fn init() { serial_println!("[cpuid_ext_topology] init"); }
pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x0B { return; }
    let eax_0b0: u32; let ebx_0b0: u32;
    unsafe {
        asm!("push rbx","cpuid","mov {1:e}, ebx","pop rbx",
             inout("eax") 0x0Bu32 => eax_0b0, out(reg) ebx_0b0,
             inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem));
    }
    let eax_0b1: u32;
    unsafe {
        asm!("push rbx","cpuid","pop rbx",
             inout("eax") 0x0Bu32 => eax_0b1, inout("ecx") 1u32 => _, lateout("edx") _, options(nostack,nomem));
    }
    let smt_val = (eax_0b0 & 0x1F) as u32;
    let threads_val = (ebx_0b0 & 0xFFFF) as u32;
    let core_val = (eax_0b1 & 0x1F) as u32;
    let smt_shift: u16 = ((smt_val * 1000) / 8).min(1000) as u16;
    let threads_per_core: u16 = ((threads_val * 1000) / 4).min(1000) as u16;
    let core_shift: u16 = ((core_val * 1000) / 16).min(1000) as u16;
    let composite: u16 = (smt_shift/4).saturating_add(threads_per_core/4).saturating_add(core_shift/2);
    let mut s = MODULE.lock();
    let ema = ((s.topo_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.smt_shift = smt_shift; s.threads_per_core = threads_per_core; s.core_shift = core_shift; s.topo_ema = ema;
    serial_println!("[cpuid_ext_topology] age={} smt_shift={} threads={} core_shift={} ema={}", age, smt_shift, threads_per_core, core_shift, ema);
}
pub fn get_smt_shift() -> u16 { MODULE.lock().smt_shift }
pub fn get_threads_per_core() -> u16 { MODULE.lock().threads_per_core }
pub fn get_core_shift() -> u16 { MODULE.lock().core_shift }
pub fn get_topo_ema() -> u16 { MODULE.lock().topo_ema }
