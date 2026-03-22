#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── PT capability guard ───────────────────────────────────────────────────────

fn has_pt() -> bool {
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
    if max_leaf < 0x14 {
        return false;
    }
    let leaf14: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x14u32 => leaf14,
            in("ecx") 0u32,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    leaf14 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_RTIT_CR3_MATCH (MSR 0x572).
/// Returns (lo, hi) — low 32 bits and high 32 bits.
/// Bits[63:5] hold the CR3 match value; bits[4:0] are reserved and always 0.
unsafe fn rdmsr_rtit_cr3_match() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x572u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── State ─────────────────────────────────────────────────────────────────────

struct Cr3MatchState {
    /// 1000 if CR3 filtering is active (lo != 0), else 0
    cr3_filter_set: u16,
    /// bits[31:16] of lo mapped to 0–1000
    cr3_lo_sense: u16,
    /// bits[15:0] of lo mapped to 0–1000
    cr3_mid_sense: u16,
    /// EMA of cr3_filter_set
    cr3_filter_ema: u16,
}

impl Cr3MatchState {
    const fn new() -> Self {
        Self {
            cr3_filter_set: 0,
            cr3_lo_sense: 0,
            cr3_mid_sense: 0,
            cr3_filter_ema: 0,
        }
    }
}

static STATE: Mutex<Cr3MatchState> = Mutex::new(Cr3MatchState::new());

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Map a u16 value (0–0xFFFF) to 0–1000 using fixed-point arithmetic.
#[inline]
fn map_u16_to_1000(raw: u16) -> u16 {
    // (raw as u32 * 1000) / 65535
    let scaled = (raw as u32 * 1000) / 65535;
    scaled as u16
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32) * 7 + (new_val as u32)) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.cr3_filter_set = 0;
    s.cr3_lo_sense = 0;
    s.cr3_mid_sense = 0;
    s.cr3_filter_ema = 0;
    crate::serial_println!(
        "[msr_ia32_rtit_cr3_match] init: IA32_RTIT_CR3_MATCH MSR 0x572 module ready"
    );
}

pub fn tick(age: u32) {
    // Sampling gate: every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    // PT guard
    if !has_pt() {
        return;
    }

    // Read MSR 0x572
    let lo: u32 = unsafe { rdmsr_rtit_cr3_match().0 };

    // cr3_filter_set: is CR3 filtering active?
    let cr3_filter_set: u16 = if lo != 0 { 1000 } else { 0 };

    // cr3_lo_sense: bits[31:16] of lo mapped to 0–1000
    let bits_hi16 = ((lo >> 16) & 0xFFFF) as u16;
    let cr3_lo_sense: u16 = map_u16_to_1000(bits_hi16);

    // cr3_mid_sense: bits[15:0] of lo mapped to 0–1000
    let bits_lo16 = (lo & 0xFFFF) as u16;
    let cr3_mid_sense: u16 = map_u16_to_1000(bits_lo16);

    let mut s = STATE.lock();

    // EMA of cr3_filter_set
    let cr3_filter_ema = ema(s.cr3_filter_ema, cr3_filter_set);

    s.cr3_filter_set = cr3_filter_set;
    s.cr3_lo_sense = cr3_lo_sense;
    s.cr3_mid_sense = cr3_mid_sense;
    s.cr3_filter_ema = cr3_filter_ema;

    crate::serial_println!(
        "[msr_ia32_rtit_cr3_match] age={} filter={} lo={} mid={} ema={}",
        age,
        cr3_filter_set,
        cr3_lo_sense,
        cr3_mid_sense,
        cr3_filter_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_cr3_filter_set() -> u16 {
    STATE.lock().cr3_filter_set
}

pub fn get_cr3_lo_sense() -> u16 {
    STATE.lock().cr3_lo_sense
}

pub fn get_cr3_mid_sense() -> u16 {
    STATE.lock().cr3_mid_sense
}

pub fn get_cr3_filter_ema() -> u16 {
    STATE.lock().cr3_filter_ema
}
