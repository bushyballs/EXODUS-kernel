#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    lbr_format: u16,
    pebs_trap: u16,
    pebs_arch_reg: u16,
    perf_cap_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    lbr_format: 0,
    pebs_trap: 0,
    pebs_arch_reg: 0,
    perf_cap_ema: 0,
});

#[inline]
fn has_perf_cap() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 15) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_perf_capabilities] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_perf_cap() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x345u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let raw_lbr = lo & 0x3F;
    let lbr_format = ((raw_lbr * 1000) / 63).min(1000) as u16;

    let pebs_trap: u16 = if (lo >> 6) & 1 != 0 { 1000 } else { 0 };
    let pebs_arch_reg: u16 = if (lo >> 7) & 1 != 0 { 1000 } else { 0 };

    let composite = (lbr_format as u32 / 4)
        .saturating_add(pebs_trap as u32 / 4)
        .saturating_add(pebs_arch_reg as u32 / 4);

    let mut s = MODULE.lock();
    let perf_cap_ema = ((s.perf_cap_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.lbr_format = lbr_format;
    s.pebs_trap = pebs_trap;
    s.pebs_arch_reg = pebs_arch_reg;
    s.perf_cap_ema = perf_cap_ema;

    serial_println!("[msr_ia32_perf_capabilities] age={} lbr_fmt={} pebs_trap={} pebs_ar={} ema={}",
        age, lbr_format, pebs_trap, pebs_arch_reg, perf_cap_ema);
}

pub fn get_lbr_format()    -> u16 { MODULE.lock().lbr_format }
pub fn get_pebs_trap()     -> u16 { MODULE.lock().pebs_trap }
pub fn get_pebs_arch_reg() -> u16 { MODULE.lock().pebs_arch_reg }
pub fn get_perf_cap_ema()  -> u16 { MODULE.lock().perf_cap_ema }
