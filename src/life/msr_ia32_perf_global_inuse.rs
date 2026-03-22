#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    gp_counters_inuse: u16,
    fixed_counters_inuse: u16,
    total_counters_inuse: u16,
    counter_pressure_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gp_counters_inuse: 0,
    fixed_counters_inuse: 0,
    total_counters_inuse: 0,
    counter_pressure_ema: 0,
});

#[inline]
fn has_pmu() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0xAu32 => eax,
            in("ecx") 0u32,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax & 0xFF) >= 1
}

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 { c += v & 1; v >>= 1; }
    c
}

pub fn init() { serial_println!("[msr_ia32_perf_global_inuse] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    if !has_pmu() { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x392u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    let gp = popcount(lo & 0xF);
    let gp_counters_inuse = (gp.saturating_mul(250)).min(1000) as u16;

    let fixed = popcount(hi & 0x7);
    let fixed_counters_inuse = (fixed.saturating_mul(333)).min(1000) as u16;

    let total = (gp_counters_inuse as u32 * 500 + fixed_counters_inuse as u32 * 500) / 1000;
    let total_counters_inuse = total.min(1000) as u16;

    let mut s = MODULE.lock();
    let counter_pressure_ema = ((s.counter_pressure_ema as u32).wrapping_mul(7)
        .saturating_add(total_counters_inuse as u32) / 8) as u16;

    s.gp_counters_inuse = gp_counters_inuse;
    s.fixed_counters_inuse = fixed_counters_inuse;
    s.total_counters_inuse = total_counters_inuse;
    s.counter_pressure_ema = counter_pressure_ema;

    serial_println!("[msr_ia32_perf_global_inuse] age={} gp={} fixed={} total={} ema={}",
        age, gp_counters_inuse, fixed_counters_inuse, total_counters_inuse, counter_pressure_ema);
}

pub fn get_gp_counters_inuse() -> u16 { MODULE.lock().gp_counters_inuse }
pub fn get_fixed_counters_inuse() -> u16 { MODULE.lock().fixed_counters_inuse }
pub fn get_total_counters_inuse() -> u16 { MODULE.lock().total_counters_inuse }
pub fn get_counter_pressure_ema() -> u16 { MODULE.lock().counter_pressure_ema }
