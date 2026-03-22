#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc_enabled: u16, fixed_enabled: u16, any_active: u16, global_ctrl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pmc_enabled: 0, fixed_enabled: 0, any_active: 0, global_ctrl_ema: 0 });

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0{c+=v&1;v>>=1;} c }
fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 15) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_perf_global_ctrl] init"); }
pub fn tick(age: u32) {
    if age % 1500 != 0 { return; }
    if !has_pdcm() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x38Fu32, out("eax") lo, out("edx") hi, options(nostack,nomem)); }
    let pmc_enabled: u16 = (popcount(lo & 0xF) * 250).min(1000) as u16;
    let fixed_enabled: u16 = (popcount(hi & 0x7) * 333).min(1000) as u16;
    let any_active: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite: u16 = (pmc_enabled/4).saturating_add(fixed_enabled/4).saturating_add(any_active/2);
    let mut s = MODULE.lock();
    let ema = ((s.global_ctrl_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.pmc_enabled = pmc_enabled; s.fixed_enabled = fixed_enabled; s.any_active = any_active; s.global_ctrl_ema = ema;
    serial_println!("[msr_ia32_perf_global_ctrl] age={} lo={:#010x} hi={:#010x} pmc={} fixed={} active={} ema={}", age, lo, hi, pmc_enabled, fixed_enabled, any_active, ema);
}
pub fn get_pmc_enabled() -> u16 { MODULE.lock().pmc_enabled }
pub fn get_fixed_enabled() -> u16 { MODULE.lock().fixed_enabled }
pub fn get_any_active() -> u16 { MODULE.lock().any_active }
pub fn get_global_ctrl_ema() -> u16 { MODULE.lock().global_ctrl_ema }
