#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    fixed0_active: u16,
    fixed1_active: u16,
    fixed2_active: u16,
    fixed_ctrl_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    fixed0_active: 0,
    fixed1_active: 0,
    fixed2_active: 0,
    fixed_ctrl_ema: 0,
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

pub fn init() { serial_println!("[msr_ia32_fixed_ctr_ctrl] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_pmu() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Du32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // Each fixed counter has 4-bit field; bits[1:0]=EN (0=off,1=os,2=usr,3=both)
    // Fixed CTR0: instructions retired bits[3:0]
    // Fixed CTR1: CPU_CLK_UNHALTED.CORE bits[7:4]
    // Fixed CTR2: CPU_CLK_UNHALTED.REF  bits[11:8]
    let en0 = lo & 0x3;
    let en1 = (lo >> 4) & 0x3;
    let en2 = (lo >> 8) & 0x3;

    let fixed0_active: u16 = if en0 != 0 { 1000 } else { 0 };
    let fixed1_active: u16 = if en1 != 0 { 1000 } else { 0 };
    let fixed2_active: u16 = if en2 != 0 { 1000 } else { 0 };

    let composite = ((fixed0_active as u32)
        .saturating_add(fixed1_active as u32)
        .saturating_add(fixed2_active as u32)) / 3;

    let mut s = MODULE.lock();
    let fixed_ctrl_ema = ((s.fixed_ctrl_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.fixed0_active = fixed0_active;
    s.fixed1_active = fixed1_active;
    s.fixed2_active = fixed2_active;
    s.fixed_ctrl_ema = fixed_ctrl_ema;

    serial_println!("[msr_ia32_fixed_ctr_ctrl] age={} ctr0={} ctr1={} ctr2={} ema={}",
        age, fixed0_active, fixed1_active, fixed2_active, fixed_ctrl_ema);
}

pub fn get_fixed0_active()  -> u16 { MODULE.lock().fixed0_active }
pub fn get_fixed1_active()  -> u16 { MODULE.lock().fixed1_active }
pub fn get_fixed2_active()  -> u16 { MODULE.lock().fixed2_active }
pub fn get_fixed_ctrl_ema() -> u16 { MODULE.lock().fixed_ctrl_ema }
