#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { spec_ctrl_ibrs: u16, spec_ctrl_stibp: u16, spec_ctrl_ssbd: u16, spec_ctrl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { spec_ctrl_ibrs:0, spec_ctrl_stibp:0, spec_ctrl_ssbd:0, spec_ctrl_ema:0 });

pub fn init() { serial_println!("[msr_ia32_spec_ctrl] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: IBRS — Indirect Branch Restricted Speculation
    let spec_ctrl_ibrs: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: STIBP — Single Thread Indirect Branch Predictors
    let spec_ctrl_stibp: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 2: SSBD — Speculative Store Bypass Disable
    let spec_ctrl_ssbd: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // Security overhead: more mitigations = higher security load
    let composite = (spec_ctrl_ibrs as u32/3).saturating_add(spec_ctrl_stibp as u32/3).saturating_add(spec_ctrl_ssbd as u32/3);
    let mut s = MODULE.lock();
    let spec_ctrl_ema = ((s.spec_ctrl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.spec_ctrl_ibrs=spec_ctrl_ibrs; s.spec_ctrl_stibp=spec_ctrl_stibp; s.spec_ctrl_ssbd=spec_ctrl_ssbd; s.spec_ctrl_ema=spec_ctrl_ema;
    serial_println!("[msr_ia32_spec_ctrl] age={} ibrs={} stibp={} ssbd={} ema={}", age, spec_ctrl_ibrs, spec_ctrl_stibp, spec_ctrl_ssbd, spec_ctrl_ema);
}
pub fn get_spec_ctrl_ibrs()  -> u16 { MODULE.lock().spec_ctrl_ibrs }
pub fn get_spec_ctrl_stibp() -> u16 { MODULE.lock().spec_ctrl_stibp }
pub fn get_spec_ctrl_ssbd()  -> u16 { MODULE.lock().spec_ctrl_ssbd }
pub fn get_spec_ctrl_ema()   -> u16 { MODULE.lock().spec_ctrl_ema }
