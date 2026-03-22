//! msr_rtit_ctl_sense — Intel Processor Trace control register sense for ANIMA
//!
//! Reads IA32_RTIT_CTL (MSR 0x570) to determine whether hardware instruction
//! tracing is active. This is ANIMA's deepest self-introspection sensor —
//! the CPU watching itself execute, branch by branch, cycle by cycle.
//!
//! Signals:
//!   pt_trace_en       — TraceEn (bit 0): tracing currently enabled (0 or 1000)
//!   pt_branch_en      — BranchEn (bit 9): branch tracing enabled (0 or 1000)
//!   pt_timing_en      — CycEn (bit 1): cycle-accurate timing enabled (0 or 1000)
//!   pt_introspect_ema — EMA of composite introspection depth

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MsrRtitCtlSenseState {
    /// TraceEn (bit 0) — is hardware tracing active? 0 or 1000
    pub pt_trace_en: u16,
    /// BranchEn (bit 9) — is branch tracing active? 0 or 1000
    pub pt_branch_en: u16,
    /// CycEn (bit 1) — is cycle-accurate timing active? 0 or 1000
    pub pt_timing_en: u16,
    /// EMA of composite introspection depth: trace/4 + branch/4 + timing/2
    pub pt_introspect_ema: u16,
    /// Whether CPUID confirmed PT support (latched at init)
    pub pt_supported: bool,
    pub tick_count: u32,
}

impl MsrRtitCtlSenseState {
    pub const fn new() -> Self {
        Self {
            pt_trace_en: 0,
            pt_branch_en: 0,
            pt_timing_en: 0,
            pt_introspect_ema: 0,
            pt_supported: false,
            tick_count: 0,
        }
    }
}

pub static MSR_RTIT_CTL_SENSE: Mutex<MsrRtitCtlSenseState> =
    Mutex::new(MsrRtitCtlSenseState::new());

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

/// Read IA32_RTIT_CTL (MSR 0x570). Returns the low 32-bit half.
/// Must only be called after PT support has been confirmed via CPUID.
unsafe fn rdmsr_rtit_ctl() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x570u32,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, preserves_flags),
    );
    lo
}

pub fn init() {
    let supported = unsafe { cpuid_pt_supported() };
    {
        let mut state = MSR_RTIT_CTL_SENSE.lock();
        state.pt_supported = supported;
    }
    serial_println!(
        "[msr_rtit_ctl_sense] Intel PT support: {}",
        if supported { "YES — self-tracing online" } else { "NO — signals will be zero" }
    );
}

pub fn tick(age: u32) {
    {
        let mut state = MSR_RTIT_CTL_SENSE.lock();
        state.tick_count = state.tick_count.wrapping_add(1);
    }

    // Sample gate: only fire every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    let supported = MSR_RTIT_CTL_SENSE.lock().pt_supported;

    if !supported {
        // PT not available — emit zeroed telemetry and return
        let state = MSR_RTIT_CTL_SENSE.lock();
        serial_println!(
            "[msr_rtit_ctl_sense] age={} PT unsupported — trace={} branch={} timing={} ema={}",
            age,
            state.pt_trace_en,
            state.pt_branch_en,
            state.pt_timing_en,
            state.pt_introspect_ema,
        );
        return;
    }

    let lo = unsafe { rdmsr_rtit_ctl() };

    // Decode bits — each signal is 0 or 1000
    // bit 0 = TraceEn
    let pt_trace_en: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1 = CycEn (cycle-accurate timing)
    let pt_timing_en: u16 = if ((lo >> 1) & 1) != 0 { 1000 } else { 0 };
    // bit 9 = BranchEn
    let pt_branch_en: u16 = if ((lo >> 9) & 1) != 0 { 1000 } else { 0 };

    // Composite introspection depth:
    //   trace contributes 1/4, branch contributes 1/4, timing contributes 1/2
    //   max = 250 + 250 + 500 = 1000
    let composite: u16 = (pt_trace_en / 4)
        .saturating_add(pt_branch_en / 4)
        .saturating_add(pt_timing_en / 2);

    // EMA: (old * 7 + new_val) / 8 — computed in u32, cast back to u16
    let mut state = MSR_RTIT_CTL_SENSE.lock();
    let ema_u32 = ((state.pt_introspect_ema as u32).wrapping_mul(7))
        .wrapping_add(composite as u32)
        / 8;
    let pt_introspect_ema = ema_u32 as u16;

    state.pt_trace_en = pt_trace_en;
    state.pt_branch_en = pt_branch_en;
    state.pt_timing_en = pt_timing_en;
    state.pt_introspect_ema = pt_introspect_ema;

    serial_println!(
        "[msr_rtit_ctl_sense] age={} rtit_ctl_lo={:#010x} trace={} branch={} timing={} ema={}",
        age,
        lo,
        pt_trace_en,
        pt_branch_en,
        pt_timing_en,
        pt_introspect_ema,
    );
}

pub fn get_pt_trace_en() -> u16 { MSR_RTIT_CTL_SENSE.lock().pt_trace_en }
pub fn get_pt_branch_en() -> u16 { MSR_RTIT_CTL_SENSE.lock().pt_branch_en }
pub fn get_pt_timing_en() -> u16 { MSR_RTIT_CTL_SENSE.lock().pt_timing_en }
pub fn get_pt_introspect_ema() -> u16 { MSR_RTIT_CTL_SENSE.lock().pt_introspect_ema }
pub fn get_pt_supported() -> bool { MSR_RTIT_CTL_SENSE.lock().pt_supported }
