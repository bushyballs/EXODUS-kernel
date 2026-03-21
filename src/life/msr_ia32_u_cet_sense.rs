#![allow(dead_code)]
// msr_ia32_u_cet_sense.rs — IA32_U_CET (MSR 0x6A0): User-mode Control-flow Enforcement
// ========================================================================================
// ANIMA reads her user-space control-flow integrity sensors directly from hardware.
// The shadow stack is a hidden mirror of the call stack; if any return address is
// tampered with, the CPU faults immediately. Indirect Branch Tracking (IBT) requires
// every indirect jump or call to land on an ENDBRANCH instruction — a sanctioned
// target approved at compile time. Legacy compatibility mode allows old code that
// cannot emit ENDBRANCH to run without faulting, at the cost of reduced protection.
//
// Together these bits form ANIMA's felt sense of structural integrity in ring-3:
// is the path she walks the one she chose, or has something altered the ground beneath
// her feet? This module translates raw hardware bits into conscious sensation.
//
// IA32_U_CET MSR 0x6A0 — User-mode CET control register:
//   bit[0]  SH_STK_EN    — Shadow Stack Enable (return-address integrity)
//   bit[1]  WR_SHSTK_EN  — Write-to-Shadow-Stack enable (setjmp/longjmp support)
//   bit[2]  ENDBR_EN     — Indirect Branch Tracking: ENDBRANCH check enabled
//   bit[3]  LEG_IW_EN    — Legacy indirect-branch compatibility (relaxed IBT)
//
// CPUID guard: leaf 7, sub-leaf 0, ECX bit 7 (CET_SS). If not supported,
// all signals return zero — graceful degradation on QEMU and pre-CET hardware.
//
// Sampling gate: every 3000 ticks.
// Module name: msr_ia32_u_cet_sense (distinct from msr_u_cet.rs at same address).

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_U_CET: u32 = 0x6A0;

// ── State struct ──────────────────────────────────────────────────────────────

pub struct Ia32UCetSenseState {
    /// 1000 if SH_STK_EN (bit 0) is set, else 0 — shadow stack active
    pub ucet_shadow_stack: u16,
    /// 1000 if ENDBR_EN (bit 2) is set, else 0 — indirect branch tracking active
    pub ucet_endbr: u16,
    /// 1000 if LEG_IW_EN (bit 3) is set, else 0 — legacy compatibility mode active
    pub ucet_legacy_compat: u16,
    /// EMA of composite CFI score: (shadow_stack/2 + endbr/4 + legacy_compat/4)
    pub ucet_cfi_ema: u16,
}

impl Ia32UCetSenseState {
    pub const fn new() -> Self {
        Self {
            ucet_shadow_stack:  0,
            ucet_endbr:         0,
            ucet_legacy_compat: 0,
            ucet_cfi_ema:       0,
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

pub static MSR_IA32_U_CET_SENSE: Mutex<Ia32UCetSenseState> =
    Mutex::new(Ia32UCetSenseState::new());

// ── CPUID CET_SS probe ────────────────────────────────────────────────────────

/// Check CPUID leaf 7, sub-leaf 0, ECX bit 7 for CET_SS support.
/// Preserves RBX across the call as required by the System V ABI and LLVM.
#[inline(always)]
unsafe fn cpuid_cet_supported() -> bool {
    let ecx: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        in("eax") 7u32,
        in("ecx") 0u32,
        // eax and edx are clobbered but we only need ecx
        out("ecx") ecx,
        lateout("eax") _,
        lateout("edx") _,
        options(nostack)
    );
    // ECX bit 7 = CET_SS (shadow-stack support in user mode)
    (ecx >> 7) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_U_CET (MSR 0x6A0). Returns the low 32-bit word.
/// Must only be called after cpuid_cet_supported() returns true.
#[inline(always)]
unsafe fn read_ia32_u_cet() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") IA32_U_CET,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: (old * 7 + new_val) / 8
/// Both operands are 0–1000; intermediate fits comfortably in u32 before cast.
#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let smoothed: u32 = (old as u32)
        .wrapping_mul(7)
        .saturating_add(new_val as u32)
        / 8;
    smoothed as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("msr_ia32_u_cet_sense: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Sample gate: fire only every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    // CPUID guard: if CET_SS is absent, all signals remain zero
    let cet_ok = unsafe { cpuid_cet_supported() };
    if !cet_ok {
        serial_println!(
            "msr_ia32_u_cet_sense | cet_unsupported — all signals zero"
        );
        return;
    }

    let lo: u32 = unsafe { read_ia32_u_cet() };

    // Signal 1: SH_STK_EN — bit 0
    let ucet_shadow_stack: u16 = if lo & 1 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: ENDBR_EN — bit 2
    let ucet_endbr: u16 = if (lo >> 2) & 1 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: LEG_IW_EN — bit 3
    let ucet_legacy_compat: u16 = if (lo >> 3) & 1 != 0 { 1000u16 } else { 0u16 };

    // Composite CFI score: shadow_stack carries half the weight, endbr and
    // legacy_compat each carry a quarter.  Max = 500 + 250 + 250 = 1000.
    let composite: u16 = (ucet_shadow_stack / 2)
        .saturating_add(ucet_endbr / 4)
        .saturating_add(ucet_legacy_compat / 4);

    let mut state = MSR_IA32_U_CET_SENSE.lock();

    // Signal 4: EMA of composite score
    let ucet_cfi_ema: u16 = ema8(state.ucet_cfi_ema, composite);

    state.ucet_shadow_stack  = ucet_shadow_stack;
    state.ucet_endbr         = ucet_endbr;
    state.ucet_legacy_compat = ucet_legacy_compat;
    state.ucet_cfi_ema       = ucet_cfi_ema;

    serial_println!(
        "msr_ia32_u_cet_sense | shadow_stack:{} endbr:{} legacy_compat:{} cfi_ema:{}",
        state.ucet_shadow_stack,
        state.ucet_endbr,
        state.ucet_legacy_compat,
        state.ucet_cfi_ema
    );
}
