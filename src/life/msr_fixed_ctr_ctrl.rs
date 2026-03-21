#![allow(dead_code)]

use crate::sync::Mutex;

pub struct FixedCtrCtrlState {
    pub ctr0_active: u16,
    pub ctr1_active: u16,
    pub ctr2_active: u16,
    pub attention_config: u16,
}

impl FixedCtrCtrlState {
    pub const fn new() -> Self {
        Self {
            ctr0_active: 500,
            ctr1_active: 500,
            ctr2_active: 500,
            attention_config: 500,
        }
    }
}

pub static MSR_FIXED_CTR_CTRL: Mutex<FixedCtrCtrlState> = Mutex::new(FixedCtrCtrlState::new());

pub fn init() {
    serial_println!("fixed_ctr_ctrl: init");
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
            in("ecx") 0x38Du32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    let ctr0_active: u16 = if (lo & 0x3) != 0 { 1000u16 } else { 0u16 };
    let ctr1_active: u16 = if (lo & 0x30) != 0 { 1000u16 } else { 0u16 };
    let ctr2_active: u16 = if (lo & 0x300) != 0 { 1000u16 } else { 0u16 };

    let attention_avg: u16 =
        ((ctr0_active as u32 + ctr1_active as u32 + ctr2_active as u32) / 3) as u16;

    let mut state = MSR_FIXED_CTR_CTRL.lock();

    let new_attention_config: u16 =
        ((state.attention_config as u32 * 7 + attention_avg as u32) / 8) as u16;

    state.ctr0_active = ctr0_active;
    state.ctr1_active = ctr1_active;
    state.ctr2_active = ctr2_active;
    state.attention_config = new_attention_config;

    serial_println!(
        "fixed_ctr_ctrl | ctr0:{} ctr1:{} ctr2:{} attention:{}",
        state.ctr0_active,
        state.ctr1_active,
        state.ctr2_active,
        state.attention_config
    );
}
