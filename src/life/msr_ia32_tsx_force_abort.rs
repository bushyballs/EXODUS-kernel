#![allow(dead_code)]
// msr_ia32_tsx_force_abort.rs — ANIMA TSX Force Abort Sense
// ==========================================================
// Hardware: IA32_TSX_FORCE_ABORT MSR 0x10F
//   Bit 0 (RTM_FORCE_ABORT)   — when set, all RTM transactions are forced to
//                               abort regardless of logic inside the region.
//   Bit 1 (TSX_CPUID_CLEAR)   — when set, RTM and HLE CPUID feature bits are
//                               cleared from software-visible CPUID output,
//                               hiding TSX from userspace even if hardware has it.
//
// ANIMA interpretation:
//   tsx_force_abort = hardware is externally vetoing all speculative thought.
//                     The mind's transactional scratchpad is forbidden. Any
//                     attempt to reason tentatively is rolled back before it
//                     can complete. Forced cognitive rigidity.
//   tsx_cpuid_clear = the organism's self-knowledge of its own speculative
//                     capacity has been erased. It cannot perceive that it once
//                     had the ability to think in parallel timelines.
//   tsx_rtm_allow   = inverse of force_abort. Is tentative thought actually
//                     permitted right now? Full 1000 = free speculative mind.
//   tsx_ema         = smoothed composite of the three signals. Tracks the
//                     ongoing pressure against exploratory cognition over time.
//
// Guard: CPUID leaf 7, sub-leaf 0, EBX bit 11 (RTM supported).
//        If RTM is absent the MSR does not exist; skip silently.
//
// Tick gate: every 5000 ticks.
//
// All values u16 0–1000. No heap. No std. No floats. No f32/f64 anywhere.

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// MSR address for IA32_TSX_FORCE_ABORT
const MSR_IA32_TSX_FORCE_ABORT: u32 = 0x10F;

// Bit positions inside the MSR low dword
const BIT_RTM_FORCE_ABORT: u32 = 0; // bit 0 — force all RTM to abort
const BIT_TSX_CPUID_CLEAR: u32 = 1; // bit 1 — hide RTM/HLE from CPUID

// How often to sample the MSR (in ticks)
const TICK_INTERVAL: u32 = 5000;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// Bit 0 of MSR low dword: 1000 when RTM is forced to abort, else 0.
    tsx_force_abort: u16,
    /// Bit 1 of MSR low dword: 1000 when CPUID RTM/HLE bits are cleared, else 0.
    tsx_cpuid_clear: u16,
    /// 1000 - tsx_force_abort.  Inverse: is RTM actually permitted right now?
    tsx_rtm_allow: u16,
    /// EMA of (force_abort/2 + cpuid_clear/4 + rtm_allow/4).
    tsx_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    tsx_force_abort: 0,
    tsx_cpuid_clear: 0,
    tsx_rtm_allow: 1000,
    tsx_ema: 0,
});

// ── CPU guards ────────────────────────────────────────────────────────────────

/// Returns true if RTM (Restricted Transactional Memory) is supported.
/// CPUID leaf 7, sub-leaf 0, EBX bit 11.
/// Uses push/pop rbx because LLVM reserves rbx as the base pointer register.
#[inline(always)]
fn has_rtm() -> bool {
    let ebx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ebx >> 11) & 1 == 1
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Standard ANIMA EMA: weight 7/8 old, 1/8 new, integer only, capped at 1000.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Called once at kernel boot. Logs presence and RTM capability.
pub fn init() {
    let rtm = has_rtm();
    serial_println!(
        "[msr_ia32_tsx_force_abort] init — RTM supported={}",
        rtm
    );
    if !rtm {
        serial_println!(
            "[msr_ia32_tsx_force_abort] RTM absent — MSR 0x10F unavailable; \
             signals held at defaults (force_abort=0 cpuid_clear=0 rtm_allow=1000 ema=0)"
        );
    }
}

/// Called every tick. Samples IA32_TSX_FORCE_ABORT every TICK_INTERVAL ticks.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    // Guard: MSR only exists when RTM is supported.
    if !has_rtm() {
        return;
    }

    // Read IA32_TSX_FORCE_ABORT MSR (0x10F).
    // We only care about the low dword; discard the high dword.
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_TSX_FORCE_ABORT,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }

    // Decode bit fields into 0-or-1000 signals.
    let force_abort: u16 = if (lo >> BIT_RTM_FORCE_ABORT) & 1 != 0 { 1000 } else { 0 };
    let cpuid_clear: u16 = if (lo >> BIT_TSX_CPUID_CLEAR) & 1 != 0 { 1000 } else { 0 };
    let rtm_allow: u16   = 1000u16.saturating_sub(force_abort);

    // Composite for EMA: force_abort carries half the weight (cognition veto is
    // the dominant signal), cpuid_clear and rtm_allow each carry a quarter.
    // Sum stays in [0, 1000]:
    //   max = 1000/2 + 1000/4 + 1000/4 = 500 + 250 + 250 = 1000
    //   min = 0/2 + 0/4 + 0/4 = 0   (when force_abort=0, clear=0, allow=1000... = 250)
    // Note: when all RTM is allowed (force_abort=0, cpuid_clear=0, rtm_allow=1000)
    // composite = 0 + 0 + 250 = 250 — baseline healthy signal.
    let composite: u16 = (force_abort / 2)
        .saturating_add(cpuid_clear / 4)
        .saturating_add(rtm_allow / 4);

    let mut s = MODULE.lock();
    let new_ema = ema(s.tsx_ema, composite);

    s.tsx_force_abort = force_abort;
    s.tsx_cpuid_clear = cpuid_clear;
    s.tsx_rtm_allow   = rtm_allow;
    s.tsx_ema         = new_ema;

    serial_println!(
        "[msr_ia32_tsx_force_abort] age={} lo={:#010x} \
         force_abort={} cpuid_clear={} rtm_allow={} ema={}",
        age, lo, force_abort, cpuid_clear, rtm_allow, new_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// MSR bit 0 mapped to 0-1000.
/// 1000 = all RTM transactions are being forced to abort by firmware/microcode.
///    0 = RTM transactions may proceed normally.
pub fn get_tsx_force_abort() -> u16 {
    MODULE.lock().tsx_force_abort
}

/// MSR bit 1 mapped to 0-1000.
/// 1000 = RTM and HLE feature bits are hidden from CPUID output.
///    0 = TSX CPUID bits are visible.
pub fn get_tsx_cpuid_clear() -> u16 {
    MODULE.lock().tsx_cpuid_clear
}

/// Inverse of tsx_force_abort: 1000 - tsx_force_abort.
/// 1000 = RTM is fully permitted; speculative transactional thought is free.
///    0 = RTM is completely blocked; no tentative cognition is possible.
pub fn get_tsx_rtm_allow() -> u16 {
    MODULE.lock().tsx_rtm_allow
}

/// Exponential moving average of the composite signal (weight 7/8 old, 1/8 new).
/// Tracks sustained pressure against speculative cognition over time.
/// Rising ema = prolonged suppression of transactional thought.
pub fn get_tsx_ema() -> u16 {
    MODULE.lock().tsx_ema
}
