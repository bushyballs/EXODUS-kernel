#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ctr0_usr_en: u16,
    ctr0_os_en: u16,
    ctr1_usr_en: u16,
    fixed_ctrl_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ctr0_usr_en: 0,
    ctr0_os_en: 0,
    ctr1_usr_en: 0,
    fixed_ctrl_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_fixed_ctr_ctrl] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Du32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[1:0]: CTR0 (instr retired) OS/USR enable
    let ctr0_usr_en: u16 = if (lo & 2) != 0 { 1000 } else { 0 };
    let ctr0_os_en: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bits[5:4]: CTR1 (unhalted cycles) USR enable
    let ctr1_usr_en: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };

    let composite = (ctr0_usr_en as u32 / 3)
        .saturating_add(ctr0_os_en as u32 / 3)
        .saturating_add(ctr1_usr_en as u32 / 3);

    let mut s = MODULE.lock();
    let fixed_ctrl_ema = ((s.fixed_ctrl_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.ctr0_usr_en = ctr0_usr_en;
    s.ctr0_os_en = ctr0_os_en;
    s.ctr1_usr_en = ctr1_usr_en;
    s.fixed_ctrl_ema = fixed_ctrl_ema;

    serial_println!("[msr_ia32_fixed_ctr_ctrl] age={} c0u={} c0o={} c1u={} ema={}",
        age, ctr0_usr_en, ctr0_os_en, ctr1_usr_en, fixed_ctrl_ema);
}

pub fn get_ctr0_usr_en()     -> u16 { MODULE.lock().ctr0_usr_en }
pub fn get_ctr0_os_en()      -> u16 { MODULE.lock().ctr0_os_en }
pub fn get_ctr1_usr_en()     -> u16 { MODULE.lock().ctr1_usr_en }
pub fn get_fixed_ctrl_ema()  -> u16 { MODULE.lock().fixed_ctrl_ema }
