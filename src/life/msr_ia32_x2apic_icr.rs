#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_ICR: u32 = 0x830;
const TICK_GATE: u32 = 800;

pub struct State {
    icr_vector:        u16,
    icr_delivery_mode: u16,
    icr_pending:       u16,
    icr_ema:           u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    icr_vector:        0,
    icr_delivery_mode: 0,
    icr_pending:       0,
    icr_ema:           0,
});

// Returns true if the CPU supports x2APIC (CPUID leaf 1, ECX bit 21).
fn x2apic_supported() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {0:e}, ecx",
            "pop rbx",
            out(reg) ecx,
            options(nostack, nomem),
        );
    }
    (ecx >> 21) & 1 == 1
}

// Read the 64-bit IA32_X2APIC_ICR MSR and return (lo, hi).
fn read_x2apic_icr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_X2APIC_ICR,
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
    serial_println!("[msr_ia32_x2apic_icr] init");
    let mut s = MODULE.lock();
    s.icr_vector        = 0;
    s.icr_delivery_mode = 0;
    s.icr_pending       = 0;
    s.icr_ema           = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !x2apic_supported() {
        serial_println!("[msr_ia32_x2apic_icr] x2APIC not supported, skipping");
        return;
    }

    let (lo, _hi) = read_x2apic_icr();

    // bits[7:0]: interrupt vector — scale 0–255 → 0–1000
    let raw_vector = (lo & 0xFF) as u32;
    let icr_vector = ((raw_vector * 1000) / 255).min(1000) as u16;

    // bits[10:8]: delivery mode — scale 0–7 → 0–1000
    let raw_mode = ((lo >> 8) & 0x7) as u32;
    let icr_delivery_mode = ((raw_mode * 1000) / 7).min(1000) as u16;

    // bit 12: delivery status (1 = send pending)
    let icr_pending: u16 = if (lo >> 12) & 1 == 1 { 1000 } else { 0 };

    let mut s = MODULE.lock();
    s.icr_vector        = icr_vector;
    s.icr_delivery_mode = icr_delivery_mode;
    s.icr_pending       = icr_pending;
    s.icr_ema           = ema(s.icr_ema, icr_vector);

    serial_println!(
        "[msr_ia32_x2apic_icr] age={} vector={} mode={} pending={} ema={}",
        age,
        s.icr_vector,
        s.icr_delivery_mode,
        s.icr_pending,
        s.icr_ema,
    );
}

pub fn get_icr_vector() -> u16 {
    MODULE.lock().icr_vector
}

pub fn get_icr_delivery_mode() -> u16 {
    MODULE.lock().icr_delivery_mode
}

pub fn get_icr_pending() -> u16 {
    MODULE.lock().icr_pending
}

pub fn get_icr_ema() -> u16 {
    MODULE.lock().icr_ema
}
