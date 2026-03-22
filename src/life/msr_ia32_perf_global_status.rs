#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pmc_overflow: u16,
    fixed_overflow: u16,
    ds_buf_ovf: u16,
    perf_ovf_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pmc_overflow: 0,
    fixed_overflow: 0,
    ds_buf_ovf: 0,
    perf_ovf_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_perf_global_status] init"); }

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Eu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // bits[N:0]: PMC counter overflow flags
    let pmc_overflow: u16 = if lo & 0xFF != 0 { 1000 } else { 0 };
    // hi bits[2:0]: Fixed counter overflow
    let fixed_overflow: u16 = if hi & 7 != 0 { 1000 } else { 0 };
    // hi bit 30: DS buffer (PEBS/BTS) overflow
    let ds_buf_ovf: u16 = if (hi >> 30) & 1 != 0 { 1000 } else { 0 };

    let composite = (pmc_overflow as u32 / 3)
        .saturating_add(fixed_overflow as u32 / 3)
        .saturating_add(ds_buf_ovf as u32 / 3);

    let mut s = MODULE.lock();
    let perf_ovf_ema = ((s.perf_ovf_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.pmc_overflow = pmc_overflow;
    s.fixed_overflow = fixed_overflow;
    s.ds_buf_ovf = ds_buf_ovf;
    s.perf_ovf_ema = perf_ovf_ema;

    serial_println!("[msr_ia32_perf_global_status] age={} pmc_ovf={} fix_ovf={} ds_ovf={} ema={}",
        age, pmc_overflow, fixed_overflow, ds_buf_ovf, perf_ovf_ema);
}

pub fn get_pmc_overflow()   -> u16 { MODULE.lock().pmc_overflow }
pub fn get_fixed_overflow() -> u16 { MODULE.lock().fixed_overflow }
pub fn get_ds_buf_ovf()     -> u16 { MODULE.lock().ds_buf_ovf }
pub fn get_perf_ovf_ema()   -> u16 { MODULE.lock().perf_ovf_ema }
