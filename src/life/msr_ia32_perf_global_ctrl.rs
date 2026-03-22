#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    gp_counters_en: u16,
    fixed_counters_en: u16,
    pmu_activity: u16,
    pmu_activity_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gp_counters_en: 0,
    fixed_counters_en: 0,
    pmu_activity: 0,
    pmu_activity_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 { c += v & 1; v >>= 1; }
    c
}

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

pub fn init() { serial_println!("[msr_ia32_perf_global_ctrl] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_pmu() { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Fu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    let gp_en = popcount(lo & 0xF);
    let gp_counters_en = (gp_en.saturating_mul(250)).min(1000) as u16;

    let fixed_en = popcount(hi & 0x7);
    let fixed_counters_en = (fixed_en.saturating_mul(333)).min(1000) as u16;

    let pmu_activity = ((gp_counters_en as u32 * 500
        + fixed_counters_en as u32 * 500) / 1000).min(1000) as u16;

    let mut s = MODULE.lock();
    let pmu_activity_ema = ((s.pmu_activity_ema as u32).wrapping_mul(7)
        .saturating_add(pmu_activity as u32) / 8).min(1000) as u16;

    s.gp_counters_en = gp_counters_en;
    s.fixed_counters_en = fixed_counters_en;
    s.pmu_activity = pmu_activity;
    s.pmu_activity_ema = pmu_activity_ema;

    serial_println!("[msr_ia32_perf_global_ctrl] age={} gp={} fixed={} act={} ema={}",
        age, gp_counters_en, fixed_counters_en, pmu_activity, pmu_activity_ema);
}

pub fn get_gp_counters_en()    -> u16 { MODULE.lock().gp_counters_en }
pub fn get_fixed_counters_en() -> u16 { MODULE.lock().fixed_counters_en }
pub fn get_pmu_activity()      -> u16 { MODULE.lock().pmu_activity }
pub fn get_pmu_activity_ema()  -> u16 { MODULE.lock().pmu_activity_ema }
