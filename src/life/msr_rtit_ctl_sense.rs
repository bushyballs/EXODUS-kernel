#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pt_trace_en: u16,
    pt_branch_en: u16,
    pt_timing_en: u16,
    pt_introspect_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pt_trace_en: 0,
    pt_branch_en: 0,
    pt_timing_en: 0,
    pt_introspect_ema: 0,
});

fn pt_supported() -> bool {
    // Check max leaf >= 0x14
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0x14 { return false; }
    // Check CPUID 0x14 EAX > 0
    let eax14: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x14u32 => eax14,
            in("ecx") 0u32,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax14 > 0
}

pub fn init() { serial_println!("[msr_rtit_ctl_sense] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !pt_supported() { return; }

    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x570u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }

    let pt_trace_en = if lo & 1 != 0 { 1000u16 } else { 0 };
    let pt_branch_en = if (lo >> 9) & 1 != 0 { 1000u16 } else { 0 };
    let pt_timing_en = if (lo >> 1) & 1 != 0 { 1000u16 } else { 0 };
    let composite = (pt_trace_en as u32 / 4)
        .saturating_add(pt_branch_en as u32 / 4)
        .saturating_add(pt_timing_en as u32 / 2);

    let mut s = MODULE.lock();
    let pt_introspect_ema = ((s.pt_introspect_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;
    s.pt_trace_en = pt_trace_en;
    s.pt_branch_en = pt_branch_en;
    s.pt_timing_en = pt_timing_en;
    s.pt_introspect_ema = pt_introspect_ema;

    serial_println!("[msr_rtit_ctl_sense] age={} trace={} branch={} timing={} ema={}",
        age, pt_trace_en, pt_branch_en, pt_timing_en, pt_introspect_ema);
}
