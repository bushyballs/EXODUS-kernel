#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    dual_monitor_valid: u16,
    smm_vmx_off: u16,
    smm_monitor_active: u16,
    smm_ctl_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    dual_monitor_valid: 0,
    smm_vmx_off: 0,
    smm_monitor_active: 0,
    smm_ctl_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_smm_monitor_ctl] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x9Bu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: dual-monitor treatment of SMM and VMX enabled
    let dual_monitor_valid: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 2: SMM_VMX_OFF (block VMX while in SMM)
    let smm_vmx_off: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // any monitoring active
    let smm_monitor_active: u16 = if lo & 5 != 0 { 1000 } else { 0 };

    let composite = (dual_monitor_valid as u32 / 3)
        .saturating_add(smm_vmx_off as u32 / 3)
        .saturating_add(smm_monitor_active as u32 / 3);

    let mut s = MODULE.lock();
    let smm_ctl_ema = ((s.smm_ctl_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.dual_monitor_valid = dual_monitor_valid;
    s.smm_vmx_off = smm_vmx_off;
    s.smm_monitor_active = smm_monitor_active;
    s.smm_ctl_ema = smm_ctl_ema;

    serial_println!("[msr_ia32_smm_monitor_ctl] age={} dual={} vmx_off={} active={} ema={}",
        age, dual_monitor_valid, smm_vmx_off, smm_monitor_active, smm_ctl_ema);
}

pub fn get_dual_monitor_valid()  -> u16 { MODULE.lock().dual_monitor_valid }
pub fn get_smm_vmx_off()         -> u16 { MODULE.lock().smm_vmx_off }
pub fn get_smm_monitor_active()  -> u16 { MODULE.lock().smm_monitor_active }
pub fn get_smm_ctl_ema()         -> u16 { MODULE.lock().smm_ctl_ema }
