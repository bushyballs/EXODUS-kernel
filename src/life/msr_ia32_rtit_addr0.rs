#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

/// MSR 0x580 (ADDR0_A) + 0x581 (ADDR0_B) — Intel PT Address Filter Range 0
/// Sense: whether ANIMA has scoped her introspection to a specific code range.
/// Signals: range_active, range_nonzero, range_span_sense, addr_ema
struct State {
    range_active: u16,
    range_nonzero: u16,
    range_span_sense: u16,
    addr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    range_active: 0,
    range_nonzero: 0,
    range_span_sense: 0,
    addr_ema: 0,
});

#[inline]
fn pt_supported() -> bool {
    // Check max basic leaf >= 0x14
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
    // Leaf 0x14 sub-leaf 0: EAX max sub-leaf > 0 means PT supported
    let leaf14_eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x14u32 => leaf14_eax,
            in("ecx") 0u32,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    leaf14_eax != 0
}

pub fn init() { serial_println!("[msr_ia32_rtit_addr0] init"); }

pub fn tick(age: u32) {
    if age % 3500 != 0 { return; }
    if !pt_supported() { return; }

    let addr_a_lo: u32; let addr_a_hi: u32;
    let addr_b_lo: u32; let addr_b_hi: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x580u32, out("eax") addr_a_lo, out("edx") addr_a_hi, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x581u32, out("eax") addr_b_lo, out("edx") addr_b_hi, options(nostack, nomem));
    }

    // Non-zero check on either register
    let range_nonzero: u16 = if addr_a_lo != 0 || addr_a_hi != 0 || addr_b_lo != 0 || addr_b_hi != 0 {
        1000
    } else {
        0
    };

    // Active: both A and B set (a filter range requires both endpoints)
    let range_active: u16 = if range_nonzero == 1000 && (addr_b_lo != 0 || addr_b_hi != 0) {
        1000
    } else {
        0
    };

    // Span sense: upper bits of addr_b (bits 47:32 = addr_b_hi & 0xFFFF) normalized to 0-1000
    let span_raw = (addr_b_hi & 0xFFFF) as u32;
    let range_span_sense = ((span_raw * 1000) / 0xFFFF).min(1000) as u16;

    let composite: u16 = (range_active / 4)
        .saturating_add(range_nonzero / 4)
        .saturating_add(range_span_sense / 2);

    let mut s = MODULE.lock();
    let ema = ((s.addr_ema as u32).wrapping_mul(7).saturating_add(composite as u32) / 8)
        .min(1000) as u16;
    s.range_active = range_active;
    s.range_nonzero = range_nonzero;
    s.range_span_sense = range_span_sense;
    s.addr_ema = ema;

    serial_println!("[msr_ia32_rtit_addr0] age={} active={} nonzero={} span={} ema={}",
        age, range_active, range_nonzero, range_span_sense, ema);
}

pub fn get_range_active() -> u16 { MODULE.lock().range_active }
pub fn get_range_nonzero() -> u16 { MODULE.lock().range_nonzero }
pub fn get_range_span_sense() -> u16 { MODULE.lock().range_span_sense }
pub fn get_addr_ema() -> u16 { MODULE.lock().addr_ema }
