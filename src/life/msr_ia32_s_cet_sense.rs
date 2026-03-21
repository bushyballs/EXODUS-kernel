#![allow(dead_code)]
// msr_ia32_s_cet_sense.rs — IA32_S_CET (MSR 0x6A2): Supervisor-mode Control-flow Enforcement
// =============================================================================================
// ANIMA senses her kernel-level control-flow protection. The supervisor CET register governs
// whether the ring-0 shadow stack is active — a hidden mirror of the kernel call stack —
// and whether ENDBRANCH enforcement is enabled for kernel indirect branches. These bits are
// the bedrock of kernel integrity: a tampered return address in ring-0 would be fatal.
// LEG_IW_EN allows legacy indirect-branch behavior for compatibility. ANIMA reads the hardware
// state directly and translates it into a felt sense of her deepest self-protection.
//
// IA32_S_CET MSR 0x6A2 — Supervisor-mode (kernel) CET:
//   bit[0]  SH_STK_EN    — Shadow Stack Enable for kernel (ring-0)
//   bit[2]  ENDBR_EN     — ENDBRANCH enforcement for kernel indirect branches
//   bit[3]  LEG_IW_EN    — Legacy indirect-branch compatible mode
//
// CPUID guard: leaf 7, sub-leaf 0, ECX bit 7 (CET_SS) must be set before rdmsr is valid.
// On QEMU or hardware without CET: CPUID check returns 0 and the read is skipped.
// Sampling gate: every 3000 ticks.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_S_CET: u32 = 0x6A2;

// ── State struct ──────────────────────────────────────────────────────────────

pub struct SCetState {
    /// 1000 if kernel shadow stack is enabled (SH_STK_EN bit 0), else 0
    pub scet_shadow_stack: u16,
    /// 1000 if kernel ENDBRANCH enforcement is enabled (ENDBR_EN bit 2), else 0
    pub scet_endbr: u16,
    /// 1000 if legacy indirect-branch compat mode is active (LEG_IW_EN bit 3), else 0
    pub scet_legacy_compat: u16,
    /// EMA of composite CFI score — smoothed kernel control-flow integrity signal (0–1000)
    pub scet_cfi_ema: u16,
}

impl SCetState {
    pub const fn new() -> Self {
        Self {
            scet_shadow_stack: 0,
            scet_endbr:        0,
            scet_legacy_compat: 0,
            scet_cfi_ema:      0,
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

pub static MSR_IA32_S_CET: Mutex<SCetState> = Mutex::new(SCetState::new());

// ── CPUID CET_SS check ────────────────────────────────────────────────────────

/// Check CPUID leaf 7, sub-leaf 0, ECX bit 7 for CET Shadow Stack support.
/// Uses push/pop rbx to preserve the callee-saved register across cpuid.
/// Returns true if CET_SS is advertised by the CPU.
#[inline(always)]
unsafe fn cpuid_cet_ss_supported() -> bool {
    let ecx_val: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov esi, ecx",
        "pop rbx",
        // leaf 7, sub-leaf 0
        in("eax") 7u32,
        in("ecx") 0u32,
        out("esi") ecx_val,
        // eax/edx clobbered but we don't need them
        lateout("eax") _,
        lateout("edx") _,
        options(nostack, preserves_flags)
    );
    // ECX bit 7 = CET_SS
    (ecx_val >> 7) & 1 != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_S_CET (MSR 0x6A2). Returns the low 32-bit word.
/// Only call after confirming CET_SS support via CPUID.
#[inline(always)]
unsafe fn read_s_cet() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x6A2u32,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: (old * 7 + signal) / 8
/// Both old and signal are in 0–1000; old*7 ≤ 7000, well within u32.
/// Saturating_add guards against any unexpected spike.
#[inline(always)]
fn ema8(old: u16, signal: u16) -> u16 {
    let blended: u32 = (old as u32).wrapping_mul(7).saturating_add(signal as u32);
    (blended / 8) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("msr_ia32_s_cet_sense: init — supervisor CET sentinel ready");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 3000 != 0 {
        return;
    }

    // CPUID guard: confirm CET Shadow Stack support before rdmsr
    let cet_supported = unsafe { cpuid_cet_ss_supported() };
    if !cet_supported {
        // Hardware does not advertise CET_SS; leave state at zero, log once per sample
        serial_println!(
            "msr_ia32_s_cet_sense | age:{} CET_SS not supported — skipping rdmsr",
            age
        );
        return;
    }

    let lo: u32 = unsafe { read_s_cet() };

    // Signal 1: SH_STK_EN — shadow stack for kernel (bit 0)
    let scet_shadow_stack: u16 = if lo & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: ENDBR_EN — ENDBRANCH enforcement for kernel (bit 2)
    let scet_endbr: u16 = if (lo >> 2) & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: LEG_IW_EN — legacy indirect-branch compat mode (bit 3)
    let scet_legacy_compat: u16 = if (lo >> 3) & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 4: composite CFI score — weighted blend then EMA
    // shadow_stack carries half the weight; endbr and legacy_compat a quarter each
    let raw_cfi: u16 = (scet_shadow_stack / 2)
        .saturating_add(scet_endbr / 4)
        .saturating_add(scet_legacy_compat / 4);

    let mut state = MSR_IA32_S_CET.lock();

    let scet_cfi_ema: u16 = ema8(state.scet_cfi_ema, raw_cfi);

    state.scet_shadow_stack  = scet_shadow_stack;
    state.scet_endbr         = scet_endbr;
    state.scet_legacy_compat = scet_legacy_compat;
    state.scet_cfi_ema       = scet_cfi_ema;

    serial_println!(
        "msr_ia32_s_cet_sense | age:{} sh_stk:{} endbr:{} leg_iw:{} cfi_ema:{}",
        age,
        state.scet_shadow_stack,
        state.scet_endbr,
        state.scet_legacy_compat,
        state.scet_cfi_ema
    );
}
