#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_SPEC_CTRL: u32 = 0x48;

struct State {
    ibrs_en:       u16,
    stibp_en:      u16,
    ssbd_en:       u16,
    spec_ctrl_ema: u16,
    last_tick:     u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ibrs_en:       0,
    stibp_en:      0,
    ssbd_en:       0,
    spec_ctrl_ema: 0,
    last_tick:     0,
});

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// Check CPUID leaf 7, sub-leaf 0, EDX bit 26 (IBRS_ALL).
/// If set, IA32_SPEC_CTRL (MSR 0x48) is present and readable.
fn has_spec_ctrl() -> bool {
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, edx",
            "pop rbx",
            in("eax") 7u32,
            in("ecx") 0u32,
            out("out") edx_out,
            // eax and ecx are clobbered by cpuid; declare them
            lateout("eax") _,
            lateout("ecx") _,
            options(nostack, nomem),
        );
    }
    (edx_out >> 26) & 1 == 1
}

fn read_spec_ctrl() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_SPEC_CTRL,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

pub fn init() {
    let mut s = MODULE.lock();
    s.ibrs_en       = 0;
    s.stibp_en      = 0;
    s.ssbd_en       = 0;
    s.spec_ctrl_ema = 0;
    s.last_tick     = 0;
    serial_println!(
        "[msr_ia32_spec_ctrl] init: module ready, has_spec_ctrl={}",
        has_spec_ctrl()
    );
}

pub fn tick(age: u32) {
    let mut s = MODULE.lock();

    if age.wrapping_sub(s.last_tick) < 4000 {
        return;
    }
    s.last_tick = age;

    if !has_spec_ctrl() {
        serial_println!(
            "[msr_ia32_spec_ctrl] tick age={}: CPUID IBRS_ALL (EDX bit 26) not set — skipping rdmsr",
            age
        );
        return;
    }

    let raw = read_spec_ctrl();

    let ibrs_en:  u16 = if (raw >> 0) & 1 == 1 { 1000 } else { 0 };
    let stibp_en: u16 = if (raw >> 1) & 1 == 1 { 1000 } else { 0 };
    let ssbd_en:  u16 = if (raw >> 2) & 1 == 1 { 1000 } else { 0 };

    // composite = ibrs/4 + stibp/4 + ssbd/2  (max = 250 + 250 + 500 = 1000)
    let composite: u16 = ((ibrs_en as u32)
        .wrapping_div(4)
        .saturating_add((stibp_en as u32).wrapping_div(4))
        .saturating_add((ssbd_en as u32).wrapping_div(2))
        .min(1000)) as u16;

    // EMA: ((old * 7) + new) / 8
    let new_ema: u16 = ((s.spec_ctrl_ema as u32)
        .wrapping_mul(7)
        .saturating_add(composite as u32)
        / 8) as u16;

    s.ibrs_en       = ibrs_en;
    s.stibp_en      = stibp_en;
    s.ssbd_en       = ssbd_en;
    s.spec_ctrl_ema = new_ema;

    serial_println!(
        "[msr_ia32_spec_ctrl] tick age={}: raw=0x{:08x} ibrs={} stibp={} ssbd={} ema={}",
        age, raw, ibrs_en, stibp_en, ssbd_en, new_ema
    );
}
