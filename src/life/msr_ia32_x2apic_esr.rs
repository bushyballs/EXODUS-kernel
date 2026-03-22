#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_X2APIC_ESR: u32 = 0x828;
const TICK_GATE: u32 = 1500;

pub struct State {
    apic_error_bits: u16,
    apic_send_errors: u16,
    apic_recv_errors: u16,
    apic_esr_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    apic_error_bits: 0,
    apic_send_errors: 0,
    apic_recv_errors: 0,
    apic_esr_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count += v & 1;
        v >>= 1;
    }
    count
}

fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {ecx}, ecx",
            "pop rbx",
            ecx = out(reg) ecx,
            out("eax") _,
            out("edx") _,
            options(nostack),
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

pub fn init() {
    let mut s = MODULE.lock();
    s.apic_error_bits = 0;
    s.apic_send_errors = 0;
    s.apic_recv_errors = 0;
    s.apic_esr_ema = 0;
    serial_println!("[msr_ia32_x2apic_esr] init complete");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_x2apic() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_X2APIC_ESR);

    let error_byte = lo & 0xFF;

    let total_error_count = popcount(error_byte);
    let apic_error_bits = ((total_error_count * 1000) / 8).min(1000) as u16;

    // bits 0, 2, 5 = send checksum error, send accept error, send illegal vector
    let send_count = ((error_byte >> 0) & 1)
        + ((error_byte >> 2) & 1)
        + ((error_byte >> 5) & 1);
    let apic_send_errors = (send_count * 333).min(1000) as u16;

    // bits 1, 3, 6 = receive checksum error, receive accept error, received illegal vector
    let recv_count = ((error_byte >> 1) & 1)
        + ((error_byte >> 3) & 1)
        + ((error_byte >> 6) & 1);
    let apic_recv_errors = (recv_count * 333).min(1000) as u16;

    let mut s = MODULE.lock();

    s.apic_error_bits = apic_error_bits;
    s.apic_send_errors = apic_send_errors;
    s.apic_recv_errors = apic_recv_errors;
    s.apic_esr_ema = ema(s.apic_esr_ema, apic_error_bits);

    serial_println!(
        "[msr_ia32_x2apic_esr] age={} esr_lo={:#010x} error_bits={} send={} recv={} ema={}",
        age,
        lo,
        s.apic_error_bits,
        s.apic_send_errors,
        s.apic_recv_errors,
        s.apic_esr_ema,
    );
}

pub fn get_apic_error_bits() -> u16 {
    MODULE.lock().apic_error_bits
}

pub fn get_apic_send_errors() -> u16 {
    MODULE.lock().apic_send_errors
}

pub fn get_apic_recv_errors() -> u16 {
    MODULE.lock().apic_recv_errors
}

pub fn get_apic_esr_ema() -> u16 {
    MODULE.lock().apic_esr_ema
}
