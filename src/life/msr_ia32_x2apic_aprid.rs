#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_APICID: u32 = 0x802;
const TICK_GATE: u32 = 10000;

pub struct State {
    x2apic_id_lo:   u16,
    x2apic_cluster: u16,
    x2apic_nonzero: u16,
    x2apic_ema:     u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    x2apic_id_lo:   0,
    x2apic_cluster: 0,
    x2apic_nonzero: 0,
    x2apic_ema:     0,
});

pub fn init() {
    serial_println!("[msr_ia32_x2apic_aprid] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_x2apic() {
        return;
    }

    let raw = read_msr(MSR_IA32_X2APIC_APICID);

    let lo_raw = (raw & 0xFFFF) as u32;
    let hi_raw = ((raw >> 16) & 0xFFFF) as u32;

    let id_lo   = ((lo_raw * 1000) / 65535).min(1000) as u16;
    let cluster = ((hi_raw * 1000) / 65535).min(1000) as u16;
    let nonzero: u16 = if lo_raw != 0 { 1000 } else { 0 };

    let mut state = MODULE.lock();

    let old_ema = state.x2apic_ema as u32;
    let new_ema = ((old_ema.wrapping_mul(7).saturating_add(id_lo as u32)) / 8) as u16;

    state.x2apic_id_lo   = id_lo;
    state.x2apic_cluster = cluster;
    state.x2apic_nonzero = nonzero;
    state.x2apic_ema     = new_ema;

    serial_println!(
        "[msr_ia32_x2apic_aprid] age={} raw=0x{:08x} id_lo={} cluster={} nonzero={} ema={}",
        age, raw, id_lo, cluster, nonzero, new_ema
    );
}

pub fn get_x2apic_id_lo() -> u16 {
    MODULE.lock().x2apic_id_lo
}

pub fn get_x2apic_cluster() -> u16 {
    MODULE.lock().x2apic_cluster
}

pub fn get_x2apic_nonzero() -> u16 {
    MODULE.lock().x2apic_nonzero
}

pub fn get_x2apic_ema() -> u16 {
    MODULE.lock().x2apic_ema
}

fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx >> 21) & 1 == 1
}

fn read_msr(addr: u32) -> u32 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    // IA32_X2APIC_APICID is a full 32-bit value split across EDX:EAX.
    // The architectural spec places the full APIC ID in EAX for this MSR,
    // with EDX reserved/zero. We reconstruct the full 32 bits anyway.
    lo | (hi << 16)
}
