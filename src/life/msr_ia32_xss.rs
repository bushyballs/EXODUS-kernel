#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_XSS MSR — Extended Supervisor State Components Sense
// MSR address: 0xDA0
// Guard: CPUID leaf 1 ECX bit 26 (XSAVE supported)
//
// Supervisor XSAVE component bits in lo:
//   bit  8 = PT (Processor Trace state)
//   bit  9 = PASID
//   bit 11 = CET_U (CET user shadow stack)
//   bit 12 = CET_S (CET supervisor shadow stack)
//   bit 13 = HDC
//   bit 17 = UINTR

const MSR_IA32_XSS: u32 = 0xDA0;

struct State {
    xss_pt_enabled:       u16,
    xss_cet_enabled:      u16,
    xss_component_count:  u16,
    xss_ema:              u16,
    last_tick:            u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    xss_pt_enabled:      0,
    xss_cet_enabled:     0,
    xss_component_count: 0,
    xss_ema:             0,
    last_tick:           0,
});

// CPUID leaf 1 ECX bit 26 — XSAVE supported by processor
fn xsave_supported() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 26) & 1 == 1
}

fn read_msr_ia32_xss() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_XSS,
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
    s.xss_pt_enabled      = 0;
    s.xss_cet_enabled     = 0;
    s.xss_component_count = 0;
    s.xss_ema             = 0;
    s.last_tick           = 0;
    serial_println!("[msr_ia32_xss] init");
}

pub fn tick(age: u32) {
    {
        let s = MODULE.lock();
        if age.wrapping_sub(s.last_tick) < 6000 {
            return;
        }
    }

    if !xsave_supported() {
        let mut s = MODULE.lock();
        s.last_tick           = age;
        s.xss_pt_enabled      = 0;
        s.xss_cet_enabled     = 0;
        s.xss_component_count = 0;
        s.xss_ema             = ema(s.xss_ema, 0);
        serial_println!(
            "[msr_ia32_xss] tick age={} XSAVE not supported; signals zeroed",
            age
        );
        return;
    }

    let lo = read_msr_ia32_xss();

    // xss_pt_enabled: bit 8 of lo
    let xss_pt_enabled: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };

    // xss_cet_enabled: 1000 if bit 11 or bit 12 set
    let xss_cet_enabled: u16 = if lo & ((1 << 11) | (1 << 12)) != 0 { 1000 } else { 0 };

    // xss_component_count: popcount(lo & 0x3FFF) * 71, clamped to 1000
    let raw_count = popcount(lo & 0x3FFF);
    let xss_component_count: u16 = (raw_count.saturating_mul(71)).min(1000) as u16;

    // xss_ema: EMA of (pt_enabled/4 + cet_enabled/4 + component_count/2)
    let composite = ((xss_pt_enabled as u32) / 4)
        .saturating_add((xss_cet_enabled as u32) / 4)
        .saturating_add((xss_component_count as u32) / 2)
        .min(1000) as u16;

    let mut s = MODULE.lock();
    s.xss_ema             = ema(s.xss_ema, composite);
    s.xss_pt_enabled      = xss_pt_enabled;
    s.xss_cet_enabled     = xss_cet_enabled;
    s.xss_component_count = xss_component_count;
    s.last_tick           = age;

    serial_println!(
        "[msr_ia32_xss] age={} lo={:#010x} pt={} cet={} components={} ema={}",
        age,
        lo,
        xss_pt_enabled,
        xss_cet_enabled,
        xss_component_count,
        s.xss_ema,
    );
}

pub fn get_xss_pt_enabled()      -> u16 { MODULE.lock().xss_pt_enabled }
pub fn get_xss_cet_enabled()     -> u16 { MODULE.lock().xss_cet_enabled }
pub fn get_xss_component_count() -> u16 { MODULE.lock().xss_component_count }
pub fn get_xss_ema()             -> u16 { MODULE.lock().xss_ema }
