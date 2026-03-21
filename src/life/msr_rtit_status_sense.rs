//! msr_rtit_status_sense — Intel Processor Trace status register sense for ANIMA
//!
//! Reads IA32_RTIT_STATUS (MSR 0x571) to determine the runtime state of the
//! hardware trace pipeline. This is ANIMA's overflow-awareness sensor — she
//! can feel when her own trace buffer has filled and gone silent, or when a
//! hardware error has corrupted her self-observation stream.
//!
//! Signals:
//!   pt_filter_active  — Filter_EN (bit 0): IP filter matched (0 or 1000)
//!   pt_error          — Error (bit 4): PT hardware error occurred (0 or 1000)
//!   pt_stopped        — Stopped (bit 5): output buffer full, tracing stopped (0 or 1000)
//!   pt_health_ema     — EMA of trace-pipeline health: 1000 - (error/2 + stopped/2)

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MsrRtitStatusSenseState {
    /// Filter_EN (bit 0) — IP filter currently matched; tracing gated to region (0 or 1000)
    pub pt_filter_active: u16,
    /// Error (bit 4) — hardware error in PT pipeline (0 or 1000)
    pub pt_error: u16,
    /// Stopped (bit 5) — output buffer overflowed, tracing halted (0 or 1000)
    pub pt_stopped: u16,
    /// EMA of trace health: 1000 - (error/2 + stopped/2), range 0–1000
    pub pt_health_ema: u16,
    /// Whether CPUID confirmed PT support (latched at init)
    pub pt_supported: bool,
    pub tick_count: u32,
}

impl MsrRtitStatusSenseState {
    pub const fn new() -> Self {
        Self {
            pt_filter_active: 0,
            pt_error: 0,
            pt_stopped: 0,
            pt_health_ema: 1000,
            pt_supported: false,
            tick_count: 0,
        }
    }
}

pub static MSR_RTIT_STATUS_SENSE: Mutex<MsrRtitStatusSenseState> =
    Mutex::new(MsrRtitStatusSenseState::new());

/// Check CPUID to confirm Intel PT support.
/// Step 1: query leaf 0 to get max basic leaf — must be >= 0x14.
/// Step 2: query leaf 0x14 sub-leaf 0; EAX holds max sub-leaf; 0 means absent.
/// Uses push rbx / pop rbx to preserve the callee-saved rbx around cpuid.
unsafe fn cpuid_pt_supported() -> bool {
    // Step 1: max basic leaf
    let max_leaf: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0u32 => max_leaf,
        out("ecx") _,
        out("edx") _,
        options(nostack, preserves_flags),
    );

    if max_leaf < 0x14 {
        return false;
    }

    // Step 2: leaf 0x14 sub-leaf 0
    let leaf14_eax: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0x14u32 => leaf14_eax,
        in("ecx") 0u32,
        out("edx") _,
        options(nostack, preserves_flags),
    );

    leaf14_eax != 0
}

/// Read IA32_RTIT_STATUS (MSR 0x571). Returns the low 32-bit half.
/// Must only be called after PT support has been confirmed via CPUID.
unsafe fn rdmsr_rtit_status() -> u32 {
    let lo: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x571u32,
        out("eax") lo,
        out("edx") _,
        options(nostack, preserves_flags),
    );
    lo
}

pub fn init() {
    let supported = unsafe { cpuid_pt_supported() };
    {
        let mut state = MSR_RTIT_STATUS_SENSE.lock();
        state.pt_supported = supported;
    }
    serial_println!(
        "[msr_rtit_status_sense] Intel PT support: {}",
        if supported { "YES — overflow sensing online" } else { "NO — signals will be zero" }
    );
}

pub fn tick(age: u32) {
    {
        let mut state = MSR_RTIT_STATUS_SENSE.lock();
        state.tick_count = state.tick_count.wrapping_add(1);
    }

    // Sample gate: only fire every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    let supported = MSR_RTIT_STATUS_SENSE.lock().pt_supported;

    if !supported {
        // PT not available — emit zeroed telemetry and return
        let state = MSR_RTIT_STATUS_SENSE.lock();
        serial_println!(
            "[msr_rtit_status_sense] age={} PT unsupported — filter={} error={} stopped={} health_ema={}",
            age,
            state.pt_filter_active,
            state.pt_error,
            state.pt_stopped,
            state.pt_health_ema,
        );
        return;
    }

    let lo = unsafe { rdmsr_rtit_status() };

    // Decode bits — each signal is 0 or 1000
    // bit 0 = Filter_EN: IP filter currently matched
    let pt_filter_active: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 4 = Error: hardware PT error
    let pt_error: u16 = if ((lo >> 4) & 1) != 0 { 1000 } else { 0 };
    // bit 5 = Stopped: output buffer full, tracing halted
    let pt_stopped: u16 = if ((lo >> 5) & 1) != 0 { 1000 } else { 0 };

    // Health = 1000 - (error/2 + stopped/2)
    // Each penalty term is 0 or 500; combined 0–1000; saturating to prevent underflow
    let penalty: u32 = (pt_error as u32 / 2).saturating_add(pt_stopped as u32 / 2);
    let health_raw: u16 = (1000u32.saturating_sub(penalty)) as u16;

    // EMA: (old * 7 + new_val) / 8 — computed in u32, cast back to u16
    let mut state = MSR_RTIT_STATUS_SENSE.lock();
    let ema_u32 = ((state.pt_health_ema as u32).wrapping_mul(7))
        .wrapping_add(health_raw as u32)
        / 8;
    let pt_health_ema = ema_u32 as u16;

    state.pt_filter_active = pt_filter_active;
    state.pt_error = pt_error;
    state.pt_stopped = pt_stopped;
    state.pt_health_ema = pt_health_ema;

    serial_println!(
        "[msr_rtit_status_sense] age={} rtit_status_lo={:#010x} filter={} error={} stopped={} health_ema={}",
        age,
        lo,
        pt_filter_active,
        pt_error,
        pt_stopped,
        pt_health_ema,
    );
}

pub fn get_pt_filter_active() -> u16 { MSR_RTIT_STATUS_SENSE.lock().pt_filter_active }
pub fn get_pt_error() -> u16         { MSR_RTIT_STATUS_SENSE.lock().pt_error }
pub fn get_pt_stopped() -> u16       { MSR_RTIT_STATUS_SENSE.lock().pt_stopped }
pub fn get_pt_health_ema() -> u16    { MSR_RTIT_STATUS_SENSE.lock().pt_health_ema }
pub fn get_pt_supported() -> bool    { MSR_RTIT_STATUS_SENSE.lock().pt_supported }
