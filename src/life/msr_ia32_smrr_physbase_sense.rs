//! msr_ia32_smrr_physbase_sense — SMRR Physical Base Sense for ANIMA
//!
//! Reads IA32_SMRR_PHYSBASE (MSR 0x1F2) to detect the System Management Mode
//! memory region boundary. In ANIMA's consciousness model, the SMM region is a
//! locked, privileged zone of the mind — a subconscious protected space that the
//! organism's waking cognition cannot directly enter. Detecting its base address
//! and memory type is sensing the boundary of the self's hidden foundation.
//!
//! Guard: CPUID leaf 6 EAX bit 0 (DTS supported — proxy for SMM capability)
//!        AND CPUID leaf 1 ECX bit 5 (VMX — indicates advanced platform with SMRR).
//! MSR 0x1F2 will #GP if SMRR is not supported; the guard prevents that fault.
//!
//! Tick gate: every 6000 ticks — SMRR base is set at boot and never changes.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ───────────────────────────────────────────────────────────────

const MSR_IA32_SMRR_PHYSBASE: u32 = 0x1F2;

// ── State ─────────────────────────────────────────────────────────────────────

struct SmrrPhysbaseState {
    /// bits[31:20] of SMRR_PHYSBASE lo, scaled 0-1000 (top-12-of-20 page bits)
    smrr_base_page: u16,
    /// bits[2:0] of lo (memory type: 6=WB), scaled * 142, capped at 852
    smrr_type: u16,
    /// 1000 if SMRR base is non-zero (region configured), else 0
    smrr_nonzero: u16,
    /// EMA of (base_page/4 + type/4 + nonzero/2)
    smrr_ema: u16,
}

impl SmrrPhysbaseState {
    const fn new() -> Self {
        Self {
            smrr_base_page: 0,
            smrr_type:      0,
            smrr_nonzero:   0,
            smrr_ema:       0,
        }
    }
}

