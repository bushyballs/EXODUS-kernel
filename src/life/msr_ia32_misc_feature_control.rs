#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    prefetch_disable: u16,
    adjacent_disable: u16,
    dcu_prefetch_disable: u16,
    misc_ctrl_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    prefetch_disable: 0,
    adjacent_disable: 0,
    dcu_prefetch_disable: 0,
    misc_ctrl_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_misc_feature_control] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1A4u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: L2 HW prefetcher disable
    let prefetch_disable: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: L2 Adjacent Cache Line Prefetch disable
    let adjacent_disable: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 2: DCU HW prefetcher disable
    let dcu_prefetch_disable: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    let composite = (prefetch_disable as u32 / 3)
        .saturating_add(adjacent_disable as u32 / 3)
        .saturating_add(dcu_prefetch_disable as u32 / 3);

    let mut s = MODULE.lock();
    let misc_ctrl_ema = ((s.misc_ctrl_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.prefetch_disable = prefetch_disable;
    s.adjacent_disable = adjacent_disable;
    s.dcu_prefetch_disable = dcu_prefetch_disable;
    s.misc_ctrl_ema = misc_ctrl_ema;

    serial_println!("[msr_ia32_misc_feature_control] age={} pf_dis={} adj_dis={} dcu_dis={} ema={}",
        age, prefetch_disable, adjacent_disable, dcu_prefetch_disable, misc_ctrl_ema);
}

pub fn get_prefetch_disable()     -> u16 { MODULE.lock().prefetch_disable }
pub fn get_adjacent_disable()     -> u16 { MODULE.lock().adjacent_disable }
pub fn get_dcu_prefetch_disable() -> u16 { MODULE.lock().dcu_prefetch_disable }
pub fn get_misc_ctrl_ema()        -> u16 { MODULE.lock().misc_ctrl_ema }
