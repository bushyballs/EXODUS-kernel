#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_MBA_THRTL_MSR_0: u32 = 0xD50;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    supported:          bool,
    mba_delay:          u16,
    mba_linear:         u16,
    mba_throttle_sense: u16,
    mba_ema:            u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    supported:          false,
    mba_delay:          0,
    mba_linear:         0,
    mba_throttle_sense: 0,
    mba_ema:            0,
});

// ── EMA ───────────────────────────────────────────────────────────────────────

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_mba() -> bool {
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
    if max_leaf < 0x10 { return false; }
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "mov {out:e}, ebx", "pop rbx",
            inout("eax") 0x10u32 => _,
            out = out(reg) ebx,
            in("ecx") 3u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    ebx != 0
}

// ── RDMSR ─────────────────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = MODULE.lock();
    s.supported          = has_mba();
    s.mba_delay          = 0;
    s.mba_linear         = 0;
    s.mba_throttle_sense = 0;
    s.mba_ema            = 0;
    serial_println!(
        "[msr_ia32_mba_thrtl] init: mba_supported={}",
        s.supported
    );
}

pub fn tick(age: u32) {
    if age % 11000 != 0 { return; }

    let mut s = MODULE.lock();

    if !s.supported {
        serial_println!(
            "[msr_ia32_mba_thrtl] tick age={} MBA not supported, skipping",
            age
        );
        return;
    }

    let (lo, _hi) = unsafe { rdmsr(IA32_MBA_THRTL_MSR_0) };

    // mba_linear: bit 31 of lo — 1000 if set, else 0
    let linear_bit = (lo >> 31) & 1;
    let mba_linear: u16 = if linear_bit != 0 { 1000 } else { 0 };

    // mba_delay: bits[11:0] of lo, scaled (delay * 1000 / 90).min(1000)
    let delay_raw = lo & 0xFFF;
    let mba_delay: u16 = ((delay_raw * 1000 / 90) as u16).min(1000);

    // mba_throttle_sense:
    //   if linear mode: invert bandwidth % — (100 - (lo & 0x7F)) * 10, min 1000
    //   if delay mode:  mba_delay (delay already maps to throttle intensity)
    let mba_throttle_sense: u16 = if mba_linear != 0 {
        let bw_pct = lo & 0x7F;           // bandwidth percentage (0-100)
        let inverted = (100u32).saturating_sub(bw_pct) * 10;
        inverted.min(1000) as u16
    } else {
        mba_delay
    };

    // mba_ema: EMA of mba_throttle_sense
    let mba_ema = ema(s.mba_ema, mba_throttle_sense);

    s.mba_delay          = mba_delay;
    s.mba_linear         = mba_linear;
    s.mba_throttle_sense = mba_throttle_sense;
    s.mba_ema            = mba_ema;

    serial_println!(
        "[msr_ia32_mba_thrtl] tick age={} msr_lo=0x{:08x} delay_raw={} mba_delay={} mba_linear={} throttle_sense={} mba_ema={}",
        age,
        lo,
        delay_raw,
        mba_delay,
        mba_linear,
        mba_throttle_sense,
        mba_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_mba_delay() -> u16 {
    MODULE.lock().mba_delay
}

pub fn get_mba_linear() -> u16 {
    MODULE.lock().mba_linear
}

pub fn get_mba_throttle_sense() -> u16 {
    MODULE.lock().mba_throttle_sense
}

pub fn get_mba_ema() -> u16 {
    MODULE.lock().mba_ema
}
