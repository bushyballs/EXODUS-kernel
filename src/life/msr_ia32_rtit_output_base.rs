#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct MsrIa32RtitOutputBaseState {
    output_base_active:    u16,
    output_base_hi:        u16,
    output_mask_lo_sense:  u16,
    output_ema:            u16,
}

impl MsrIa32RtitOutputBaseState {
    const fn new() -> Self {
        Self {
            output_base_active:   0,
            output_base_hi:       0,
            output_mask_lo_sense: 0,
            output_ema:           0,
        }
    }
}

static STATE: Mutex<MsrIa32RtitOutputBaseState> =
    Mutex::new(MsrIa32RtitOutputBaseState::new());

// ── PT Guard ──────────────────────────────────────────────────────────────────

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
    let leaf14_eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x14u32 => leaf14_eax,
            in("ecx") 0u32,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    leaf14_eax != 0
}

// ── RDMSR helper ─────────────────────────────────────────────────────────────

/// Reads the low 32 bits of an MSR (EDX is discarded — only the low half
/// is needed for both 0x560 and 0x561 signal extraction).
fn rdmsr_lo(msr: u32) -> u32 {
    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    lo
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Map a u32 value linearly into 0–1000 given a known max.
/// Uses integer arithmetic only.
#[inline(always)]
fn scale_to_1000(value: u32, max: u32) -> u16 {
    if max == 0 {
        return 0;
    }
    let scaled = (value as u64 * 1000u64) / max as u64;
    if scaled > 1000 { 1000u16 } else { scaled as u16 }
}

/// EMA: (old * 7 + new_val) / 8 — computed in u32, cast to u16.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    let result = (old as u32 * 7 + new_val as u32) / 8;
    result as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.output_base_active   = 0;
    s.output_base_hi       = 0;
    s.output_mask_lo_sense = 0;
    s.output_ema           = 0;
    crate::serial_println!(
        "[msr_ia32_rtit_output_base] init: PT={} output_base=0x560 mask_ptrs=0x561",
        has_pt()
    );
}

pub fn tick(age: u32) {
    // Sampling gate: every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // PT guard — all signals stay zero if PT is not supported
    if !has_pt() {
        crate::serial_println!(
            "[msr_ia32_rtit_output_base] age={} PT not supported — signals zeroed",
            age
        );
        return;
    }

    // ── Read MSRs ─────────────────────────────────────────────────────────────
    // IA32_RTIT_OUTPUT_BASE (0x560): physical base address of the PT output
    // buffer.  Bits [31:7] are significant (128-byte aligned).
    let output_base_lo: u32 = rdmsr_lo(0x560);

    // IA32_RTIT_OUTPUT_MASK_PTRS (0x561): bits[31:7] = mask, bits[6:3] = tail
    // pointer upper bits.
    let output_mask_lo: u32 = rdmsr_lo(0x561);

    // ── Derive signals ────────────────────────────────────────────────────────

    // output_base_active: 1000 if the buffer is configured (base != 0), else 0
    let new_active: u16 = if output_base_lo != 0 { 1000 } else { 0 };

    // output_base_hi: bits [31:16] of output_base_lo mapped to 0–1000
    let base_hi_raw: u32 = (output_base_lo >> 16) & 0xFFFF;
    let new_base_hi: u16 = scale_to_1000(base_hi_raw, 0xFFFF);

    // output_mask_lo_sense: bits [15:0] of output_mask_lo mapped to 0–1000
    let mask_lo_raw: u32 = output_mask_lo & 0xFFFF;
    let new_mask_lo_sense: u16 = scale_to_1000(mask_lo_raw, 0xFFFF);

    // ── Update state ──────────────────────────────────────────────────────────
    let mut s = STATE.lock();

    s.output_base_active   = new_active;
    s.output_base_hi       = new_base_hi;
    s.output_mask_lo_sense = new_mask_lo_sense;
    s.output_ema           = ema(s.output_ema, new_active);

    crate::serial_println!(
        "[msr_ia32_rtit_output_base] age={} active={} base_hi={} mask_lo={} ema={}",
        age,
        s.output_base_active,
        s.output_base_hi,
        s.output_mask_lo_sense,
        s.output_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_output_base_active() -> u16 {
    STATE.lock().output_base_active
}

pub fn get_output_base_hi() -> u16 {
    STATE.lock().output_base_hi
}

pub fn get_output_mask_lo_sense() -> u16 {
    STATE.lock().output_mask_lo_sense
}

pub fn get_output_ema() -> u16 {
    STATE.lock().output_ema
}
