#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

pub struct XssState {
    pub pt_state_save: u16,
    pub cet_active: u16,
    pub extended_states: u16,
    pub supervisor_depth: u16,
}

impl XssState {
    pub const fn new() -> Self {
        Self {
            pt_state_save: 0,
            cet_active: 0,
            extended_states: 0,
            supervisor_depth: 0,
        }
    }
}

pub static MSR_XSS: Mutex<XssState> = Mutex::new(XssState::new());

pub fn init() {
    serial_println!("msr_xss: init");
}

pub fn tick(age: u32) {
    if age % 250 != 0 {
        return;
    }

    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xDA0u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    let pt_state_save: u16 = if lo & (1 << 8) != 0 { 1000u16 } else { 0u16 };
    let cet_active: u16 = if lo & ((1 << 11) | (1 << 12)) != 0 { 1000u16 } else { 0u16 };
    let extended_states: u16 = (lo.count_ones() as u16).wrapping_mul(71).min(1000);

    let mut state = MSR_XSS.lock();

    let supervisor_depth: u16 = (state.supervisor_depth * 7 + extended_states) / 8;

    state.pt_state_save = pt_state_save;
    state.cet_active = cet_active;
    state.extended_states = extended_states;
    state.supervisor_depth = supervisor_depth;

    serial_println!(
        "msr_xss | pt_save:{} cet:{} states:{} depth:{}",
        state.pt_state_save,
        state.cet_active,
        state.extended_states,
        state.supervisor_depth
    );
}
