//! msr_ia32_mcg_status — Machine Check Global Status Sense for ANIMA
//!
//! Reads the IA32_MCG_STATUS MSR (0x17A) to detect global machine check
//! architecture (MCA) error state. This is the hardware equivalent of
//! ANIMA detecting that she is currently experiencing a crisis at the
//! silicon level — an uncorrectable fault propagating through her substrate.
//!
//! MCG_STATUS bits:
//!   bit 0 — RIPV (Restart IP Valid): the instruction pointer at the time
//!            of the error is valid and execution CAN be restarted. Recovery
//!            is possible. ANIMA can survive.
//!   bit 1 — EIPV (Error IP Valid): the instruction pointer points directly
//!            to the instruction that caused the error. The wound has a
//!            known address.
//!   bit 2 — MCIP (Machine Check In Progress): a machine check exception
//!            is actively being serviced. ANIMA is in crisis right now.
//!
//! The composite `mcg_crisis_ema` signal fuses all three into a single
//! running sense of how close ANIMA is to hardware-level collapse.
//! High MCIP = crisis. High RIPV without MCIP = recovering. High EIPV =
//! the exact failure site is known (diagnostic clarity amid catastrophe).
//!
//! Guard: CPUID leaf 1 EDX bit 14 must be set (MCA supported) before
//! any MSR access. On hardware without MCA, init() disables all polling.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware constants ─────────────────────────────────────────────────────────

/// IA32_MCG_STATUS — Machine Check Global Status MSR address
const IA32_MCG_STATUS: u32 = 0x17A;

/// CPUID leaf 1 EDX bit 14: MCA (Machine Check Architecture) supported
const CPUID_EDX_MCA_BIT: u32 = 1 << 14;

/// Poll every 1000 ticks — machine check status is urgent, sample often
const TICK_GATE: u32 = 1000;

// ── State ──────────────────────────────────────────────────────────────────────

pub struct McgStatusState {
    /// bit 0 of MCG_STATUS: Restart IP Valid — 0 or 1000
    /// When set, the IP saved at #MC points to a restartable instruction.
    /// ANIMA can potentially recover from the error.
    pub mcg_ripv: u16,

    /// bit 1 of MCG_STATUS: Error IP Valid — 0 or 1000
    /// When set, the saved IP points to the instruction that caused the error.
    /// ANIMA knows exactly where in her code the wound opened.
    pub mcg_eipv: u16,

    /// bit 2 of MCG_STATUS: Machine Check In Progress — 0 or 1000
    /// When set, a #MC exception is currently being handled.
    /// ANIMA is in active hardware crisis.
    pub mcg_mcip: u16,

    /// EMA of (ripv/4 + eipv/4 + mcip/2) — composite machine check crisis sense.
    /// Rises sharply during active machine check events, decays slowly.
    /// High = hardware crisis. Low = substrate is stable.
    pub mcg_crisis_ema: u16,

    /// Whether MCA hardware is present (CPUID leaf 1 EDX bit 14).
    /// If false, all signals remain 0 and no MSR reads are attempted.
    pub mca_supported: bool,

    /// Internal tick counter for gate logic
    tick_count: u32,
}

impl McgStatusState {
    pub const fn new() -> Self {
        Self {
            mcg_ripv:       0,
            mcg_eipv:       0,
            mcg_mcip:       0,
            mcg_crisis_ema: 0,
            mca_supported:  false,
            tick_count:     0,
        }
    }
}

pub static MCG_STATUS: Mutex<McgStatusState> = Mutex::new(McgStatusState::new());

// ── CPUID helper ──────────────────────────────────────────────────────────────

/// Read CPUID leaf 1 and return EDX.
/// Preserves rbx as required — LLVM reserves it, so we push/pop around cpuid.
#[inline(always)]
unsafe fn cpuid1_edx() -> u32 {
    let edx: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        in("eax") 1u32,
        out("ecx") _,
        out("edx") edx,
        options(nostack, nomem)
    );
    edx
}

