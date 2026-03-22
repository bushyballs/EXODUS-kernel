#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_POWER_CTL: u32 = 0x1FC;

const BIT_C1E_ENABLE:        u64 = 1 << 0;
const BIT_PKG_CSTATE_DEMOTE: u64 = 1 << 2;
const BIT_PKG_CSTATE_UNDEM:  u64 = 1 << 3;
const BIT_EE_TURBO_DIS:      u64 = 1 << 10;
const BIT_C1_AUTO_DEMOTE:    u64 = 1 << 13;
const BIT_C1_AUTO_UNDEM:     u64 = 1 << 14;

const TICK_GATE: u32 = 6000;

pub struct State {
    c1e_enable:     u16,
    ee_turbo_dis:   u16,
    c_state_ctrl:   u16,
    power_ctl_ema:  u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    c1e_enable:    0,
    ee_turbo_dis:  0,
    c_state_ctrl:  0,
    power_ctl_ema: 0,
});

fn has_rapl() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 4) & 1 == 1
}

fn read_msr(addr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn sample(raw: u64) -> (u16, u16, u16) {
    let c1e_enable = if raw & BIT_C1E_ENABLE != 0 { 1000u16 } else { 0u16 };

    let ee_turbo_dis = if raw & BIT_EE_TURBO_DIS != 0 { 1000u16 } else { 0u16 };

    let mut c_state_count: u16 = 0;
    if raw & BIT_PKG_CSTATE_DEMOTE != 0 { c_state_count = c_state_count.saturating_add(250); }
    if raw & BIT_PKG_CSTATE_UNDEM  != 0 { c_state_count = c_state_count.saturating_add(250); }
    if raw & BIT_C1_AUTO_DEMOTE    != 0 { c_state_count = c_state_count.saturating_add(250); }
    if raw & BIT_C1_AUTO_UNDEM     != 0 { c_state_count = c_state_count.saturating_add(250); }

    (c1e_enable, ee_turbo_dis, c_state_count)
}

pub fn init() {
    serial_println!("[msr_ia32_power_ctl] init");
    let mut state = MODULE.lock();
    state.c1e_enable    = 0;
    state.ee_turbo_dis  = 0;
    state.c_state_ctrl  = 0;
    state.power_ctl_ema = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 { return; }
    if !has_rapl() { return; }

    let raw = read_msr(MSR_POWER_CTL);
    let (c1e, ee_turbo, c_state) = sample(raw);

    let mut state = MODULE.lock();
    state.c1e_enable   = c1e;
    state.ee_turbo_dis = ee_turbo;
    state.c_state_ctrl = c_state;
    state.power_ctl_ema = ema(state.power_ctl_ema, c_state);

    serial_println!(
        "[msr_ia32_power_ctl] age={} c1e={} ee_turbo_dis={} c_state_ctrl={} ema={}",
        age,
        state.c1e_enable,
        state.ee_turbo_dis,
        state.c_state_ctrl,
        state.power_ctl_ema,
    );
}

pub fn get_c1e_enable() -> u16 {
    MODULE.lock().c1e_enable
}

pub fn get_ee_turbo_dis() -> u16 {
    MODULE.lock().ee_turbo_dis
}

pub fn get_c_state_ctrl() -> u16 {
    MODULE.lock().c_state_ctrl
}

pub fn get_power_ctl_ema() -> u16 {
    MODULE.lock().power_ctl_ema
}
