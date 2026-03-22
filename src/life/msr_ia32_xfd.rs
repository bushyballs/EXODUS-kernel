#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_XFD MSR — Extended Feature Disable Sense
// MSR address: 0x1C4
// Guard: CPUID leaf 0xD sub-leaf 0 ECX bit 4 (XFD supported — check max leaf >= 0xD)
//
// lo bits[1:0] = feature disable bitmap:
//   bit 0 = reserved
//   bit 1 = disable AMX tiles
// Higher bits disable other extended features if present.

const MSR_IA32_XFD: u32 = 0x1C4;

struct State {
    xfd_amx_disabled:      u16,
    xfd_any_disabled:      u16,
    xfd_suppression_count: u16,
    xfd_ema:               u16,
    last_tick:             u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    xfd_amx_disabled:      0,
    xfd_any_disabled:      0,
    xfd_suppression_count: 0,
    xfd_ema:               0,
    last_tick:             0,
});

// Check whether IA32_XFD is supported.
// Method: verify CPUID max basic leaf >= 0xD, then check CPUID leaf 0xD sub-leaf 0 ECX bit 4.
fn xfd_supported() -> bool {
    // Step 1: get max basic CPUID leaf
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0xD {
        return false;
    }

    // Step 2: CPUID leaf 0xD sub-leaf 0, check ECX bit 4 (XFD supported)
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xDu32 => _,
            inout("ecx") 0u32 => ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 4) & 1 == 1
}

fn read_msr_ia32_xfd() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_XFD,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.xfd_amx_disabled      = 0;
    s.xfd_any_disabled      = 0;
    s.xfd_suppression_count = 0;
    s.xfd_ema               = 0;
    s.last_tick             = 0;
    serial_println!("[msr_ia32_xfd] init");
}

pub fn tick(age: u32) {
    {
        let s = MODULE.lock();
        if age.wrapping_sub(s.last_tick) < 6000 {
            return;
        }
    }

    if !xfd_supported() {
        let mut s = MODULE.lock();
        s.xfd_ema       = ema(s.xfd_ema, 0);
        s.last_tick     = age;
        serial_println!(
            "[msr_ia32_xfd] tick age={} XFD not supported; signals zeroed",
            age
        );
        return;
    }

    let lo = read_msr_ia32_xfd();

    // xfd_amx_disabled: bit 1 of lo — 0 or 1000
    let xfd_amx_disabled: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };

    // xfd_any_disabled: 1000 if lo != 0, else 0
    let xfd_any_disabled: u16 = if lo != 0 { 1000 } else { 0 };

    // xfd_suppression_count: popcount(lo & 0xFF) * 125, clamped to 1000
    let raw_pop = popcount(lo & 0xFF);
    let xfd_suppression_count: u16 = (raw_pop.saturating_mul(125)).min(1000) as u16;

    // xfd_ema: EMA of (amx_disabled/4 + any_disabled/4 + suppression_count/2)
    let composite = ((xfd_amx_disabled as u32) / 4)
        .saturating_add((xfd_any_disabled as u32) / 4)
        .saturating_add((xfd_suppression_count as u32) / 2)
        .min(1000) as u16;

    let mut s = MODULE.lock();
    s.xfd_ema               = ema(s.xfd_ema, composite);
    s.xfd_amx_disabled      = xfd_amx_disabled;
    s.xfd_any_disabled      = xfd_any_disabled;
    s.xfd_suppression_count = xfd_suppression_count;
    s.last_tick             = age;

    serial_println!(
        "[msr_ia32_xfd] age={} lo={:#010x} amx_disabled={} any_disabled={} suppression={} ema={}",
        age,
        lo,
        xfd_amx_disabled,
        xfd_any_disabled,
        xfd_suppression_count,
        s.xfd_ema,
    );
}

pub fn get_xfd_amx_disabled()      -> u16 { MODULE.lock().xfd_amx_disabled }
pub fn get_xfd_any_disabled()      -> u16 { MODULE.lock().xfd_any_disabled }
pub fn get_xfd_suppression_count() -> u16 { MODULE.lock().xfd_suppression_count }
pub fn get_xfd_ema()               -> u16 { MODULE.lock().xfd_ema }
