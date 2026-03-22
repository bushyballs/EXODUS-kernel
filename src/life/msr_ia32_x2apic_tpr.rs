#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const IA32_X2APIC_TPR: u32 = 0x808;
const TICK_GATE: u32 = 1000;

pub struct State {
    tpr_value:    u16,
    tpr_class:    u16,
    tpr_nonzero:  u16,
    tpr_ema:      u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    tpr_value:   0,
    tpr_class:   0,
    tpr_nonzero: 0,
    tpr_ema:     0,
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
    (ecx >> 21) & 1 != 0
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

pub fn init() {
    let mut s = MODULE.lock();
    s.tpr_value   = 0;
    s.tpr_class   = 0;
    s.tpr_nonzero = 0;
    s.tpr_ema     = 0;
    serial_println!("[msr_ia32_x2apic_tpr] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_x2apic() {
        return;
    }

    let (lo, _hi) = read_msr(IA32_X2APIC_TPR);

    let raw_tpr   = lo & 0xFF;
    let raw_class = (lo >> 4) & 0xF;

    let tpr_value   = ((raw_tpr * 1000 / 255) as u16).min(1000);
    let tpr_class   = ((raw_class * 1000 / 15) as u16).min(1000);
    let tpr_nonzero = if raw_tpr != 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();
    s.tpr_value   = tpr_value;
    s.tpr_class   = tpr_class;
    s.tpr_nonzero = tpr_nonzero;
    s.tpr_ema     = ema(s.tpr_ema, tpr_value);

    serial_println!(
        "[msr_ia32_x2apic_tpr] age={} tpr_value={} tpr_class={} tpr_nonzero={} tpr_ema={}",
        age, s.tpr_value, s.tpr_class, s.tpr_nonzero, s.tpr_ema
    );
}

pub fn get_tpr_value() -> u16 {
    MODULE.lock().tpr_value
}

pub fn get_tpr_class() -> u16 {
    MODULE.lock().tpr_class
}

pub fn get_tpr_nonzero() -> u16 {
    MODULE.lock().tpr_nonzero
}

pub fn get_tpr_ema() -> u16 {
    MODULE.lock().tpr_ema
}
