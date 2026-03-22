#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    lbr_tos: u16,
    lbr_tos_delta: u16,
    lbr_churn: u16,
    lbr_activity_ema: u16,
    last_tos: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    lbr_tos: 0,
    lbr_tos_delta: 0,
    lbr_churn: 0,
    lbr_activity_ema: 0,
    last_tos: 0,
});

pub fn init() { serial_println!("[msr_ia32_lbr_tos] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1C9u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let mut s = MODULE.lock();
    let prev = s.last_tos;

    let tos_raw = lo & 0x1F;
    let lbr_tos = ((tos_raw * 1000) / 31).min(1000) as u16;

    let delta = lo.wrapping_sub(prev) & 0x1F;
    let lbr_tos_delta = ((delta * 1000) / 31).min(1000) as u16;
    let lbr_churn = lbr_tos_delta;

    let lbr_activity_ema = ((s.lbr_activity_ema as u32).wrapping_mul(7)
        .saturating_add(lbr_churn as u32) / 8) as u16;

    s.last_tos = lo;
    s.lbr_tos = lbr_tos;
    s.lbr_tos_delta = lbr_tos_delta;
    s.lbr_churn = lbr_churn;
    s.lbr_activity_ema = lbr_activity_ema;

    serial_println!("[msr_ia32_lbr_tos] age={} tos={} delta={} churn={} ema={}",
        age, lbr_tos, lbr_tos_delta, lbr_churn, lbr_activity_ema);
}

pub fn get_lbr_tos()          -> u16 { MODULE.lock().lbr_tos }
pub fn get_lbr_tos_delta()    -> u16 { MODULE.lock().lbr_tos_delta }
pub fn get_lbr_churn()        -> u16 { MODULE.lock().lbr_churn }
pub fn get_lbr_activity_ema() -> u16 { MODULE.lock().lbr_activity_ema }
