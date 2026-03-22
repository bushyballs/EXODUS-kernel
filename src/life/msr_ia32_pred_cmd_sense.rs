#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ibpb_ready: u16,
    ibrs_active: u16,
    predictor_isolation: u16,
    isolation_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ibpb_ready: 0,
    ibrs_active: 0,
    predictor_isolation: 0,
    isolation_ema: 0,
});

#[inline]
fn has_ibpb() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx,
            options(nostack, nomem),
        );
    }
    (edx >> 26) & 1 == 1
}

#[inline]
fn read_spec_ctrl_lo() -> u32 {
    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x48u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    lo
}

pub fn init() { serial_println!("[msr_ia32_pred_cmd_sense] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let ibpb_ready: u16 = if has_ibpb() { 1000 } else { 0 };
    let lo = read_spec_ctrl_lo();
    let ibrs_active: u16 = if (lo & 1) != 0 { 1000 } else { 0 };

    let predictor_isolation = ((ibpb_ready as u32 + ibrs_active as u32) / 2) as u16;

    let mut s = MODULE.lock();
    let isolation_ema = ((s.isolation_ema as u32).wrapping_mul(7)
        .saturating_add(predictor_isolation as u32) / 8) as u16;

    s.ibpb_ready = ibpb_ready;
    s.ibrs_active = ibrs_active;
    s.predictor_isolation = predictor_isolation;
    s.isolation_ema = isolation_ema;

    serial_println!("[msr_ia32_pred_cmd_sense] age={} ibpb={} ibrs={} isolation={} ema={}",
        age, ibpb_ready, ibrs_active, predictor_isolation, isolation_ema);
}

pub fn get_ibpb_ready() -> u16 { MODULE.lock().ibpb_ready }
pub fn get_ibrs_active() -> u16 { MODULE.lock().ibrs_active }
pub fn get_predictor_isolation() -> u16 { MODULE.lock().predictor_isolation }
pub fn get_isolation_ema() -> u16 { MODULE.lock().isolation_ema }
