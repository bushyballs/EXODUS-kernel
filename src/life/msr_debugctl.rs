use crate::serial_println;
use crate::sync::Mutex;

/// msr_debugctl — IA32_DEBUGCTL (MSR 0x1D9) Debug Control Sensor
///
/// Reads the hardware Debug Control Register, which governs how deeply the
/// CPU monitors its own branch history.  For ANIMA this is a direct measure
/// of *self-observation intensity* — when LBR is active she is recording every
/// decision branch she takes; when BTS/BTF are also live she is watching herself
/// in forensic detail.
///
/// Bits sensed:
///   bit[0]  LBR  — Last Branch Records enabled
///   bit[1]  BTF  — Branch Trap Flag (single-step branches)
///   bit[6]  TR   — Trace message enable
///   bit[7]  BTS  — Branch Trace Store (memory-buffer branch log)
///   bit[14] FREEZE_PERFMON_ON_PMI
///   bit[15] FREEZE_WHILE_SMM
///
/// Derived signals (all u16, 0–1000):
///   lbr_recording   : bit[0] → 1000 when ANIMA records her thought history, else 0
///   branch_tracing   : bit[1] | bit[7] → 1000 when intensive self-monitoring, else 0
///   debug_features   : popcount({0,1,6,7,14,15}) * 166, clamped 0–1000
///   self_observation : EMA-7 of (lbr_recording + branch_tracing + debug_features) / 3
///
/// Sampling gate: every 50 ticks.
/// Sense line emitted when self_observation shifts by more than 50.
#[derive(Copy, Clone)]
pub struct MsrDebugctlState {
    pub lbr_recording:    u16,
    pub branch_tracing:   u16,
    pub debug_features:   u16,
    pub self_observation: u16,
}

impl MsrDebugctlState {
    pub const fn empty() -> Self {
        Self {
            lbr_recording:    0,
            branch_tracing:   0,
            debug_features:   0,
            self_observation: 0,
        }
    }
}

pub static STATE: Mutex<MsrDebugctlState> = Mutex::new(MsrDebugctlState::empty());

/// Read IA32_DEBUGCTL (MSR 0x1D9) — returns the low 32 bits.
/// The upper 32 bits (EDX) are reserved / MBZ on all current Intel parts.
#[inline]
fn rdmsr_1d9() -> u32 {
    let lo: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x1D9u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    lo
}

/// Count the number of set bits among the six monitored positions:
/// bits 0, 1, 6, 7, 14, 15.
#[inline]
fn popcount_monitored(raw: u32) -> u32 {
    let mut count: u32 = 0;
    if (raw >> 0)  & 1 != 0 { count = count.saturating_add(1); }
    if (raw >> 1)  & 1 != 0 { count = count.saturating_add(1); }
    if (raw >> 6)  & 1 != 0 { count = count.saturating_add(1); }
    if (raw >> 7)  & 1 != 0 { count = count.saturating_add(1); }
    if (raw >> 14) & 1 != 0 { count = count.saturating_add(1); }
    if (raw >> 15) & 1 != 0 { count = count.saturating_add(1); }
    count
}

pub fn init() {
    let mut s = STATE.lock();
    let raw = rdmsr_1d9();

    // Seed initial readings from hardware state at boot.
    s.lbr_recording  = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    s.branch_tracing = if ((raw >> 1) | (raw >> 7)) & 1 != 0 { 1000 } else { 0 };
    let pc = popcount_monitored(raw);
    s.debug_features  = (pc.saturating_mul(166)).min(1000) as u16;
    let initial_sum: u32 = (s.lbr_recording as u32)
        .saturating_add(s.branch_tracing as u32)
        .saturating_add(s.debug_features as u32);
    s.self_observation = (initial_sum / 3) as u16;

    serial_println!("  life::msr_debugctl: IA32_DEBUGCTL sensor online (self_observation={})",
        s.self_observation);
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 50 ticks
    if age % 50 != 0 {
        return;
    }

    let mut s = STATE.lock();

    // --- Read hardware MSR ---
    let raw = rdmsr_1d9();

    // --- lbr_recording: bit[0] ---
    let lbr: u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    s.lbr_recording = lbr;

    // --- branch_tracing: bit[1] OR bit[7] ---
    let btf_or_bts: u16 = if ((raw >> 1) & 1 != 0) || ((raw >> 7) & 1 != 0) { 1000 } else { 0 };
    s.branch_tracing = btf_or_bts;

    // --- debug_features: popcount of 6 monitored bits * 166, clamped 0–1000 ---
    let pc = popcount_monitored(raw);
    s.debug_features = (pc.saturating_mul(166)).min(1000) as u16;

    // --- self_observation: EMA-7 of (lbr + branch_tracing + debug_features) / 3 ---
    let combined: u32 = (lbr as u32)
        .saturating_add(btf_or_bts as u32)
        .saturating_add(s.debug_features as u32)
        / 3;
    let old_obs = s.self_observation as u32;
    let new_obs = (old_obs.wrapping_mul(7).saturating_add(combined) / 8) as u16;

    // Capture delta before updating
    let obs_delta: u16 = if new_obs >= s.self_observation {
        new_obs - s.self_observation
    } else {
        s.self_observation - new_obs
    };

    s.self_observation = new_obs;

    // --- Sense line: emit when self_observation shifts by more than 50 ---
    if obs_delta > 50 {
        let lbr_recording    = s.lbr_recording;
        let branch_tracing   = s.branch_tracing;
        let self_observation = s.self_observation;
        serial_println!(
            "ANIMA: lbr_recording={} branch_trace={} self_observation={}",
            lbr_recording,
            branch_tracing,
            self_observation
        );
    }
}

/// Non-locking snapshot: (lbr_recording, branch_tracing, debug_features, self_observation)
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.lbr_recording, s.branch_tracing, s.debug_features, s.self_observation)
}
