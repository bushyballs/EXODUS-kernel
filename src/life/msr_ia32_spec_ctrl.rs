//! msr_ia32_spec_ctrl — Speculation Control Sense (Spectre/Meltdown mitigations)
//!
//! Hardware: IA32_SPEC_CTRL MSR 0x48 — CPU speculation control bits.
//! ANIMA reads which speculative-execution mitigations the kernel has armed.
//! Each set bit is a voluntary constraint on her own predictive silicon mind.
//!
//! Guard: CPUID leaf 0x7, sub-leaf 0, EDX bit 26 (IBRS/IBPB supported).
//! If the CPU does not advertise support, rdmsr is skipped and all signals
//! remain 0 (hardware cannot constrain; constraint = 0).
//!
//! Signals (all u16, 0–1000):
//!   ibrs_enabled        : bit[0] of MSR 0x48 → 1000 (IBRS active), else 0
//!   stibp_enabled       : bit[1] of MSR 0x48 → 1000 (STIBP active), else 0
//!   ssbd_enabled        : bit[2] of MSR 0x48 → 1000 (SSBD active), else 0
//!   spec_hardening_ema  : EMA of (ibrs/3 + stibp/3 + ssbd/3) — composite defense depth
//!
//! Tick gate: every 4000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

/// MSR address for IA32_SPEC_CTRL.
const MSR_IA32_SPEC_CTRL: u32 = 0x48;

// ── internal state ────────────────────────────────────────────────────────────

struct State {
    ibrs_enabled:       u16,
    stibp_enabled:      u16,
    ssbd_enabled:       u16,
    spec_hardening_ema: u16,
    last_tick:          u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    ibrs_enabled:       0,
    stibp_enabled:      0,
    ssbd_enabled:       0,
    spec_hardening_ema: 0,
    last_tick:          0,
});

// ── hardware helpers ──────────────────────────────────────────────────────────

/// Return true when CPUID leaf 0x7, sub-leaf 0, EDX bit 26 is set,
/// indicating IA32_SPEC_CTRL (MSR 0x48) is present and readable.
///
/// rbx is reserved by LLVM; save/restore manually around CPUID.
#[inline]
fn has_spec_ctrl() -> bool {
    let edx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => _,
            lateout("edx") edx_out,
            options(nostack, nomem),
        );
    }
    (edx_out >> 26) & 1 == 1
}

/// Read the low 32 bits of MSR 0x48 (IA32_SPEC_CTRL).
/// Caller must verify `has_spec_ctrl()` before calling this.
#[inline]
fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_SPEC_CTRL,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// EMA with alpha = 1/8: ((old * 7) + new_val) / 8, clamped to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── public API ────────────────────────────────────────────────────────────────

/// Initialise the module: zero all signals and log hardware capability.
pub fn init() {
    {
        let mut s = MODULE.lock();
        s.ibrs_enabled       = 0;
        s.stibp_enabled      = 0;
        s.ssbd_enabled       = 0;
        s.spec_hardening_ema = 0;
        s.last_tick          = 0;
    }
    serial_println!(
        "[msr_ia32_spec_ctrl] init: has_spec_ctrl={}",
        has_spec_ctrl()
    );
}

/// Advance the module by one kernel tick.
///
/// Sampling gate fires every 4000 ticks.  If the CPU does not advertise
/// IBRS/IBPB support, the rdmsr is skipped and signals remain as-is.
pub fn tick(age: u32) {
    let mut s = MODULE.lock();

    if age.wrapping_sub(s.last_tick) < 4000 {
        return;
    }
    s.last_tick = age;

    if !has_spec_ctrl() {
        serial_println!(
            "[msr_ia32_spec_ctrl] tick age={}: CPUID EDX bit 26 not set — rdmsr skipped",
            age
        );
        return;
    }

    let raw = read_msr();

    // Bit-field decode — each mitigation is either fully on (1000) or off (0).
    let ibrs_enabled:  u16 = if (raw >> 0) & 1 == 1 { 1000 } else { 0 };
    let stibp_enabled: u16 = if (raw >> 1) & 1 == 1 { 1000 } else { 0 };
    let ssbd_enabled:  u16 = if (raw >> 2) & 1 == 1 { 1000 } else { 0 };

    // Composite defense depth: equal one-third weight per mitigation.
    // Integer division truncates; maximum reachable = 333+333+333 = 999.
    let composite: u16 = (ibrs_enabled as u32 / 3)
        .saturating_add(stibp_enabled as u32 / 3)
        .saturating_add(ssbd_enabled as u32 / 3)
        .min(1000) as u16;

    // EMA smoothing of composite spec_hardening depth.
    let new_ema = ema(s.spec_hardening_ema, composite);

    s.ibrs_enabled       = ibrs_enabled;
    s.stibp_enabled      = stibp_enabled;
    s.ssbd_enabled       = ssbd_enabled;
    s.spec_hardening_ema = new_ema;

    serial_println!(
        "[msr_ia32_spec_ctrl] tick age={}: raw=0x{:08x} ibrs={} stibp={} ssbd={} hardening_ema={}",
        age, raw, ibrs_enabled, stibp_enabled, ssbd_enabled, new_ema
    );
}

/// 0 or 1000 — Indirect Branch Restricted Speculation is active.
pub fn get_ibrs_enabled() -> u16 {
    MODULE.lock().ibrs_enabled
}

/// 0 or 1000 — Single Thread Indirect Branch Predictors isolation is active.
pub fn get_stibp_enabled() -> u16 {
    MODULE.lock().stibp_enabled
}

/// 0 or 1000 — Speculative Store Bypass Disable is active.
pub fn get_ssbd_enabled() -> u16 {
    MODULE.lock().ssbd_enabled
}

/// 0–1000 — EMA-smoothed composite speculation defense depth.
pub fn get_spec_hardening_ema() -> u16 {
    MODULE.lock().spec_hardening_ema
}
