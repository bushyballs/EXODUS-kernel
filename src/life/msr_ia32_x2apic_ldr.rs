#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    ldr_cluster: u16,
    ldr_logical_id: u16,
    ldr_nonzero: u16,
    msr_ia32_x2apic_ldr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ldr_cluster: 0,
    ldr_logical_id: 0,
    ldr_nonzero: 0,
    msr_ia32_x2apic_ldr_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_x2apic_ldr] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x80Du32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // x2APIC logical destination register
    let ldr_cluster = (((lo >> 16) & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ldr_logical_id = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let ldr_nonzero: u16 = if (lo != 0) { 1000 } else { 0 };

    let composite = (ldr_cluster as u32 / 3)
        .saturating_add(ldr_logical_id as u32 / 3)
        .saturating_add(ldr_nonzero as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_x2apic_ldr_ema = ((s.msr_ia32_x2apic_ldr_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.ldr_cluster = ldr_cluster;
    s.ldr_logical_id = ldr_logical_id;
    s.ldr_nonzero = ldr_nonzero;
    s.msr_ia32_x2apic_ldr_ema = msr_ia32_x2apic_ldr_ema;

    serial_println!("[msr_ia32_x2apic_ldr] age={} ldr_cluster={} ldr_logical_id={} ldr_nonzero={} ema={}",
        age, ldr_cluster, ldr_logical_id, ldr_nonzero, msr_ia32_x2apic_ldr_ema);
}

pub fn get_ldr_cluster()  -> u16 { MODULE.lock().ldr_cluster }
pub fn get_ldr_logical_id()  -> u16 { MODULE.lock().ldr_logical_id }
pub fn get_ldr_nonzero()  -> u16 { MODULE.lock().ldr_nonzero }
pub fn get_msr_ia32_x2apic_ldr_ema() -> u16 { MODULE.lock().msr_ia32_x2apic_ldr_ema }
