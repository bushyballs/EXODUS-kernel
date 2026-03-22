#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { core_thread_count: u16, thread_count: u16, core_count: u16, topology_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { core_thread_count:0, thread_count:0, core_count:0, topology_ema:0 });

pub fn init() { serial_println!("[msr_ia32_core_thread_count] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x35u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[15:0]: thread count in package
    let thread_count = ((lo & 0xFFFF) * 1000 / 256).min(1000) as u16;
    // bits[31:16]: core count in package
    let core_count = (((lo >> 16) & 0xFFFF) * 1000 / 256).min(1000) as u16;
    // Combined topology sense
    let core_thread_count = ((thread_count as u32 + core_count as u32) / 2) as u16;
    let composite = (core_thread_count as u32/3).saturating_add(thread_count as u32/3).saturating_add(core_count as u32/3);
    let mut s = MODULE.lock();
    let topology_ema = ((s.topology_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.core_thread_count=core_thread_count; s.thread_count=thread_count; s.core_count=core_count; s.topology_ema=topology_ema;
    serial_println!("[msr_ia32_core_thread_count] age={} threads={} cores={} topo={} ema={}", age, thread_count, core_count, core_thread_count, topology_ema);
}
pub fn get_core_thread_count() -> u16 { MODULE.lock().core_thread_count }
pub fn get_thread_count()      -> u16 { MODULE.lock().thread_count }
pub fn get_core_count()        -> u16 { MODULE.lock().core_count }
pub fn get_topology_ema()      -> u16 { MODULE.lock().topology_ema }
