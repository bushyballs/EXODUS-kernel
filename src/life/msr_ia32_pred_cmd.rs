#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pred_cmd_ibpb: u16, pred_cmd_sbpb: u16, pred_cmd_activity: u16, pred_cmd_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pred_cmd_ibpb:0, pred_cmd_sbpb:0, pred_cmd_activity:0, pred_cmd_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pred_cmd] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    // PRED_CMD is write-only — we infer activity by SPEC_CTRL state instead
    let spec_lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48u32, out("eax") spec_lo, out("edx") _, options(nostack, nomem)); }
    // IBPB is triggered when IBRS transitions; infer from IBRS presence
    let pred_cmd_ibpb: u16 = if (spec_lo & 1) != 0 { 1000 } else { 0 };
    // SBPB (Selective Branch Predictor Barrier) — present if SSBD active
    let pred_cmd_sbpb: u16 = if (spec_lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // Overall barrier activity level
    let pred_cmd_activity: u16 = if spec_lo != 0 { 1000 } else { 0 };
    let composite = (pred_cmd_ibpb as u32/3).saturating_add(pred_cmd_sbpb as u32/3).saturating_add(pred_cmd_activity as u32/3);
    let mut s = MODULE.lock();
    let pred_cmd_ema = ((s.pred_cmd_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pred_cmd_ibpb=pred_cmd_ibpb; s.pred_cmd_sbpb=pred_cmd_sbpb; s.pred_cmd_activity=pred_cmd_activity; s.pred_cmd_ema=pred_cmd_ema;
    serial_println!("[msr_ia32_pred_cmd] age={} ibpb={} sbpb={} active={} ema={}", age, pred_cmd_ibpb, pred_cmd_sbpb, pred_cmd_activity, pred_cmd_ema);
}
pub fn get_pred_cmd_ibpb()     -> u16 { MODULE.lock().pred_cmd_ibpb }
pub fn get_pred_cmd_sbpb()     -> u16 { MODULE.lock().pred_cmd_sbpb }
pub fn get_pred_cmd_activity() -> u16 { MODULE.lock().pred_cmd_activity }
pub fn get_pred_cmd_ema()      -> u16 { MODULE.lock().pred_cmd_ema }
