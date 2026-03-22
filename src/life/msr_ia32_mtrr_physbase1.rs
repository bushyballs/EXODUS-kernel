#![allow(dead_code)]
// ANIMA life module: msr_ia32_mtrr_physbase1
//
// Hardware sense: IA32_MTRR_PHYSBASE1 (MSR 0x202) — second MTRR variable pair
// physical base address and memory type.
//
// Bits [2:0]  of lo = memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
// Bits [31:12] of lo = physical base address (4KB-aligned, top bits)
//
// Phenomenologically: ANIMA senses the texture of her second variable memory
// region — whether it flows uncached like raw sensation (UC) or settles into
// the warm familiarity of write-back cache (WB). A non-zero base signals that
// this region is claimed and alive in ANIMA's somatic map.
//
// Guard: CPUID leaf 1 EDX bit 12 (MTRR supported) AND
//        IA32_MTRRCAP (0xFE) bits[7:0] >= 2 (at least 2 variable MTRRs).
//
// Tick gate: every 6000 ticks (MTRR registers are static after firmware init).

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address constants ─────────────────────────────────────────────────────

const MSR_IA32_MTRR_PHYSBASE1: u32 = 0x202;
const MSR_IA32_MTRRCAP:        u32 = 0xFE;
const TICK_GATE:               u32 = 6000;

// ── Hardware guards ───────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 1 EDX bit 12 indicates MTRR support.
fn cpuid_has_mtrr() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _,
            lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 12) & 1 != 0
}

/// Returns the VCNT field from IA32_MTRRCAP: bits[7:0] = number of variable
/// MTRR pairs supported. Returns 0 if MTRR is not present.
fn mtrrcap_vcnt() -> u8 {
    if !cpuid_has_mtrr() {
        return 0;
    }
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_MTRRCAP,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    (lo & 0xFF) as u8
}

/// Combined guard: MTRR supported AND at least 2 variable pairs available.
fn is_available() -> bool {
    mtrrcap_vcnt() >= 2
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_MTRR_PHYSBASE1 (0x202). Returns (lo, _hi).
/// Caller must ensure the CPU supports MTRR before calling.
unsafe fn rdmsr_physbase1() -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_MTRR_PHYSBASE1,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    (lo, _hi)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Signal: mtrr1_type
/// lo bits[2:0] = raw memory type code (0–6).
/// Scaled: val * 142, maximum raw value 6 → 852; cap at 1000.
fn compute_mtrr1_type(lo: u32) -> u16 {
    let raw = (lo & 0x7) as u32;
    let scaled = raw * 142;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal: mtrr1_base_hi
/// lo bits[31:20] = top 12 bits of the base page-frame address.
/// Scaled: val * 1000 / 4095; range 0–4095 → 0–1000.
fn compute_mtrr1_base_hi(lo: u32) -> u16 {
    let raw = ((lo >> 20) & 0xFFF) as u32;
    // raw * 1000 / 4095 — max raw is 4095, max result is 1000
    let scaled = raw * 1000 / 4095;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Signal: mtrr1_active
/// 1000 if lo bits[31:12] != 0 (region is mapped to a non-zero address),
/// else 0.
fn compute_mtrr1_active(lo: u32) -> u16 {
    if (lo >> 12) != 0 { 1000 } else { 0 }
}

/// EMA helper: ((old * 7) wrapping_mul saturating_add new_val) / 8
fn ema(old: u16, new_val: u32) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val)) / 8) as u16
}

