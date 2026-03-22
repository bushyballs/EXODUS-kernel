#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    xss_supervisor_bits: u16,
    pt_trace_ss: u16,
    hdc_en: u16,
    xss_config_ema: u16,
    last_tick: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    xss_supervisor_bits: 0,
    pt_trace_ss: 0,
    hdc_en: 0,
    xss_config_ema: 0,
    last_tick: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

fn xsaves_supported() -> bool {
    let leaf_eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xDu32 => leaf_eax,
            in("ecx") 1u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (leaf_eax >> 3) & 1 == 1
}

fn read_ia32_xss() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x140u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.xss_supervisor_bits = 0;
    s.pt_trace_ss = 0;
    s.hdc_en = 0;
    s.xss_config_ema = 0;
    s.last_tick = 0;
    serial_println!("[msr_ia32_xss] init");
}

pub fn tick(age: u32) {
    let mut s = MODULE.lock();

    if age.wrapping_sub(s.last_tick) < 6000 {
        return;
    }
    s.last_tick = age;

    if !xsaves_supported() {
        serial_println!(
            "[msr_ia32_xss] tick age={} XSAVES not supported; signals zeroed",
            age
        );
        s.xss_supervisor_bits = 0;
        s.pt_trace_ss = 0;
        s.hdc_en = 0;
        s.xss_config_ema = ema(s.xss_config_ema, 0);
        return;
    }

    let lo = read_ia32_xss();

    // xss_supervisor_bits: popcount of bits[15:0], scaled *62, capped 1000
    let bits_lo = lo & 0xFFFF;
    let raw_count = popcount(bits_lo);
    let scaled = (raw_count * 62).min(1000);
    let xss_supervisor_bits = scaled as u16;

    // pt_trace_ss: bit 8
    let pt_trace_ss: u16 = if (lo >> 8) & 1 == 1 { 1000 } else { 0 };

    // hdc_en: bit 13
    let hdc_en: u16 = if (lo >> 13) & 1 == 1 { 1000 } else { 0 };

    // composite for EMA
    let composite = ((xss_supervisor_bits as u32) / 2)
        .saturating_add((pt_trace_ss as u32) / 4)
        .saturating_add((hdc_en as u32) / 4)
        .min(1000) as u16;

    let xss_config_ema = ema(s.xss_config_ema, composite);

    s.xss_supervisor_bits = xss_supervisor_bits;
    s.pt_trace_ss = pt_trace_ss;
    s.hdc_en = hdc_en;
    s.xss_config_ema = xss_config_ema;

    serial_println!(
        "[msr_ia32_xss] age={} lo=0x{:08x} sup_bits={} pt_ss={} hdc={} ema={}",
        age,
        lo,
        xss_supervisor_bits,
        pt_trace_ss,
        hdc_en,
        xss_config_ema,
    );
}
