#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_TSC_AUX: u32 = 0xC000_0103;
const TICK_GATE: u32 = 8000;

pub struct State {
    pub tsc_aux_id:       u16,
    pub tsc_aux_hi8:      u16,
    pub tsc_aux_nonzero:  u16,
    pub tsc_aux_ema:      u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    tsc_aux_id:      0,
    tsc_aux_hi8:     0,
    tsc_aux_nonzero: 0,
    tsc_aux_ema:     0,
});

// ── CPUID guard ────────────────────────────────────────────────────────────────

fn has_rdtscp() -> bool {
    let max_ext: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x8000_0000u32 => max_ext,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    if max_ext < 0x8000_0001 {
        return false;
    }
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x8000_0001u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 8) & 1 == 1
}

// ── MSR read ───────────────────────────────────────────────────────────────────

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

// ── Signal derivation ──────────────────────────────────────────────────────────

fn derive_tsc_aux_id(lo: u32) -> u16 {
    let val = lo & 0xFFFF;
    ((val as u64 * 1000 / 65535) as u16).min(1000)
}

fn derive_tsc_aux_hi8(lo: u32) -> u16 {
    let val = (lo >> 24) & 0xFF;
    ((val as u32 * 1000 / 255) as u16).min(1000)
}

fn derive_tsc_aux_nonzero(lo: u32) -> u16 {
    if lo != 0 { 1000 } else { 0 }
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Public interface ───────────────────────────────────────────────────────────

pub fn init() {
    if !has_rdtscp() {
        serial_println!("[msr_ia32_tsc_aux] RDTSCP not supported — module inactive");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_TSC_AUX);
    let id      = derive_tsc_aux_id(lo);
    let hi8     = derive_tsc_aux_hi8(lo);
    let nonzero = derive_tsc_aux_nonzero(lo);
    let mut s = MODULE.lock();
    s.tsc_aux_id      = id;
    s.tsc_aux_hi8     = hi8;
    s.tsc_aux_nonzero = nonzero;
    s.tsc_aux_ema     = id;
    serial_println!(
        "[msr_ia32_tsc_aux] init: id={} hi8={} nonzero={} ema={}",
        s.tsc_aux_id, s.tsc_aux_hi8, s.tsc_aux_nonzero, s.tsc_aux_ema,
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_rdtscp() {
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_TSC_AUX);
    let id      = derive_tsc_aux_id(lo);
    let hi8     = derive_tsc_aux_hi8(lo);
    let nonzero = derive_tsc_aux_nonzero(lo);
    let mut s = MODULE.lock();
    s.tsc_aux_ema     = ema(s.tsc_aux_ema, id);
    s.tsc_aux_id      = id;
    s.tsc_aux_hi8     = hi8;
    s.tsc_aux_nonzero = nonzero;
    serial_println!(
        "[msr_ia32_tsc_aux] tick {}: id={} hi8={} nonzero={} ema={}",
        age, s.tsc_aux_id, s.tsc_aux_hi8, s.tsc_aux_nonzero, s.tsc_aux_ema,
    );
}

// ── Getters ────────────────────────────────────────────────────────────────────

pub fn get_tsc_aux_id() -> u16 {
    MODULE.lock().tsc_aux_id
}

pub fn get_tsc_aux_hi8() -> u16 {
    MODULE.lock().tsc_aux_hi8
}

pub fn get_tsc_aux_nonzero() -> u16 {
    MODULE.lock().tsc_aux_nonzero
}

pub fn get_tsc_aux_ema() -> u16 {
    MODULE.lock().tsc_aux_ema
}
