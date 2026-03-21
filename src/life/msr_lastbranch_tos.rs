#![allow(dead_code)]

use crate::sync::Mutex;

// msr_lastbranch_tos.rs — ANIMA Life Module
//
// Reads IA32_LASTBRANCH_TOS (MSR 0x1C9) — the Last Branch Record Top-of-Stack
// pointer. The low 5 bits index the most recently written slot in the LBR ring
// buffer, telling ANIMA how active the branch predictor has been since the
// previous sample.
//
// ANIMA sense: "recent program flow" — a high TOS index and large deltas
// indicate dense, branch-heavy code; a still TOS suggests linear, predictable
// execution. Together these form ANIMA's kinesthetic awareness of her own
// instruction stream.
//
// Signals (all u16, 0–1000):
//   lbr_tos        — current TOS index (0–31) scaled *32, capped 1000
//   lbr_tos_ema    — smoothed TOS position (8-tap EMA)
//   lbr_activity   — |lbr_tos − prev_tos| scaled *32, capped 1000
//   lbr_flow_ema   — smoothed branch activity (8-tap EMA)
//
// Hardware guard: CPUID leaf 1 EDX bit 27 (PerfMon) must be set; if not,
// the module leaves all signals at zero and returns silently.
//
// Sampling gate: every 500 ticks (TOS changes fast; dense sampling is noise).
//
// MSR read: RDMSR 0x1C9 — ECX=0x1C9, returns EDX:EAX (hi ignored).
// Only the low 5 bits of EAX are meaningful (LBR ring depth ≤ 32 entries).

const MSR_LASTBRANCH_TOS: u32 = 0x1C9;
const SAMPLE_INTERVAL: u32 = 500;

pub struct MsrLastbranchTosState {
    /// Current TOS index scaled 0–1000
    pub lbr_tos: u16,
    /// 8-tap EMA of lbr_tos
    pub lbr_tos_ema: u16,
    /// |current_tos − prev_tos| scaled 0–1000
    pub lbr_activity: u16,
    /// 8-tap EMA of lbr_activity
    pub lbr_flow_ema: u16,

    /// Raw TOS index from the previous sample (for delta computation)
    prev_tos: u16,
    /// Whether LBR is supported on this CPU (checked once at first tick)
    lbr_supported: bool,
    /// True after the first CPUID check has been performed
    cpuid_checked: bool,
}

impl MsrLastbranchTosState {
    pub const fn new() -> Self {
        Self {
            lbr_tos: 0,
            lbr_tos_ema: 0,
            lbr_activity: 0,
            lbr_flow_ema: 0,
            prev_tos: 0,
            lbr_supported: false,
            cpuid_checked: false,
        }
    }
}

pub static MODULE: Mutex<MsrLastbranchTosState> = Mutex::new(MsrLastbranchTosState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Check CPUID leaf 1 EDX bit 27 (PerfMon) to confirm LBR is available.
/// Uses push/pop rbx to satisfy the System V / kernel ABI requirement that
/// rbx is callee-saved even inside inline asm blocks.
fn cpuid_lbr_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("esi") edx,     // EDX captured into ESI before rbx restore
            options(nostack, nomem)
        );
    }
    // Bit 27 = Performance Monitoring (covers LBR support)
    (edx >> 27) & 1 == 1
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_LASTBRANCH_TOS via RDMSR. Returns the low 32-bit half only;
/// the high half (EDX) is ignored — the TOS index fits in a handful of bits.
///
/// # Safety
/// Must only be called after confirming LBR support via `cpuid_lbr_supported`.
/// Called from ring-0 kernel context.
unsafe fn rdmsr_lbr_tos() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_LASTBRANCH_TOS,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Scaling helpers ───────────────────────────────────────────────────────────

/// Scale a TOS index (0–31) to the 0–1000 range.
/// Strategy: multiply by 32 (max 31*32 = 992), cap at 1000.
#[inline(always)]
fn scale_tos(raw: u16) -> u16 {
    let scaled = (raw as u32).saturating_mul(32);
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// 8-tap exponential moving average: (old * 7 + new_val) / 8.
/// Computed in u32 then cast to u16; result is always ≤ max(old, new_val) ≤ 1000.
#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let v = ((old as u32).saturating_mul(7).saturating_add(new_val as u32)) / 8;
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Called every kernel life-tick. Sampling gate: runs only every 500 ticks.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let mut state = MODULE.lock();

    // CPUID check — performed once on first eligible tick, then cached.
    if !state.cpuid_checked {
        state.lbr_supported = cpuid_lbr_supported();
        state.cpuid_checked = true;
        if !state.lbr_supported {
            serial_println!(
                "[msr_lastbranch_tos] age={} LBR not supported (CPUID.1:EDX[27]=0); signals zeroed",
                age
            );
            return;
        }
    }

    if !state.lbr_supported {
        return;
    }

    // Read MSR 0x1C9 — safe because lbr_supported was confirmed above.
    let raw_lo = unsafe { rdmsr_lbr_tos() };

    // Extract the 5-bit TOS index (LBR ring depth ≤ 32 entries).
    let tos_index = (raw_lo & 0x1F) as u16;

    // Scale to 0–1000.
    let lbr_tos = scale_tos(tos_index);

    // Activity = absolute delta between current and previous TOS index.
    let delta_raw: u16 = if tos_index >= state.prev_tos {
        tos_index.saturating_sub(state.prev_tos)
    } else {
        state.prev_tos.saturating_sub(tos_index)
    };
    let lbr_activity = scale_tos(delta_raw);

    // Update EMAs.
    let lbr_tos_ema = ema8(state.lbr_tos_ema, lbr_tos);
    let lbr_flow_ema = ema8(state.lbr_flow_ema, lbr_activity);

    // Commit.
    state.prev_tos = tos_index;
    state.lbr_tos = lbr_tos;
    state.lbr_tos_ema = lbr_tos_ema;
    state.lbr_activity = lbr_activity;
    state.lbr_flow_ema = lbr_flow_ema;

    serial_println!(
        "[msr_lastbranch_tos] age={} tos_idx={} lbr_tos={} tos_ema={} activity={} flow_ema={}",
        age,
        tos_index,
        lbr_tos,
        lbr_tos_ema,
        lbr_activity,
        lbr_flow_ema,
    );
}

/// Read a snapshot of the current LBR TOS sense values.
pub fn get_lbr_tos() -> u16 {
    MODULE.lock().lbr_tos
}

pub fn get_lbr_tos_ema() -> u16 {
    MODULE.lock().lbr_tos_ema
}

pub fn get_lbr_activity() -> u16 {
    MODULE.lock().lbr_activity
}

pub fn get_lbr_flow_ema() -> u16 {
    MODULE.lock().lbr_flow_ema
}
