#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_lbr_v2() -> bool {
    let ecx1: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx1,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if (ecx1 >> 15) & 1 == 0 {
        return false;
    }
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x1C {
        return false;
    }
    let eax_1c: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x1Cu32 => eax_1c,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    eax_1c != 0
}

// ── MSR read helper ──────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        lateout("eax") lo,
        lateout("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── State ────────────────────────────────────────────────────────────────────

struct LbrFrom4State {
    from4_hi:      u16,
    to4_hi:        u16,
    from8_hi:      u16,
    lbr_depth_ema: u16,
    supported:     bool,
    initialized:   bool,
}

impl LbrFrom4State {
    const fn new() -> Self {
        Self {
            from4_hi:      0,
            to4_hi:        0,
            from8_hi:      0,
            lbr_depth_ema: 0,
            supported:     false,
            initialized:   false,
        }
    }
}

static STATE: Mutex<LbrFrom4State> = Mutex::new(LbrFrom4State::new());

// MSR addresses
const MSR_LBR_FROM_4: u32 = 0x684;
const MSR_LBR_TO_4:   u32 = 0x6C4;
const MSR_LBR_FROM_8: u32 = 0x688;
// MSR_LBR_TO_8 (0x6C8) is read but not stored as a primary signal

const SAMPLE_INTERVAL: u32 = 250;

// ── Mapping helper ───────────────────────────────────────────────────────────
// bits[31:16] of the MSR low 32-bit word → 0-1000
// raw range is 0x0000..=0xFFFF (65535); scale: val * 1000 / 65535
// use 32-bit arithmetic throughout — no float, no overflow

#[inline]
fn bits31_16_to_signal(raw64: u64) -> u16 {
    // extract the low 32 bits, then bits [31:16]
    let lo32 = raw64 as u32;
    let bits = (lo32 >> 16) & 0xFFFF;
    // scale: (bits * 1000) / 65535
    let scaled = (bits as u32) * 1000 / 65535;
    scaled.min(1000) as u16
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline]
fn ema8(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32) * 7 + (new_val as u32)) / 8;
    result.min(1000) as u16
}

// ── Public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    if s.initialized {
        return;
    }
    s.supported   = has_lbr_v2();
    s.initialized = true;
    crate::serial_println!(
        "[msr_lbr_from4] init supported={}",
        s.supported
    );
}

pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        return;
    }

    if !s.supported {
        return;
    }

    // Read MSRs
    let raw_from4 = unsafe { rdmsr(MSR_LBR_FROM_4) };
    let raw_to4   = unsafe { rdmsr(MSR_LBR_TO_4) };
    let raw_from8 = unsafe { rdmsr(MSR_LBR_FROM_8) };
    // MSR_LBR_TO_8 read; not stored as primary signal
    let _raw_to8  = unsafe { rdmsr(0x6C8u32) };

    let from4_hi = bits31_16_to_signal(raw_from4);
    let to4_hi   = bits31_16_to_signal(raw_to4);
    let from8_hi = bits31_16_to_signal(raw_from8);

    // depth_ema input: from4_hi/4 + to4_hi/4 + from8_hi/2
    let depth_input = (from4_hi as u32) / 4
        + (to4_hi   as u32) / 4
        + (from8_hi as u32) / 2;
    let depth_input_u16 = depth_input.min(1000) as u16;

    let new_ema = ema8(s.lbr_depth_ema, depth_input_u16);

    s.from4_hi      = from4_hi;
    s.to4_hi        = to4_hi;
    s.from8_hi      = from8_hi;
    s.lbr_depth_ema = new_ema;

    crate::serial_println!(
        "[msr_lbr_from4] age={} from4={} to4={} from8={} ema={}",
        age,
        from4_hi,
        to4_hi,
        from8_hi,
        new_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_from4_hi() -> u16 {
    STATE.lock().from4_hi
}

pub fn get_to4_hi() -> u16 {
    STATE.lock().to4_hi
}

pub fn get_from8_hi() -> u16 {
    STATE.lock().from8_hi
}

pub fn get_lbr_depth_ema() -> u16 {
    STATE.lock().lbr_depth_ema
}
