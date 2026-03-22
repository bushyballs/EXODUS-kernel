#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ppr_priority: u16,
    ppr_subclass: u16,
    ppr_active: u16,
    msr_ia32_x2apic_ppr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ppr_priority: 0,
    ppr_subclass: 0,
    ppr_active: 0,
    msr_ia32_x2apic_ppr_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_x2apic_ppr] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x80Au32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // x2APIC processor priority register
    let ppr_priority = (((lo >> 4) & 0xF) * 1000 / 15).min(1000) as u16;
    let ppr_subclass = ((lo & 0xF) * 1000 / 15).min(1000) as u16;
    let ppr_active: u16 = if (lo != 0) { 1000 } else { 0 };

    let composite = (ppr_priority as u32 / 3)
        .saturating_add(ppr_subclass as u32 / 3)
        .saturating_add(ppr_active as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_x2apic_ppr_ema = ((s.msr_ia32_x2apic_ppr_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.ppr_priority = ppr_priority;
    s.ppr_subclass = ppr_subclass;
    s.ppr_active = ppr_active;
    s.msr_ia32_x2apic_ppr_ema = msr_ia32_x2apic_ppr_ema;

    serial_println!("[msr_ia32_x2apic_ppr] age={} ppr_priority={} ppr_subclass={} ppr_active={} ema={}",
        age, ppr_priority, ppr_subclass, ppr_active, msr_ia32_x2apic_ppr_ema);
}

pub fn get_ppr_priority()  -> u16 { MODULE.lock().ppr_priority }
pub fn get_ppr_subclass()  -> u16 { MODULE.lock().ppr_subclass }
pub fn get_ppr_active()  -> u16 { MODULE.lock().ppr_active }
pub fn get_msr_ia32_x2apic_ppr_ema() -> u16 { MODULE.lock().msr_ia32_x2apic_ppr_ema }