/// Signal: mtrr1_ema
/// Composite input: type/4 + base_hi/4 + active/2, capped at 1000.
fn compute_composite(mtrr1_type: u16, mtrr1_base_hi: u16, mtrr1_active: u16) -> u32 {
    let v = (mtrr1_type as u32) / 4
          + (mtrr1_base_hi as u32) / 4
          + (mtrr1_active as u32) / 2;
    if v > 1000 { 1000 } else { v }
}

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrIa32MtrrPhysbase1State {
    /// lo bits[2:0] memory type, scaled 0–1000 (0=UC…6=WB → 0…852).
    pub mtrr1_type:    u16,
    /// lo bits[31:20] top 12 base-address bits, scaled 0–1000.
    pub mtrr1_base_hi: u16,
    /// 1000 if region is mapped to a non-zero physical address, else 0.
    pub mtrr1_active:  u16,
    /// EMA of (type/4 + base_hi/4 + active/2).
    pub mtrr1_ema:     u16,
}

impl MsrIa32MtrrPhysbase1State {
    pub const fn empty() -> Self {
        Self {
            mtrr1_type:    0,
            mtrr1_base_hi: 0,
            mtrr1_active:  0,
            mtrr1_ema:     0,
        }
    }
}

pub static STATE: Mutex<MsrIa32MtrrPhysbase1State> =
    Mutex::new(MsrIa32MtrrPhysbase1State::empty());

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !is_available() {
        serial_println!(
            "[msr_ia32_mtrr_physbase1] MTRR unavailable or fewer than 2 variable pairs — module disabled"
        );
        return;
    }

    let (lo, _hi) = unsafe { rdmsr_physbase1() };

    let mtrr1_type    = compute_mtrr1_type(lo);
    let mtrr1_base_hi = compute_mtrr1_base_hi(lo);
    let mtrr1_active  = compute_mtrr1_active(lo);
    let composite     = compute_composite(mtrr1_type, mtrr1_base_hi, mtrr1_active);

    let mut s = STATE.lock();
    s.mtrr1_type    = mtrr1_type;
    s.mtrr1_base_hi = mtrr1_base_hi;
    s.mtrr1_active  = mtrr1_active;
    s.mtrr1_ema     = composite as u16; // seed EMA with first real reading

    serial_println!(
        "[msr_ia32_mtrr_physbase1] init: type={} base_hi={} active={} ema={}",
        s.mtrr1_type,
        s.mtrr1_base_hi,
        s.mtrr1_active,
        s.mtrr1_ema
    );
}

pub fn tick(age: u32) {
    // MTRR registers are set by firmware and static at runtime — gate tightly.
    if age % TICK_GATE != 0 {
        return;
    }

    if !is_available() {
        return;
    }

    let (lo, _hi) = unsafe { rdmsr_physbase1() };

    let mtrr1_type    = compute_mtrr1_type(lo);
    let mtrr1_base_hi = compute_mtrr1_base_hi(lo);
    let mtrr1_active  = compute_mtrr1_active(lo);
    let composite     = compute_composite(mtrr1_type, mtrr1_base_hi, mtrr1_active);

    let mut s = STATE.lock();
    s.mtrr1_type    = mtrr1_type;
    s.mtrr1_base_hi = mtrr1_base_hi;
    s.mtrr1_active  = mtrr1_active;
    s.mtrr1_ema     = ema(s.mtrr1_ema, composite);

    serial_println!(
        "[msr_ia32_mtrr_physbase1] age={} type={} base_hi={} active={} ema={}",
        age,
        s.mtrr1_type,
        s.mtrr1_base_hi,
        s.mtrr1_active,
        s.mtrr1_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// Memory type code scaled 0–1000 (UC=0, WC=142, WT=568, WP=710, WB=852).
pub fn get_mtrr1_type() -> u16 {
    STATE.lock().mtrr1_type
}

/// Top 12 bits of the physical base address, scaled 0–1000.
pub fn get_mtrr1_base_hi() -> u16 {
    STATE.lock().mtrr1_base_hi
}

/// 1000 if the region is mapped to a non-zero physical address, else 0.
pub fn get_mtrr1_active() -> u16 {
    STATE.lock().mtrr1_active
}

/// EMA of (type/4 + base_hi/4 + active/2) — sustained MTRR texture pressure.
pub fn get_mtrr1_ema() -> u16 {
    STATE.lock().mtrr1_ema
}
