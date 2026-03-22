#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_MC3_MISC MSR address: 0x400 + 4*bank + 3 = 0x400 + 12 + 3 = 0x40B
const MC3_MISC_ADDR: u32 = 0x40B;

// Tick gate — sample every 3000 ticks
const TICK_GATE: u32 = 3000;

struct State {
    mc3_addr_mode:      u16,  // bits[5:0] of lo, scaled 0-1000
    mc3_misc_lo_sense:  u16,  // bits[31:16] of lo, scaled 0-1000
    mc3_misc_hi_sense:  u16,  // bits[15:0] of hi, scaled 0-1000
    mc3_misc_ema:       u16,  // EMA of composite signal
}

static MODULE: Mutex<State> = Mutex::new(State {
    mc3_addr_mode:     0,
    mc3_misc_lo_sense: 0,
    mc3_misc_hi_sense: 0,
    mc3_misc_ema:      0,
});

/// Check CPUID leaf 1 EDX bit 14 — MCA (Machine Check Architecture) present.
fn has_mca() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _,
            lateout("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 14) & 1 == 1
}

/// Scale a raw value into 0-1000 range given the maximum raw value.
/// Uses integer arithmetic only; result is clamped to 1000.
#[inline]
fn scale(raw: u32, max: u32) -> u16 {
    if max == 0 {
        return 0;
    }
    let scaled = raw.saturating_mul(1000) / max;
    scaled.min(1000) as u16
}

/// Apply EMA: ((old * 7) + new) / 8, all u32 intermediate, result clamped to u16 0-1000.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8)
        .min(1000) as u16
}

pub fn init() {
    serial_println!("[msr_ia32_mc3_misc] init — MSR {:#05x}", MC3_MISC_ADDR);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_mca() {
        serial_println!("[msr_ia32_mc3_misc] age={} MCA not present, skipping", age);
        return;
    }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MC3_MISC_ADDR,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // bits[5:0] of lo — address mode code (range 0-63)
    let addr_mode_raw: u32 = (lo & 0x3F) as u32;
    let mc3_addr_mode = scale(addr_mode_raw, 63);

    // bits[31:16] of lo — model-specific misc data (range 0-65535)
    let misc_lo_raw: u32 = ((lo >> 16) & 0xFFFF) as u32;
    let mc3_misc_lo_sense = scale(misc_lo_raw, 65535);

    // bits[15:0] of hi — model-specific misc data (range 0-65535)
    let misc_hi_raw: u32 = (hi & 0xFFFF) as u32;
    let mc3_misc_hi_sense = scale(misc_hi_raw, 65535);

    // Composite: addr_mode/4 + misc_lo/4 + misc_hi/2 (weights sum to 1.0)
    let composite: u16 = (mc3_addr_mode / 4)
        .saturating_add(mc3_misc_lo_sense / 4)
        .saturating_add(mc3_misc_hi_sense / 2)
        .min(1000);

    let mut s = MODULE.lock();
    let new_ema = ema(s.mc3_misc_ema, composite);

    s.mc3_addr_mode     = mc3_addr_mode;
    s.mc3_misc_lo_sense = mc3_misc_lo_sense;
    s.mc3_misc_hi_sense = mc3_misc_hi_sense;
    s.mc3_misc_ema      = new_ema;

    serial_println!(
        "[msr_ia32_mc3_misc] age={} lo={:#010x} hi={:#010x} addr_mode={} lo_sense={} hi_sense={} ema={}",
        age, lo, hi,
        mc3_addr_mode, mc3_misc_lo_sense, mc3_misc_hi_sense, new_ema
    );
}

pub fn get_mc3_addr_mode()      -> u16 { MODULE.lock().mc3_addr_mode }
pub fn get_mc3_misc_lo_sense()  -> u16 { MODULE.lock().mc3_misc_lo_sense }
pub fn get_mc3_misc_hi_sense()  -> u16 { MODULE.lock().mc3_misc_hi_sense }
pub fn get_mc3_misc_ema()       -> u16 { MODULE.lock().mc3_misc_ema }
