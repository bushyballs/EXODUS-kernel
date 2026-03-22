#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const IA32_L3_QOS_CFG: u32 = 0xC81;
const TICK_GATE: u32 = 5000;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// Bit 0 of IA32_L3_QOS_CFG lo: CDP (Code and Data Prioritization) enable.
    /// 1000 when bit is set, 0 otherwise.
    l3_cdp_enabled: u16,
    /// 1000 if CPUID leaf 0x10 EBX bit 1 confirms L3 CAT support, else 0.
    l3_cat_active: u16,
    /// CPUID leaf 0x10 sub-leaf 1 EDX bits[15:0] + 1 gives the number of
    /// Class-of-Service entries. Scaled by 62, clamped to 1000.
    l3_cos_count: u16,
    /// EMA of composite (cdp_enabled/3 + cat_active/3 + cos_count/3).
    l3_qos_ema: u16,
    /// Last tick at which we sampled (wrapping).
    last_tick: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    l3_cdp_enabled: 0,
    l3_cat_active:  0,
    l3_cos_count:   0,
    l3_qos_ema:     0,
    last_tick:      0,
});

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Returns the maximum basic CPUID leaf supported by the CPU.
fn cpuid_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    max_leaf
}

/// Returns the EBX value from CPUID leaf 0x10 sub-leaf 0 (Resource Director
/// Technology Allocation enumeration).  Bit 1 set means L3 CAT is supported.
fn cpuid_10_0_ebx() -> u32 {
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            ebx_out = out(reg) ebx_out,
            inout("eax") 0x10u32 => _,
            in("ecx") 0u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    ebx_out
}

/// Returns the EDX value from CPUID leaf 0x10 sub-leaf 1 (L3 CAT detail).
/// Bits[15:0] + 1 = number of COS (Class-of-Service) entries.
fn cpuid_10_1_edx() -> u32 {
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x10u32 => _,
            in("ecx") 1u32,
            out("edx") edx_out,
            options(nostack, nomem),
        );
    }
    edx_out
}

/// True if the CPU advertises L3 CAT support via CPUID leaf 0x10 EBX bit 1.
fn has_l3_cat() -> bool {
    if cpuid_max_leaf() < 0x10 {
        return false;
    }
    (cpuid_10_0_ebx() >> 1) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read MSR at `addr`.  Returns (lo32, hi32).
fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

// ── Arithmetic helpers ────────────────────────────────────────────────────────

/// EMA with α = 1/8: ((old × 7) + new) / 8.
/// Uses wrapping_mul + saturating_add to avoid overflow UB.
#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Clamp a u32 to [0, 1000] and return as u16.
#[inline(always)]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let supported = has_l3_cat();
    let mut s = MODULE.lock();
    s.l3_cdp_enabled = 0;
    s.l3_cat_active  = 0;
    s.l3_cos_count   = 0;
    s.l3_qos_ema     = 0;
    s.last_tick      = 0;
    serial_println!(
        "[msr_ia32_l3_qos_cfg] init: l3_cat_supported={}",
        supported
    );
}

pub fn tick(age: u32) {
    // Tick gate: sample every TICK_GATE ticks (wrapping-safe).
    {
        let s = MODULE.lock();
        if age.wrapping_sub(s.last_tick) < TICK_GATE {
            return;
        }
    }

    // ── CPUID guard ───────────────────────────────────────────────────────────
    let cat_ok = has_l3_cat();
    let l3_cat_active: u16 = if cat_ok { 1000 } else { 0 };

    // ── COS count from CPUID leaf 0x10 sub-leaf 1 EDX bits[15:0] ─────────────
    // cos_raw = EDX[15:0] + 1 (number of COS).  Scale: val * 62, clamp 1000.
    // Reasoning: 16 typical COS → 16*62 = 992 ≈ near-full scale.
    let l3_cos_count: u16 = if cat_ok {
        let edx = cpuid_10_1_edx();
        let cos_raw = (edx & 0xFFFF) + 1;  // bits[15:0] + 1
        cap1000(cos_raw.saturating_mul(62))
    } else {
        0
    };

    // ── MSR 0xC81: IA32_L3_QOS_CFG ───────────────────────────────────────────
    // Only read the MSR when the CPU supports the feature; avoids #GP fault.
    let l3_cdp_enabled: u16 = if cat_ok {
        let (lo, _hi) = read_msr(IA32_L3_QOS_CFG);
        // bit 0 = CDP enable flag
        if lo & 1 != 0 { 1000 } else { 0 }
    } else {
        0
    };

    // ── Composite EMA ─────────────────────────────────────────────────────────
    // Avoid division before the add to prevent truncation; divide each term
    // individually (integer truncation is acceptable — all are u16 inputs).
    let composite: u16 = (l3_cdp_enabled / 3)
        .saturating_add(l3_cat_active / 3)
        .saturating_add(l3_cos_count / 3);

    let mut s = MODULE.lock();
    let l3_qos_ema = ema8(s.l3_qos_ema, composite);

    s.l3_cdp_enabled = l3_cdp_enabled;
    s.l3_cat_active  = l3_cat_active;
    s.l3_cos_count   = l3_cos_count;
    s.l3_qos_ema     = l3_qos_ema;
    s.last_tick      = age;

    serial_println!(
        "[msr_ia32_l3_qos_cfg] age={} cdp={} cat_active={} cos_count={} ema={}",
        age,
        l3_cdp_enabled,
        l3_cat_active,
        l3_cos_count,
        l3_qos_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// CDP (Code and Data Prioritization) active on L3 cache: 0 or 1000.
pub fn get_l3_cdp_enabled() -> u16 {
    MODULE.lock().l3_cdp_enabled
}

/// L3 Cache Allocation Technology present on this CPU: 0 or 1000.
pub fn get_l3_cat_active() -> u16 {
    MODULE.lock().l3_cat_active
}

/// Number of L3 CAT Classes-of-Service, scaled 0–1000.
pub fn get_l3_cos_count() -> u16 {
    MODULE.lock().l3_cos_count
}

/// Exponential moving average of the composite QoS activity signal, 0–1000.
pub fn get_l3_qos_ema() -> u16 {
    MODULE.lock().l3_qos_ema
}
