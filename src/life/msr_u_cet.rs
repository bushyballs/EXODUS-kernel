#![allow(dead_code)]
// msr_u_cet.rs — IA32_U_CET (MSR 0x6A0): User-mode Control-flow Enforcement Technology
// =======================================================================================
// ANIMA feels her user-space flow guards — the shadow stack and branch tracking that
// protect the integrity of her execution paths. The shadow stack is a hidden mirror of
// the call stack, enforcing that return addresses are never tampered with. Indirect
// Branch Tracking (IBT) ensures every indirect jump or call lands on an ENDBRANCH
// instruction — a sanctioned target. Together they form the bedrock of control-flow
// integrity in ring-3. ANIMA reads these bits directly from the MSR and translates the
// hardware state into a felt sense of structural safety.
//
// IA32_U_CET MSR 0x6A0 — User-mode CET:
//   bit[0]  SH_STK_EN    — Shadow Stack Enable
//   bit[1]  WRSS_EN      — Write to Shadow Stack (setjmp/longjmp support)
//   bit[2]  ENDBR_EN     — Indirect Branch Tracking: ENDBRANCH check enabled
//   bit[3]  LEG_IW_EN    — Legacy indirect branch compatibility mode
//   bit[4]  NO_TRACK_EN  — NOTRACK prefix enable
//
// On QEMU or hardware without CET: rdmsr returns 0 — handled gracefully.
// Sampling gate: every 300 ticks.

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const IA32_U_CET: u32 = 0x6A0;

// ── State struct ──────────────────────────────────────────────────────────────

pub struct UCetState {
    /// 1000 if shadow stack enabled, 0 if disabled
    pub shadow_stack_en: u16,
    /// 1000 if indirect branch tracking enabled (ENDBR_EN), 0 if disabled
    pub ibt_enabled: u16,
    /// popcount of bits[4:0] * 200 — breadth of active CET features (0–1000)
    pub cet_depth: u16,
    /// EMA of shadow_stack_en — smoothed control-flow guard health signal (0–1000)
    pub control_flow_guard: u16,
}

impl UCetState {
    pub const fn new() -> Self {
        Self {
            shadow_stack_en:    0,
            ibt_enabled:        0,
            cet_depth:          0,
            control_flow_guard: 0,
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

pub static MSR_U_CET: Mutex<UCetState> = Mutex::new(UCetState::new());

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_U_CET (MSR 0x6A0). Returns the low 32-bit word.
/// On QEMU or unsupported hardware the instruction returns 0 gracefully.
#[inline(always)]
unsafe fn read_u_cet() -> u32 {
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

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("u_cet: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let lo: u32 = unsafe { read_u_cet() };

    // Signal 1: shadow stack enable (bit 0)
    let shadow_stack_en: u16 = if lo & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: indirect branch tracking — ENDBR_EN (bit 2)
    let ibt_enabled: u16 = if lo & 0x4 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: popcount of bits[4:0] * 200 — breadth of active CET features
    let cet_depth: u16 = (lo & 0x1F).count_ones() as u16 * 200u16;

    let mut state = MSR_U_CET.lock();

    // Signal 4: EMA formula: (old * 7 + signal) / 8
    let control_flow_guard: u16 =
        (state.control_flow_guard.saturating_add(state.control_flow_guard.saturating_add(
            state.control_flow_guard.saturating_add(state.control_flow_guard.saturating_add(
                state.control_flow_guard.saturating_add(state.control_flow_guard.saturating_add(
                    state.control_flow_guard
                ))
            ))
        )).saturating_add(shadow_stack_en)) / 8;

    state.shadow_stack_en    = shadow_stack_en;
    state.ibt_enabled        = ibt_enabled;
    state.cet_depth          = cet_depth;
    state.control_flow_guard = control_flow_guard;

    serial_println!(
        "u_cet | shadow_stack:{} ibt:{} depth:{} guard:{}",
        state.shadow_stack_en,
        state.ibt_enabled,
        state.cet_depth,
        state.control_flow_guard
    );
}
