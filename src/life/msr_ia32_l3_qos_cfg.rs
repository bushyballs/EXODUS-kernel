#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct L3QosCfgState {
    cdp_enabled:      u16,
    l3_cfg_lo_sense:  u16,
    l3_cfg_hi_sense:  u16,
    l3_qos_ema:       u16,
}

static STATE: Mutex<L3QosCfgState> = Mutex::new(L3QosCfgState {
    cdp_enabled:      0,
    l3_cfg_lo_sense:  0,
    l3_cfg_hi_sense:  0,
    l3_qos_ema:       0,
});

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_l3_cat() -> bool {
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
    if max_leaf < 0x10 {
        return false;
    }
    let edx_10: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x10u32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_10,
            options(nostack, nomem)
        );
    }
    (edx_10 >> 1) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read MSR 0xC81 (IA32_L3_QOS_CFG).
/// Returns (lo32, hi32).
unsafe fn rdmsr_c81() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0xC81u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Cap helper ────────────────────────────────────────────────────────────────

#[inline(always)]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.cdp_enabled     = 0;
    s.l3_cfg_lo_sense = 0;
    s.l3_cfg_hi_sense = 0;
    s.l3_qos_ema      = 0;
    crate::serial_println!("[msr_ia32_l3_qos_cfg] init: L3 CAT supported={}", has_l3_cat());
}

pub fn tick(age: u32) {
    // Sample every 8000 ticks
    if age % 8000 != 0 {
        return;
    }

    if !has_l3_cat() {
        return;
    }

    let (lo, _hi) = unsafe { rdmsr_c81() };

    // Bit 0 → CDP enable: 0 or 1000
    let cdp_enabled: u16 = if lo & 1 != 0 { 1000 } else { 0 };

    // bits[7:1] × 8, capped at 1000
    let lo_bits: u32 = ((lo >> 1) & 0x7F) * 8;
    let l3_cfg_lo_sense: u16 = cap1000(lo_bits);

    // bits[15:8] × 4, capped at 1000
    let hi_bits: u32 = ((lo >> 8) & 0xFF) * 4;
    let l3_cfg_hi_sense: u16 = cap1000(hi_bits);

    let mut s = STATE.lock();

    // EMA of cdp_enabled
    let l3_qos_ema = ema8(s.l3_qos_ema, cdp_enabled);

    s.cdp_enabled     = cdp_enabled;
    s.l3_cfg_lo_sense = l3_cfg_lo_sense;
    s.l3_cfg_hi_sense = l3_cfg_hi_sense;
    s.l3_qos_ema      = l3_qos_ema;

    crate::serial_println!(
        "[msr_ia32_l3_qos_cfg] age={} cdp={} lo={} hi={} ema={}",
        age,
        cdp_enabled,
        l3_cfg_lo_sense,
        l3_cfg_hi_sense,
        l3_qos_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_cdp_enabled() -> u16 {
    STATE.lock().cdp_enabled
}

pub fn get_l3_cfg_lo_sense() -> u16 {
    STATE.lock().l3_cfg_lo_sense
}

pub fn get_l3_cfg_hi_sense() -> u16 {
    STATE.lock().l3_cfg_hi_sense
}

pub fn get_l3_qos_ema() -> u16 {
    STATE.lock().l3_qos_ema
}
