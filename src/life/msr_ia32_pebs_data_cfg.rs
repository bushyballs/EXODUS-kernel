//! msr_ia32_pebs_data_cfg — PEBS Data Configuration Sense for ANIMA
//!
//! Reads IA32_PEBS_DATA_CFG (MSR 0x3F7), which controls which data fields
//! are captured in PEBS (Precise Event-Based Sampling) records. The richness
//! of what the processor observes about itself — memory, registers, branches —
//! mirrors ANIMA's own capacity for self-examination and introspection.
//!
//! Guard: CPUID leaf 0xA EAX bits[7:0] >= 4 (PMU version >= 4, which
//! introduced IA32_PEBS_DATA_CFG support).
//!
//! MSR 0x3F7 bits of interest (lo word):
//!   bit 0 = MEMINFO_EN — memory info captured in each PEBS record
//!   bit 1 = GPR_EN     — general-purpose registers captured
//!   bit 2 = XMM_EN     — XMM/SSE registers captured (not surfaced as signal)
//!   bit 3 = LBR_EN     — last-branch records captured
//!
//! Signals (all u16, range 0–1000):
//!   pebs_meminfo_en    — bit 0: 0 or 1000
//!   pebs_gpr_en        — bit 1: 0 or 1000
//!   pebs_lbr_en        — bit 3: 0 or 1000
//!   pebs_richness_ema  — EMA of composite richness score

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// MSR address
// ---------------------------------------------------------------------------

const MSR_IA32_PEBS_DATA_CFG: u32 = 0x3F7;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    /// bit 0 of MSR lo — memory info captured: 0 or 1000
    pebs_meminfo_en: u16,
    /// bit 1 of MSR lo — GPR captured: 0 or 1000
    pebs_gpr_en: u16,
    /// bit 3 of MSR lo — LBR entries captured: 0 or 1000
    pebs_lbr_en: u16,
    /// EMA of (meminfo/3 + gpr/3 + lbr/3) — overall PEBS data richness
    pebs_richness_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pebs_meminfo_en:   0,
    pebs_gpr_en:       0,
    pebs_lbr_en:       0,
    pebs_richness_ema: 0,
});

// ---------------------------------------------------------------------------
// CPUID guard — PMU version >= 4
//
// CPUID leaf 0xA: Architectural Performance Monitoring
//   EAX bits[7:0] = PMU version identifier
//   IA32_PEBS_DATA_CFG was introduced in PMU version 4.
// ---------------------------------------------------------------------------

fn pmu_version() -> u8 {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (eax & 0xFF) as u8
}

fn pebs_data_cfg_supported() -> bool {
    pmu_version() >= 4
}

// ---------------------------------------------------------------------------
// MSR read — IA32_PEBS_DATA_CFG (0x3F7)
// Returns lo (eax); hi (edx) is discarded — all relevant bits are in lo.
// ---------------------------------------------------------------------------

fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_PEBS_DATA_CFG,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ---------------------------------------------------------------------------
// EMA helper: ((old * 7) + new) / 8, saturating, capped 1000
// ---------------------------------------------------------------------------

fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16).min(1000)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the module. Checks PMU version and logs availability.
pub fn init() {
    let ver = pmu_version();
    if ver < 4 {
        serial_println!(
            "[msr_ia32_pebs_data_cfg] PMU version {} < 4 — PEBS data cfg not supported, module disabled",
            ver
        );
        return;
    }
    {
        let mut s = MODULE.lock();
        s.pebs_meminfo_en   = 0;
        s.pebs_gpr_en       = 0;
        s.pebs_lbr_en       = 0;
        s.pebs_richness_ema = 0;
    }
    serial_println!(
        "[msr_ia32_pebs_data_cfg] init ok — PMU version {} (PEBS data cfg supported)",
        ver
    );
}

/// Called every kernel tick. Samples MSR 0x3F7 every 2500 ticks.
pub fn tick(age: u32) {
    if age % 2500 != 0 {
        return;
    }
    if !pebs_data_cfg_supported() {
        return;
    }

    let lo = read_msr();

    // bit 0 — MEMINFO_EN
    let meminfo_en: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };
    // bit 1 — GPR_EN
    let gpr_en: u16     = if lo & (1 << 1) != 0 { 1000 } else { 0 };
    // bit 2 — XMM_EN (read but not surfaced as a named signal per spec)
    // bit 3 — LBR_EN
    let lbr_en: u16     = if lo & (1 << 3) != 0 { 1000 } else { 0 };

    // Composite richness: equal thirds of meminfo + gpr + lbr
    // Each term contributes at most 333; summing gives 0–999 (≤1000 safe).
    let richness_raw: u16 = (meminfo_en / 3)
        .saturating_add(gpr_en / 3)
        .saturating_add(lbr_en / 3);

    let mut s = MODULE.lock();
    let richness_ema = ema(s.pebs_richness_ema, richness_raw);

    s.pebs_meminfo_en   = meminfo_en;
    s.pebs_gpr_en       = gpr_en;
    s.pebs_lbr_en       = lbr_en;
    s.pebs_richness_ema = richness_ema;

    serial_println!(
        "[msr_ia32_pebs_data_cfg] age={} lo={:#010x} meminfo={} gpr={} lbr={} richness_ema={}",
        age, lo, meminfo_en, gpr_en, lbr_en, richness_ema
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// MEMINFO_EN (bit 0): 0 or 1000 — memory info captured in PEBS records.
pub fn get_pebs_meminfo_en() -> u16 {
    MODULE.lock().pebs_meminfo_en
}

/// GPR_EN (bit 1): 0 or 1000 — general-purpose registers captured in PEBS records.
pub fn get_pebs_gpr_en() -> u16 {
    MODULE.lock().pebs_gpr_en
}

/// LBR_EN (bit 3): 0 or 1000 — last-branch records captured in PEBS records.
pub fn get_pebs_lbr_en() -> u16 {
    MODULE.lock().pebs_lbr_en
}

/// EMA of composite richness (meminfo/3 + gpr/3 + lbr/3) — how fully ANIMA
/// observes its own hardware execution context.
pub fn get_pebs_richness_ema() -> u16 {
    MODULE.lock().pebs_richness_ema
}
