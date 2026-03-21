#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    gs_base_nonzero: u16,
    kgs_base_nonzero: u16,
    gs_divergence: u16,
    gs_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gs_base_nonzero: 0,
    kgs_base_nonzero: 0,
    gs_divergence: 0,
    gs_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_gs_base] init"); }

pub fn tick(age: u32) {
    if age % 600 != 0 { return; }

    let gs_lo: u32; let _gs_hi: u32;
    let kgs_lo: u32; let _kgs_hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0xC0000101u32, out("eax") gs_lo, out("edx") _gs_hi, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0xC0000102u32, out("eax") kgs_lo, out("edx") _kgs_hi, options(nostack, nomem));
    }

    let gs_base_nonzero: u16 = if gs_lo != 0 { 1000 } else { 0 };
    let kgs_base_nonzero: u16 = if kgs_lo != 0 { 1000 } else { 0 };
    let gs_divergence: u16 = if gs_lo != kgs_lo { 1000 } else { 0 };

    let composite = (gs_base_nonzero as u32 / 4)
        .saturating_add(kgs_base_nonzero as u32 / 4)
        .saturating_add(gs_divergence as u32 / 2);

    let mut s = MODULE.lock();
    let gs_ema = (((s.gs_ema as u32).wrapping_mul(7)).wrapping_add(composite) / 8) as u16;

    s.gs_base_nonzero = gs_base_nonzero;
    s.kgs_base_nonzero = kgs_base_nonzero;
    s.gs_divergence = gs_divergence;
    s.gs_ema = gs_ema;

    serial_println!("[msr_ia32_gs_base] age={} gs={} kgs={} div={} ema={}",
        age, gs_base_nonzero, kgs_base_nonzero, gs_divergence, gs_ema);
}

pub fn get_gs_base_nonzero() -> u16 { MODULE.lock().gs_base_nonzero }
pub fn get_kgs_base_nonzero() -> u16 { MODULE.lock().kgs_base_nonzero }
pub fn get_gs_divergence() -> u16 { MODULE.lock().gs_divergence }
pub fn get_gs_ema() -> u16 { MODULE.lock().gs_ema }
