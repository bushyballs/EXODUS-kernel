#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct LbrInfo0State {
    lbr_info0_mispred:   u16,
    lbr_info0_cycles:    u16,
    lbr_info0_hi_sense:  u16,
    lbr_info0_ema:       u16,
}

impl LbrInfo0State {
    const fn new() -> Self {
        Self {
            lbr_info0_mispred:  0,
            lbr_info0_cycles:   0,
            lbr_info0_hi_sense: 0,
            lbr_info0_ema:      0,
        }
    }
}

static STATE: Mutex<LbrInfo0State> = Mutex::new(LbrInfo0State::new());

// ---------------------------------------------------------------------------
// CPUID guard — PDCM: CPUID leaf 1 ECX bit 15
// ---------------------------------------------------------------------------

fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 != 0
}

// ---------------------------------------------------------------------------
// MSR read — MSR_LBR_INFO_0 = 0xDC0
// ---------------------------------------------------------------------------

#[inline]
fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            lateout("eax") lo,
            lateout("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_pdcm() {
        crate::serial_println!(
            "[msr_ia32_lbr_info0] PDCM not supported — module disabled"
        );
        return;
    }
    crate::serial_println!("[msr_ia32_lbr_info0] init ok (PDCM present)");
}

pub fn tick(age: u32) {
    // Sample every 300 ticks
    if age % 300 != 0 {
        return;
    }

    if !has_pdcm() {
        return;
    }

    // Read MSR_LBR_INFO_0 (0xDC0)
    let raw = rdmsr(0xDC0);
    let lo = (raw & 0xFFFF_FFFF) as u32;
    let hi = ((raw >> 32) & 0xFFFF_FFFF) as u32;

    // bit 63 of full 64-bit value = MISPRED = bit 31 of hi
    let mispred: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };

    // bits [15:0] of lo = cycle count, normalised to 0-1000
    let raw_cycles = (lo & 0xFFFF) as u32;
    let cycles: u16 = (raw_cycles * 1000 / 65536) as u16;

    // bits [30:0] of hi (excluding bit 31), lower 15 bits used for hi_sense
    let raw_hi = (hi & 0x7FFF) as u32;
    let hi_sense: u16 = (raw_hi * 1000 / 32768) as u16;

    // EMA composite: mispred/4 + cycles/4 + hi_sense/2
    let composite: u16 = ((mispred as u32) / 4
        + (cycles as u32) / 4
        + (hi_sense as u32) / 2) as u16;

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32, cast to u16
    let new_ema: u16 = ((s.lbr_info0_ema as u32 * 7 + composite as u32) / 8) as u16;

    s.lbr_info0_mispred  = mispred;
    s.lbr_info0_cycles   = cycles;
    s.lbr_info0_hi_sense = hi_sense;
    s.lbr_info0_ema      = new_ema;

    crate::serial_println!(
        "[msr_ia32_lbr_info0] age={} mispred={} cycles={} hi={} ema={}",
        age, mispred, cycles, hi_sense, new_ema
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_lbr_info0_mispred() -> u16 {
    STATE.lock().lbr_info0_mispred
}

pub fn get_lbr_info0_cycles() -> u16 {
    STATE.lock().lbr_info0_cycles
}

pub fn get_lbr_info0_hi_sense() -> u16 {
    STATE.lock().lbr_info0_hi_sense
}

pub fn get_lbr_info0_ema() -> u16 {
    STATE.lock().lbr_info0_ema
}
