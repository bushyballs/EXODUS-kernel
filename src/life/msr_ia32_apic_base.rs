#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    apic_bsp: u16,
    x2apic_mode: u16,
    apic_global_en: u16,
    apic_topology_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    apic_bsp: 0,
    x2apic_mode: 0,
    apic_global_en: 0,
    apic_topology_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_apic_base] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1Bu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 8: BSP flag (1 = this is the bootstrap processor)
    let apic_bsp: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    // bit 10: x2APIC mode enabled
    let x2apic_mode: u16 = if (lo >> 10) & 1 != 0 { 1000 } else { 0 };
    // bit 11: APIC global enable
    let apic_global_en: u16 = if (lo >> 11) & 1 != 0 { 1000 } else { 0 };

    let composite = (apic_bsp as u32 / 4)
        .saturating_add(x2apic_mode as u32 / 4)
        .saturating_add(apic_global_en as u32 / 2);

    let mut s = MODULE.lock();
    let apic_topology_ema = ((s.apic_topology_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.apic_bsp = apic_bsp;
    s.x2apic_mode = x2apic_mode;
    s.apic_global_en = apic_global_en;
    s.apic_topology_ema = apic_topology_ema;

    serial_println!("[msr_ia32_apic_base] age={} bsp={} x2apic={} en={} ema={}",
        age, apic_bsp, x2apic_mode, apic_global_en, apic_topology_ema);
}

pub fn get_apic_bsp()           -> u16 { MODULE.lock().apic_bsp }
pub fn get_x2apic_mode()        -> u16 { MODULE.lock().x2apic_mode }
pub fn get_apic_global_en()     -> u16 { MODULE.lock().apic_global_en }
pub fn get_apic_topology_ema()  -> u16 { MODULE.lock().apic_topology_ema }
