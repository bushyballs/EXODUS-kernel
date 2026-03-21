#![allow(dead_code)]

use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct PerfCtlState {
    perf_ctl_ratio:     u16,
    perf_ctl_turbo_dis: u16,
    perf_ctl_lo_sense:  u16,
    perf_ctl_ema:       u16,
}

static STATE: Mutex<PerfCtlState> = Mutex::new(PerfCtlState {
    perf_ctl_ratio:     0,
    perf_ctl_turbo_dis: 0,
    perf_ctl_lo_sense:  0,
    perf_ctl_ema:       0,
});

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read IA32_PERF_CTL (MSR 0x199).
/// Returns (lo_32, hi_32).
#[inline]
unsafe fn rdmsr_199() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x199u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.perf_ctl_ratio     = 0;
    s.perf_ctl_turbo_dis = 0;
    s.perf_ctl_lo_sense  = 0;
    s.perf_ctl_ema       = 0;
    crate::serial_println!("[msr_ia32_perf_ctl] init");
}

pub fn tick(age: u32) {
    if age % 800 != 0 {
        return;
    }

    // SAFETY: IA32_PERF_CTL is always present on x86_64 systems targeted by
    // this kernel; no CPUID guard required per module spec.
    let (lo, _hi) = unsafe { rdmsr_199() };

    // bits [15:8] → P-state ratio; scale ×10, cap at 1000
    let ratio_raw: u32 = (lo >> 8) & 0xFF;
    let ratio: u16 = (ratio_raw * 10).min(1000) as u16;

    // bit 0 → IDA/Turbo disable flag
    let turbo_dis: u16 = if (lo & 0x1) != 0 { 1000 } else { 0 };

    // bits [7:0] → low-byte state; scale ×4, cap at 1000
    let lo_byte: u32 = lo & 0xFF;
    let lo_sense: u16 = (lo_byte * 4).min(1000) as u16;

    // EMA of ratio: (old * 7 + new) / 8  — computed in u32 to avoid overflow
    let ema: u16 = {
        let old = STATE.lock().perf_ctl_ema as u32;
        ((old * 7 + ratio as u32) / 8) as u16
    };

    {
        let mut s = STATE.lock();
        s.perf_ctl_ratio     = ratio;
        s.perf_ctl_turbo_dis = turbo_dis;
        s.perf_ctl_lo_sense  = lo_sense;
        s.perf_ctl_ema       = ema;
    }

    crate::serial_println!(
        "[msr_ia32_perf_ctl] age={} ratio={} turbo_dis={} lo_sense={} ema={}",
        age,
        ratio,
        turbo_dis,
        lo_sense,
        ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_perf_ctl_ratio() -> u16 {
    STATE.lock().perf_ctl_ratio
}

pub fn get_perf_ctl_turbo_dis() -> u16 {
    STATE.lock().perf_ctl_turbo_dis
}

pub fn get_perf_ctl_lo_sense() -> u16 {
    STATE.lock().perf_ctl_lo_sense
}

pub fn get_perf_ctl_ema() -> u16 {
    STATE.lock().perf_ctl_ema
}