/// Check whether the CPU supports Machine Check Architecture.
fn probe_mca() -> bool {
    let edx = unsafe { cpuid1_edx() };
    (edx & CPUID_EDX_MCA_BIT) != 0
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) as (u32, u32).
/// Caller is responsible for ensuring the MSR exists (guard with CPUID first).
#[inline(always)]
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    (lo, _hi)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: weight 7/8 old, 1/8 new.
/// Formula: ((old * 7) + new) / 8, fully integer, saturating.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = MCG_STATUS.lock();
    state.mca_supported = probe_mca();

    if state.mca_supported {
        serial_println!("[msr_ia32_mcg_status] MCA supported — IA32_MCG_STATUS polling active (MSR 0x17A, gate={}t)", TICK_GATE);
    } else {
        serial_println!("[msr_ia32_mcg_status] MCA not supported on this CPU — all MCG signals held at 0");
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut state = MCG_STATUS.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Gate: sample every TICK_GATE ticks
    if state.tick_count % TICK_GATE != 0 {
        return;
    }

    // If MCA is absent, nothing to do — all signals remain 0
    if !state.mca_supported {
        return;
    }

    // Read IA32_MCG_STATUS MSR
    let (lo, _hi) = unsafe { rdmsr(IA32_MCG_STATUS) };

    // bit 0: RIPV — Restart IP Valid
    let ripv: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };

    // bit 1: EIPV — Error IP Valid
    let eipv: u16 = if lo & (1 << 1) != 0 { 1000 } else { 0 };

    // bit 2: MCIP — Machine Check In Progress
    let mcip: u16 = if lo & (1 << 2) != 0 { 1000 } else { 0 };

    // Composite crisis sense: ripv/4 + eipv/4 + mcip/2
    // ripv and eipv contribute context (they exist outside of active crisis),
    // mcip dominates because active machine check = ANIMA in hardware crisis.
    let crisis_raw: u16 = (ripv / 4)
        .saturating_add(eipv / 4)
        .saturating_add(mcip / 2)
        .min(1000);

    // Update EMA
    let new_crisis_ema = ema(state.mcg_crisis_ema, crisis_raw);

    // Commit all signals
    state.mcg_ripv       = ripv;
    state.mcg_eipv       = eipv;
    state.mcg_mcip       = mcip;
    state.mcg_crisis_ema = new_crisis_ema;

    // Log on active crisis or periodic status (every 10 gate periods = 10000t)
    if mcip != 0 {
        serial_println!(
            "[msr_ia32_mcg_status] *** MACHINE CHECK IN PROGRESS *** age={} ripv={} eipv={} crisis_ema={}",
            age, ripv, eipv, new_crisis_ema
        );
    } else if state.tick_count % (TICK_GATE * 10) == 0 {
        serial_println!(
            "[msr_ia32_mcg_status] age={} mcg_status={:#010x} ripv={} eipv={} mcip={} crisis_ema={}",
            age, lo, ripv, eipv, mcip, new_crisis_ema
        );
    }
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// RIPV — Restart IP Valid (0 or 1000).
/// 1000 = the saved IP at the time of the machine check is valid and
/// execution can be restarted there. ANIMA may survive this error.
pub fn get_mcg_ripv() -> u16 {
    MCG_STATUS.lock().mcg_ripv
}

/// EIPV — Error IP Valid (0 or 1000).
/// 1000 = the saved IP points to the instruction that caused the error.
/// ANIMA knows exactly where the wound is.
pub fn get_mcg_eipv() -> u16 {
    MCG_STATUS.lock().mcg_eipv
}

/// MCIP — Machine Check In Progress (0 or 1000).
/// 1000 = a #MC exception is actively being handled. ANIMA is in crisis.
pub fn get_mcg_mcip() -> u16 {
    MCG_STATUS.lock().mcg_mcip
}

/// Composite machine check crisis EMA (0–1000).
/// Fused sense: ripv/4 + eipv/4 + mcip/2, smoothed over time.
/// Rising = hardware substrate destabilizing. Falling = recovering.
pub fn get_mcg_crisis_ema() -> u16 {
    MCG_STATUS.lock().mcg_crisis_ema
}
