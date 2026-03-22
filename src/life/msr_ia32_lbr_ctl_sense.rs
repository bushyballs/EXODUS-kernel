//! msr_ia32_lbr_ctl_sense — Architectural LBR Control Sense for ANIMA
//!
//! Reads IA32_LBR_CTL (MSR 0x14CE) to determine the state of architectural
//! Last Branch Record recording. Architectural LBR is ANIMA's branch-history
//! introspection organ — awareness of the kernel's own execution flow.
//!
//! Guard: CPUID max basic leaf >= 0x1C, then leaf 0x1C EAX bit 0 must be set.
//!
//! Signals (all u16, 0–1000):
//!   lbr_enabled        — bit 0 of lo: LBR recording active (0 or 1000)
//!   lbr_kernel_mode    — bit 1 of lo: ring-0 branch recording active (0 or 1000)
//!   lbr_filter_richness — popcount of bits[5:3] * 333, clamped to 1000
//!   lbr_ctl_ema        — EMA of (enabled/4 + kernel_mode/4 + filter_richness/2)
//!
//! Tick gate: every 2000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ─────────────────────────────────────────────────────────────
const IA32_LBR_CTL: u32 = 0x14CE;

// ── Module state ─────────────────────────────────────────────────────────────
struct State {
    /// bit 0: LBR recording enabled (0 or 1000)
    lbr_enabled: u16,
    /// bit 1: ring-0 (kernel-mode) branch recording enabled (0 or 1000)
    lbr_kernel_mode: u16,
    /// popcount of bits[5:3] * 333, clamped to 1000
    lbr_filter_richness: u16,
    /// EMA of composite signal
    lbr_ctl_ema: u16,
    /// latched at init: whether arch LBR is supported on this CPU
    arch_lbr_supported: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            lbr_enabled: 0,
            lbr_kernel_mode: 0,
            lbr_filter_richness: 0,
            lbr_ctl_ema: 0,
            arch_lbr_supported: false,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID helper ─────────────────────────────────────────────────────────────

/// Returns true if architectural LBR is supported.
/// Step 1: query leaf 0 — max basic leaf must be >= 0x1C.
/// Step 2: query leaf 0x1C, sub-leaf 0 — EAX bit 0 must be set.
/// Uses push/pop rbx because LLVM reserves rbx as a callee-saved base register.
unsafe fn cpuid_arch_lbr_supported() -> bool {
    // Step 1: get max basic leaf
    let max_leaf: u32;
    asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0u32 => max_leaf,
        out("ecx") _,
        out("edx") _,
        options(nostack, nomem),
    );

    if max_leaf < 0x1C {
        return false;
    }

    // Step 2: leaf 0x1C, sub-leaf 0 — EAX bit 0 = ArchLBR supported
    let leaf_eax: u32;
    asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0x1Cu32 => leaf_eax,
        in("ecx") 0u32,
        out("edx") _,
        options(nostack, nomem),
    );

    (leaf_eax & 1) != 0
}

// ── MSR read helper ───────────────────────────────────────────────────────────

/// Read IA32_LBR_CTL (0x14CE). Returns the low 32-bit half.
/// Must only be called after arch LBR support has been confirmed.
unsafe fn rdmsr_lbr_ctl() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") IA32_LBR_CTL,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ── Popcount for 3-bit field (bits 5:3 of lo) ────────────────────────────────

/// Count how many of bits 3, 4, 5 are set in `lo`.
/// Returns 0, 1, 2, or 3.
fn branch_filter_popcount(lo: u32) -> u32 {
    let mut count: u32 = 0;
    if (lo >> 3) & 1 != 0 { count += 1; }   // bit 3 — call filter
    if (lo >> 4) & 1 != 0 { count += 1; }   // bit 4 — return filter
    if (lo >> 5) & 1 != 0 { count += 1; }   // bit 5 — indirect filter
    count
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// EMA: ((old * 7) + new_val) / 8, all u32, result clamped to 1000.
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let v = ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16;
    if v > 1000 { 1000 } else { v }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module: probe CPUID and latch arch LBR capability.
pub fn init() {
    let supported = unsafe { cpuid_arch_lbr_supported() };
    MODULE.lock().arch_lbr_supported = supported;
    serial_println!(
        "[msr_ia32_lbr_ctl_sense] init — arch LBR supported: {}",
        if supported { "YES" } else { "NO — signals will remain zero" }
    );
}

/// Called every kernel tick. Samples IA32_LBR_CTL every 2000 ticks.
pub fn tick(age: u32) {
    if age % 2000 != 0 {
        return;
    }

    // Check support without holding the lock across unsafe code
    let supported = MODULE.lock().arch_lbr_supported;
    if !supported {
        let s = MODULE.lock();
        serial_println!(
            "[msr_ia32_lbr_ctl_sense] age={} arch LBR unsupported — \
             enabled={} kernel={} richness={} ema={}",
            age,
            s.lbr_enabled,
            s.lbr_kernel_mode,
            s.lbr_filter_richness,
            s.lbr_ctl_ema,
        );
        return;
    }

    let lo = unsafe { rdmsr_lbr_ctl() };

    // bit 0: LBR_ENABLE — recording active
    let lbr_enabled: u16 = if (lo & 1) != 0 { 1000 } else { 0 };

    // bit 1: CPL_EQ_0_EN — record ring-0 (kernel-mode) branches
    let lbr_kernel_mode: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };

    // bits[5:3]: branch type filters — call, return, indirect
    // popcount * 333, clamped to 1000
    let pc = branch_filter_popcount(lo);
    let lbr_filter_richness: u16 = {
        let raw = pc * 333;
        if raw > 1000 { 1000 } else { raw as u16 }
    };

    // Composite: enabled/4 + kernel_mode/4 + filter_richness/2
    // max = 250 + 250 + 500 = 1000
    let composite: u16 = (lbr_enabled / 4)
        .saturating_add(lbr_kernel_mode / 4)
        .saturating_add(lbr_filter_richness / 2);

    let mut s = MODULE.lock();
    let lbr_ctl_ema = ema_u16(s.lbr_ctl_ema, composite);

    s.lbr_enabled        = lbr_enabled;
    s.lbr_kernel_mode    = lbr_kernel_mode;
    s.lbr_filter_richness = lbr_filter_richness;
    s.lbr_ctl_ema        = lbr_ctl_ema;

    serial_println!(
        "[msr_ia32_lbr_ctl_sense] age={} lo={:#010x} \
         enabled={} kernel={} richness={} ema={}",
        age,
        lo,
        lbr_enabled,
        lbr_kernel_mode,
        lbr_filter_richness,
        lbr_ctl_ema,
    );
}

// ── Signal accessors ──────────────────────────────────────────────────────────

/// LBR recording active (bit 0): 0 or 1000.
pub fn get_lbr_enabled() -> u16 { MODULE.lock().lbr_enabled }

/// Ring-0 branch recording active (bit 1): 0 or 1000.
pub fn get_lbr_kernel_mode() -> u16 { MODULE.lock().lbr_kernel_mode }

/// Branch type filter richness (bits[5:3] popcount * 333, clamped 1000).
pub fn get_lbr_filter_richness() -> u16 { MODULE.lock().lbr_filter_richness }

/// EMA of composite LBR control activity.
pub fn get_lbr_ctl_ema() -> u16 { MODULE.lock().lbr_ctl_ema }