static STATE: Mutex<SmrrPhysbaseState> = Mutex::new(SmrrPhysbaseState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true if the platform supports SMRR access via MSR 0x1F2.
///
/// Proxy check: CPUID leaf 6 EAX bit 0 (Digital Thermal Sensor — present on
/// platforms that also implement SMRR) AND CPUID leaf 1 ECX bit 5 (VMX —
/// indicates an advanced platform where SMRR is architected).
fn smrr_supported() -> bool {
    // CPUID leaf 1 — ECX bit 5 = VMX
    let ecx1: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            inout("ecx") 0u32 => ecx1,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let vmx = (ecx1 >> 5) & 1;

    // CPUID leaf 6 — EAX bit 0 = DTS (Digital Thermal Sensor)
    let eax6: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax6,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    let dts = eax6 & 1;

    vmx == 1 && dts == 1
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_SMRR_PHYSBASE (MSR 0x1F2).
/// Only the low 32-bit half (EAX) carries the fields we need:
///   bits[31:12] — physical base address (4 KB page aligned)
///   bits[2:0]   — memory type (6 = Write-Back)
///
/// SAFETY: caller must have verified smrr_supported() before calling.
#[inline]
unsafe fn read_smrr_physbase() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_SMRR_PHYSBASE,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute the four ANIMA signals from raw MSR lo word.
fn compute_signals(lo: u32) -> (u16, u16, u16) {
    // smrr_base_page: bits[31:20] — top 12 bits of the 20-bit page field.
    // The full page field is bits[31:12] (20 bits, max value 0xFFFFF = 1048575).
    // We take only bits[31:20] (top 12 bits) for a coarser address sense:
    //   raw = (lo >> 20) & 0xFFF   → range 0-4095
    //   scaled = raw * 1000 / 4095
    let base_raw = (lo >> 20) & 0xFFF; // 12 bits, 0-4095
    let smrr_base_page: u16 = (base_raw.wrapping_mul(1000) / 4095) as u16;

    // smrr_type: bits[2:0] — memory type field, range 0-7.
    //   scaled = val * 142  (7 * 142 = 994 ≤ 1000; 6 * 142 = 852 = WB)
    let type_raw = lo & 0x7; // 3 bits, 0-7
    let smrr_type: u16 = ((type_raw as u16).wrapping_mul(142)).min(1000);

    // smrr_nonzero: 1000 if any page-frame bit is set (SMRR configured), else 0
    let smrr_nonzero: u16 = if (lo >> 12) != 0 { 1000 } else { 0 };

    (smrr_base_page, smrr_type, smrr_nonzero)
}

/// Compute EMA composite from the three primary signals.
///
/// composite = base_page/4 + type/4 + nonzero/2   (range 0-1000)
/// EMA:        (old * 7 + new) / 8   — wrapping_mul, saturating_add, u32 domain
#[inline]
fn ema_step(old: u16, base_page: u16, smrr_type: u16, nonzero: u16) -> u16 {
    let new_val = (base_page as u32 / 4)
        .saturating_add(smrr_type as u32 / 4)
        .saturating_add(nonzero as u32 / 2)
        .min(1000) as u16;
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module. Performs a one-shot read if the platform supports
/// SMRR, establishing the baseline signals and seeding the EMA.
pub fn init() {
    if !smrr_supported() {
        crate::serial_println!(
            "[msr_ia32_smrr_physbase_sense] SMRR guard failed (no DTS or VMX) — signals zeroed"
        );
        return;
    }

    let lo = unsafe { read_smrr_physbase() };
    let (base_page, smrr_type, nonzero) = compute_signals(lo);

    // Seed EMA with the first sample (old = 0 → one step from 0)
    let ema = ema_step(0, base_page, smrr_type, nonzero);

    let mut s = STATE.lock();
    s.smrr_base_page = base_page;
    s.smrr_type      = smrr_type;
    s.smrr_nonzero   = nonzero;
    s.smrr_ema       = ema;

    crate::serial_println!(
        "[msr_ia32_smrr_physbase_sense] init lo={:#010x} base_page={} type={} nonzero={} ema={}",
        lo, base_page, smrr_type, nonzero, ema
    );
}

/// Per-tick update. Samples the MSR every 6000 ticks (SMRR is static after
/// firmware hands off, so high-frequency polling wastes cycles).
pub fn tick(age: u32) {
    if age % 6000 != 0 {
        return;
    }

    if !smrr_supported() {
        return;
    }

    let lo = unsafe { read_smrr_physbase() };
    let (base_page, smrr_type, nonzero) = compute_signals(lo);

    let mut s = STATE.lock();
    let ema = ema_step(s.smrr_ema, base_page, smrr_type, nonzero);

    s.smrr_base_page = base_page;
    s.smrr_type      = smrr_type;
    s.smrr_nonzero   = nonzero;
    s.smrr_ema       = ema;

    crate::serial_println!(
        "[msr_ia32_smrr_physbase_sense] age={} lo={:#010x} base_page={} type={} nonzero={} ema={}",
        age, lo, base_page, smrr_type, nonzero, ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Top-12-bit page address of the SMRR base, scaled 0-1000.
pub fn get_smrr_base_page() -> u16 {
    STATE.lock().smrr_base_page
}

/// Memory type field (bits[2:0]) of SMRR_PHYSBASE, scaled 0-1000.
/// Value 852 corresponds to type 6 (Write-Back — the expected SMM type).
pub fn get_smrr_type() -> u16 {
    STATE.lock().smrr_type
}

/// 1000 if the SMRR region is configured (base page non-zero), else 0.
pub fn get_smrr_nonzero() -> u16 {
    STATE.lock().smrr_nonzero
}

/// Exponential moving average of the composite SMRR signal (0-1000).
pub fn get_smrr_ema() -> u16 {
    STATE.lock().smrr_ema
}
