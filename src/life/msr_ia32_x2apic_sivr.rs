#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_SIVR: u32 = 0x80F;

pub struct State {
    spurious_vector: u16,
    apic_sw_enable:  u16,
    focus_check:     u16,
    sivr_ema:        u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    spurious_vector: 0,
    apic_sw_enable:  0,
    focus_check:     0,
    sivr_ema:        0,
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

fn scale_vector(val: u32) -> u16 {
    (val.saturating_mul(1000) / 255).min(1000) as u16
}

pub fn init() {
    serial_println!("[msr_ia32_x2apic_sivr] init");
    let mut state = MODULE.lock();
    state.spurious_vector = 0;
    state.apic_sw_enable  = 0;
    state.focus_check     = 0;
    state.sivr_ema        = 0;
}

pub fn tick(age: u32) {
    if age % 4000 != 0 {
        return;
    }

    if !has_x2apic() {
        serial_println!("[msr_ia32_x2apic_sivr] x2APIC not present, skipping");
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_X2APIC_SIVR);

    let raw_vec   = lo & 0xFF;
    let sv        = scale_vector(raw_vec);
    let asw       = if (lo >> 8) & 1 == 1 { 1000u16 } else { 0u16 };
    let fc        = if (lo >> 9) & 1 == 1 { 1000u16 } else { 0u16 };

    let mut state = MODULE.lock();
    state.spurious_vector = sv;
    state.apic_sw_enable  = asw;
    state.focus_check     = fc;
    state.sivr_ema        = ema(state.sivr_ema, sv);

    serial_println!(
        "[msr_ia32_x2apic_sivr] tick={} vec={} apic_en={} focus={} ema={}",
        age,
        state.spurious_vector,
        state.apic_sw_enable,
        state.focus_check,
        state.sivr_ema,
    );
}

pub fn get_spurious_vector() -> u16 {
    MODULE.lock().spurious_vector
}

pub fn get_apic_sw_enable() -> u16 {
    MODULE.lock().apic_sw_enable
}

pub fn get_focus_check() -> u16 {
    MODULE.lock().focus_check
}

pub fn get_sivr_ema() -> u16 {
    MODULE.lock().sivr_ema
}
