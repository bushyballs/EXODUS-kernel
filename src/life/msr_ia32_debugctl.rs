#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    lbr_active: u16,
    btf_active: u16,
    bts_active: u16,
    debug_depth_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    lbr_active: 0,
    btf_active: 0,
    bts_active: 0,
    debug_depth_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_debugctl] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1D9u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: LBR — Last Branch Record recording enabled
    let lbr_active: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: BTF — Branch Trap Flag (single-step at branches)
    let btf_active: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 7: BTS — Branch Trace Store to memory
    let bts_active: u16 = if (lo >> 7) & 1 != 0 { 1000 } else { 0 };

    // Composite debug depth: LBR=heaviest, BTF=moderate, BTS=moderate
    let composite = (lbr_active as u32 / 2)
        .saturating_add(btf_active as u32 / 4)
        .saturating_add(bts_active as u32 / 4);

    let mut s = MODULE.lock();
    let debug_depth_ema = ((s.debug_depth_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.lbr_active = lbr_active;
    s.btf_active = btf_active;
    s.bts_active = bts_active;
    s.debug_depth_ema = debug_depth_ema;

    serial_println!("[msr_ia32_debugctl] age={} lbr={} btf={} bts={} ema={}",
        age, lbr_active, btf_active, bts_active, debug_depth_ema);
}

pub fn get_lbr_active()       -> u16 { MODULE.lock().lbr_active }
pub fn get_btf_active()       -> u16 { MODULE.lock().btf_active }
pub fn get_bts_active()       -> u16 { MODULE.lock().bts_active }
pub fn get_debug_depth_ema()  -> u16 { MODULE.lock().debug_depth_ema }
