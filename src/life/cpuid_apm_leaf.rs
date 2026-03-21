#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    invariant_tsc: u16,
    apm_legacy_count: u16,
    apm_hw_pstate: u16,
    apm_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    invariant_tsc: 0,
    apm_legacy_count: 0,
    apm_hw_pstate: 0,
    apm_ema: 0,
});

fn has_ext_leaf7() -> bool {
    let max_ext: u32;
    unsafe {
        asm!("push rbx", "cpuid", "pop rbx",
            inout("eax") 0x80000000u32 => max_ext,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem));
    }
    max_ext >= 0x80000007
}

fn popcount8(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 { c += v & 1; v >>= 1; }
    c
}

pub fn init() { serial_println!("[cpuid_apm_leaf] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_ext_leaf7() { return; }

    let edx_val: u32;
    unsafe {
        asm!("push rbx", "cpuid", "pop rbx",
            inout("eax") 0x80000007u32 => _,
            lateout("ecx") _, lateout("edx") edx_val,
            options(nostack, nomem));
    }

    let invariant_tsc: u16 = if (edx_val >> 8) & 1 != 0 { 1000 } else { 0 };
    let apm_legacy_count: u16 = (popcount8(edx_val & 0xFF).saturating_mul(125)).min(1000) as u16;
    let apm_hw_pstate: u16 = if (edx_val >> 7) & 1 != 0 { 1000 } else { 0 };

    let composite = (invariant_tsc as u32 / 2)
        .saturating_add(apm_legacy_count as u32 / 4)
        .saturating_add(apm_hw_pstate as u32 / 4);

    let mut s = MODULE.lock();
    let apm_ema = (((s.apm_ema as u32).wrapping_mul(7)).wrapping_add(composite) / 8) as u16;

    s.invariant_tsc = invariant_tsc;
    s.apm_legacy_count = apm_legacy_count;
    s.apm_hw_pstate = apm_hw_pstate;
    s.apm_ema = apm_ema;

    serial_println!("[cpuid_apm_leaf] age={} inv_tsc={} legacy={} hw_pstate={} ema={}",
        age, invariant_tsc, apm_legacy_count, apm_hw_pstate, apm_ema);
}

pub fn get_invariant_tsc() -> u16 { MODULE.lock().invariant_tsc }
pub fn get_apm_legacy_count() -> u16 { MODULE.lock().apm_legacy_count }
pub fn get_apm_hw_pstate() -> u16 { MODULE.lock().apm_hw_pstate }
pub fn get_apm_ema() -> u16 { MODULE.lock().apm_ema }
