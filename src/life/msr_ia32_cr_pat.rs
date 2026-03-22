#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    wc_regions: u16,
    uc_regions: u16,
    wb_dominance: u16,
    pat_config_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    wc_regions: 0,
    uc_regions: 0,
    wb_dominance: 0,
    pat_config_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_cr_pat] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x277u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // PAT has 8 entries (PA0-PA7), each 3 bits wide
    // PA0-PA3 in lo (bits 2:0, 10:8, 18:16, 26:24)
    // PA4-PA7 in hi (same layout)
    // Types: 0=UC, 1=WC, 4=WT, 5=WP, 6=WB, 7=UC-
    let mut wc_count = 0u32;
    let mut uc_count = 0u32;
    let mut wb_count = 0u32;

    for i in 0u32..4 {
        let shift = i * 8;
        let lo_field = (lo >> shift) & 0x7;
        let hi_field = (hi >> shift) & 0x7;
        for field in [lo_field, hi_field] {
            match field {
                1 => wc_count += 1,
                0 | 7 => uc_count += 1,
                6 => wb_count += 1,
                _ => {}
            }
        }
    }

    let wc_regions  = ((wc_count * 1000) / 8).min(1000) as u16;
    let uc_regions  = ((uc_count * 1000) / 8).min(1000) as u16;
    let wb_dominance = ((wb_count * 1000) / 8).min(1000) as u16;

    let composite = (wb_dominance as u32 / 2)
        .saturating_add(wc_regions as u32 / 4)
        .saturating_add(1000u32.saturating_sub(uc_regions as u32) / 4);

    let mut s = MODULE.lock();
    let pat_config_ema = ((s.pat_config_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.wc_regions = wc_regions;
    s.uc_regions = uc_regions;
    s.wb_dominance = wb_dominance;
    s.pat_config_ema = pat_config_ema;

    serial_println!("[msr_ia32_cr_pat] age={} wc={} uc={} wb_dom={} ema={}",
        age, wc_regions, uc_regions, wb_dominance, pat_config_ema);
}

pub fn get_wc_regions()     -> u16 { MODULE.lock().wc_regions }
pub fn get_uc_regions()     -> u16 { MODULE.lock().uc_regions }
pub fn get_wb_dominance()   -> u16 { MODULE.lock().wb_dominance }
pub fn get_pat_config_ema() -> u16 { MODULE.lock().pat_config_ema }
