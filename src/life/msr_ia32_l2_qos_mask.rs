#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const IA32_L2_QOS_MASK_0: u32 = 0xD10;
const TICK_GATE: u32 = 9000;

pub struct State {
    pub l2_mask_bits:    u16,
    pub l2_mask_raw:     u16,
    pub l2_mask_density: u16,
    pub l2_qos_ema:      u16,
    pub supported:       bool,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    l2_mask_bits:    0,
    l2_mask_raw:     0,
    l2_mask_density: 0,
    l2_qos_ema:      0,
    supported:       false,
});

fn has_l2_cat() -> bool {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0x10 {
        return false;
    }
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "mov {out:e}, ebx", "pop rbx",
            inout("eax") 0x10u32 => _,
            out = out(reg) ebx,
            in("ecx") 2u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    ebx != 0
}

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0F0F_0F0F;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let supported = has_l2_cat();
    let mut s = MODULE.lock();
    s.supported = supported;
    if supported {
        serial_println!("[msr_ia32_l2_qos_mask] L2 CAT supported — init OK");
    } else {
        serial_println!("[msr_ia32_l2_qos_mask] L2 CAT not supported on this CPU — signals will remain zero");
    }
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    let mut s = MODULE.lock();
    if !s.supported {
        return;
    }

    let (lo, _hi) = read_msr(IA32_L2_QOS_MASK_0);

    // l2_mask_bits: popcount of lo bits, scaled to 0-1000 (up to 20 L2 ways typical)
    let count = popcount(lo);
    let l2_mask_bits = ((count * 1000) / 20).min(1000) as u16;

    // l2_mask_raw: lower 20 bits scaled to 0-1000
    let lo20 = lo & 0x000F_FFFF;
    let l2_mask_raw = ((lo20 * 1000) / 0x000F_FFFF).min(1000) as u16;

    // l2_mask_density: fraction of 32-bit word that is set, scaled 0-1000
    let l2_mask_density = ((count * 1000) / 32).min(1000) as u16;

    // l2_qos_ema: EMA of l2_mask_bits
    let l2_qos_ema = ema(s.l2_qos_ema, l2_mask_bits);

    s.l2_mask_bits    = l2_mask_bits;
    s.l2_mask_raw     = l2_mask_raw;
    s.l2_mask_density = l2_mask_density;
    s.l2_qos_ema      = l2_qos_ema;
}

pub fn get_l2_mask_bits() -> u16 {
    MODULE.lock().l2_mask_bits
}

pub fn get_l2_mask_raw() -> u16 {
    MODULE.lock().l2_mask_raw
}

pub fn get_l2_mask_density() -> u16 {
    MODULE.lock().l2_mask_density
}

pub fn get_l2_qos_ema() -> u16 {
    MODULE.lock().l2_qos_ema
}
