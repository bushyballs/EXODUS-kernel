#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct RdtAllocState {
    l3_cat:       u16,
    l2_cat:       u16,
    mba_supported: u16,
    rdt_alloc_ema: u16,
}

static STATE: Mutex<RdtAllocState> = Mutex::new(RdtAllocState {
    l3_cat:        0,
    l2_cat:        0,
    mba_supported: 0,
    rdt_alloc_ema: 0,
});

// ── CPUID helpers ────────────────────────────────────────────────────────────

fn has_rdt_alloc() -> bool {
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
    edx_10 != 0
}

/// Read CPUID leaf 0x10, sub-leaf 0, and return EDX.
fn read_leaf10_subleaf0_edx() -> u32 {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x10u32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx,
            options(nostack, nomem)
        );
    }
    edx
}

// ── Signal computation ───────────────────────────────────────────────────────

fn compute_signals() -> (u16, u16, u16) {
    let edx = read_leaf10_subleaf0_edx();

    // bit 1 = L3 CAT, bit 2 = L2 CAT, bit 3 = MBA
    let l3_cat:        u16 = if (edx >> 1) & 1 != 0 { 1000 } else { 0 };
    let l2_cat:        u16 = if (edx >> 2) & 1 != 0 { 1000 } else { 0 };
    let mba_supported: u16 = if (edx >> 3) & 1 != 0 { 1000 } else { 0 };

    (l3_cat, l2_cat, mba_supported)
}

/// Composite score: l3_cat/4 + l2_cat/4 + mba_supported/2  (integer, 0–750)
fn composite(l3: u16, l2: u16, mba: u16) -> u16 {
    let score = (l3 as u32) / 4 + (l2 as u32) / 4 + (mba as u32) / 2;
    score.min(1000) as u16
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let v = ((old as u32) * 7 + (new_val as u32)) / 8;
    v as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_rdt_alloc() {
        crate::serial_println!(
            "[cpuid_rdt_allocation] CPUID leaf 0x10 not supported or no allocation types present — module inactive"
        );
        return;
    }

    let (l3, l2, mba) = compute_signals();
    let score = composite(l3, l2, mba);

    let mut s = STATE.lock();
    s.l3_cat        = l3;
    s.l2_cat        = l2;
    s.mba_supported = mba;
    s.rdt_alloc_ema = score; // seed EMA from first reading

    crate::serial_println!(
        "[cpuid_rdt_allocation] init: l3_cat={} l2_cat={} mba={} ema={}",
        l3, l2, mba, score
    );
}

pub fn tick(age: u32) {
    // Sampling gate: run every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    if !has_rdt_alloc() {
        return;
    }

    let (l3, l2, mba) = compute_signals();
    let score = composite(l3, l2, mba);

    let mut s = STATE.lock();
    s.l3_cat        = l3;
    s.l2_cat        = l2;
    s.mba_supported = mba;
    s.rdt_alloc_ema = ema(s.rdt_alloc_ema, score);

    let cur_ema = s.rdt_alloc_ema;

    crate::serial_println!(
        "[cpuid_rdt_allocation] age={} l3_cat={} l2_cat={} mba={} ema={}",
        age, l3, l2, mba, cur_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_l3_cat() -> u16 {
    STATE.lock().l3_cat
}

pub fn get_l2_cat() -> u16 {
    STATE.lock().l2_cat
}

pub fn get_mba_supported() -> u16 {
    STATE.lock().mba_supported
}

pub fn get_rdt_alloc_ema() -> u16 {
    STATE.lock().rdt_alloc_ema
}
