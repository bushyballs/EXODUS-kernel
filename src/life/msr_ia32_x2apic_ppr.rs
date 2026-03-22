#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_PPR: u32 = 0x80A;
const TICK_GATE: u32 = 800;

pub struct State {
    ppr_value: u16,
    ppr_class: u16,
    ppr_high: u16,
    ppr_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    ppr_value: 0,
    ppr_class: 0,
    ppr_high: 0,
    ppr_ema: 0,
});

fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 21) & 1 == 1
}

fn read_msr(addr: u32) -> (u32, u32) {
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
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn scale_255(val: u32) -> u16 {
    (val * 1000 / 255).min(1000) as u16
}

fn scale_15(val: u32) -> u16 {
    (val * 1000 / 15).min(1000) as u16
}

pub fn init() {
    if !has_x2apic() {
        serial_println!("[msr_ia32_x2apic_ppr] x2APIC not present, module inactive");
        return;
    }
    serial_println!("[msr_ia32_x2apic_ppr] init OK — MSR 0x80A (IA32_X2APIC_PPR)");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_x2apic() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_X2APIC_PPR);

    let raw_ppr = lo & 0xFF;
    let raw_class = (lo >> 4) & 0xF;

    let ppr_value = scale_255(raw_ppr);
    let ppr_class = scale_15(raw_class);
    let ppr_high: u16 = if raw_ppr > 128 { 1000 } else { 0 };

    let mut state = MODULE.lock();
    let ppr_ema = ema(state.ppr_ema, ppr_value);

    state.ppr_value = ppr_value;
    state.ppr_class = ppr_class;
    state.ppr_high = ppr_high;
    state.ppr_ema = ppr_ema;

    serial_println!(
        "[msr_ia32_x2apic_ppr] tick={} ppr_value={} ppr_class={} ppr_high={} ppr_ema={}",
        age, ppr_value, ppr_class, ppr_high, ppr_ema
    );
}

pub fn get_ppr_value() -> u16 {
    MODULE.lock().ppr_value
}

pub fn get_ppr_class() -> u16 {
    MODULE.lock().ppr_class
}

pub fn get_ppr_high() -> u16 {
    MODULE.lock().ppr_high
}

pub fn get_ppr_ema() -> u16 {
    MODULE.lock().ppr_ema
}
