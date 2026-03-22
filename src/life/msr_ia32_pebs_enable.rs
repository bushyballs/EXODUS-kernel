#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    pebs_pmc_count:   u16, // popcount(bits[3:0]) * 250, capped 1000
    pebs_fixed_en:    u16, // bit 0 of edx (FixedCtr0 PEBS): 0 or 1000
    pebs_density:     u16, // EMA of pebs_pmc_count
    pebs_config_ema:  u16, // EMA of composite (pebs_pmc_count/2 + pebs_fixed_en/2)
}

static MODULE: Mutex<State> = Mutex::new(State {
    pebs_pmc_count:  0,
    pebs_fixed_en:   0,
    pebs_density:    0,
    pebs_config_ema: 0,
});

// ---------------------------------------------------------------------------
// CPUID guard — PDCM: leaf 1, ECX bit 15
// ---------------------------------------------------------------------------

fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_val,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 == 1
}

// ---------------------------------------------------------------------------
// MSR read — IA32_PEBS_ENABLE (0x3F1)
// Returns (lo, hi): lo holds bits[3:0] PMC enables, hi bit 0 = FixedCtr0 PEBS
// ---------------------------------------------------------------------------

fn read_msr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3F1u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

// EMA: (old * 7 + new) / 8, saturating, capped 1000
fn ema(old: u16, new: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16).min(1000)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_pdcm() {
        serial_println!("[msr_ia32_pebs_enable] PDCM not supported — module disabled");
        return;
    }
    {
        let mut s = MODULE.lock();
        s.pebs_pmc_count  = 0;
        s.pebs_fixed_en   = 0;
        s.pebs_density    = 0;
        s.pebs_config_ema = 0;
    }
    serial_println!("[msr_ia32_pebs_enable] init ok (PDCM present)");
}

pub fn tick(age: u32) {
    if age % 2500 != 0 {
        return;
    }
    if !has_pdcm() {
        return;
    }

    let (lo, hi) = read_msr();

    // bits[3:0] = PEBS enable for PMC0-3
    let pmc_bits = lo & 0xF;
    let pc = popcount(pmc_bits);
    let pebs_pmc_count: u16 = ((pc * 250) as u16).min(1000);

    // bit 32 of the 64-bit MSR = bit 0 of edx (hi word)
    let pebs_fixed_en: u16 = if hi & 1 != 0 { 1000 } else { 0 };

    let mut s = MODULE.lock();

    let pebs_density    = ema(s.pebs_density,    pebs_pmc_count);
    let composite: u16  = pebs_pmc_count.saturating_add(pebs_fixed_en) / 2;
    let pebs_config_ema = ema(s.pebs_config_ema, composite);

    s.pebs_pmc_count  = pebs_pmc_count;
    s.pebs_fixed_en   = pebs_fixed_en;
    s.pebs_density    = pebs_density;
    s.pebs_config_ema = pebs_config_ema;

    serial_println!(
        "[msr_ia32_pebs_enable] age={} lo={:#010x} hi={:#010x} pmc_count={} fixed_en={} density={} cfg_ema={}",
        age, lo, hi, pebs_pmc_count, pebs_fixed_en, pebs_density, pebs_config_ema
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_pebs_pmc_count()   -> u16 { MODULE.lock().pebs_pmc_count }
pub fn get_pebs_fixed_en()    -> u16 { MODULE.lock().pebs_fixed_en }
pub fn get_pebs_density()     -> u16 { MODULE.lock().pebs_density }
pub fn get_pebs_config_ema()  -> u16 { MODULE.lock().pebs_config_ema }
