#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    tpr_priority: u16,
    tpr_subclass: u16,
    tpr_active: u16,
    msr_ia32_x2apic_tpr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    tpr_priority: 0,
    tpr_subclass: 0,
    tpr_active: 0,
    msr_ia32_x2apic_tpr_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_x2apic_tpr] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x808u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // x2APIC task priority register
    let tpr_priority = (((lo >> 4) & 0xF) * 1000 / 15).min(1000) as u16;
    let tpr_subclass = ((lo & 0xF) * 1000 / 15).min(1000) as u16;
    let tpr_active: u16 = if (lo != 0) { 1000 } else { 0 };

    let composite = (tpr_priority as u32 / 3)
        .saturating_add(tpr_subclass as u32 / 3)
        .saturating_add(tpr_active as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_x2apic_tpr_ema = ((s.msr_ia32_x2apic_tpr_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.tpr_priority = tpr_priority;
    s.tpr_subclass = tpr_subclass;
    s.tpr_active = tpr_active;
    s.msr_ia32_x2apic_tpr_ema = msr_ia32_x2apic_tpr_ema;

    serial_println!("[msr_ia32_x2apic_tpr] age={} tpr_priority={} tpr_subclass={} tpr_active={} ema={}",
        age, tpr_priority, tpr_subclass, tpr_active, msr_ia32_x2apic_tpr_ema);
}

pub fn get_tpr_priority()  -> u16 { MODULE.lock().tpr_priority }
pub fn get_tpr_subclass()  -> u16 { MODULE.lock().tpr_subclass }
pub fn get_tpr_active()  -> u16 { MODULE.lock().tpr_active }
pub fn get_msr_ia32_x2apic_tpr_ema() -> u16 { MODULE.lock().msr_ia32_x2apic_tpr_ema }
