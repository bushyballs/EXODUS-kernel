#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ──────────────────────────────────────────────────────────────
const MSR_PKG_PERF_STATUS: u32 = 0x613;

// ── Sampling gate ─────────────────────────────────────────────────────────────
const SAMPLE_EVERY: u32 = 500;

// ── State ─────────────────────────────────────────────────────────────────────
struct RaplPkgPerfState {
    throttle_lo:     u16, // low 16 bits of current reading mapped 0-1000
    throttle_delta:  u16, // delta since last tick mapped 0-1000
    throttle_active: u16, // 1000 if throttling is happening now, 0 if not
    throttle_ema:    u16, // EMA of throttle_active
    last_lo:         u32, // raw low 32 bits of previous MSR read
}

impl RaplPkgPerfState {
    const fn new() -> Self {
        Self {
            throttle_lo:     0,
            throttle_delta:  0,
            throttle_active: 0,
            throttle_ema:    0,
            last_lo:         0,
        }
    }
}

static STATE: Mutex<RaplPkgPerfState> = Mutex::new(RaplPkgPerfState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────
fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_val >> 4) & 1 != 0
}

// ── Raw MSR read ──────────────────────────────────────────────────────────────
/// Read an MSR; returns (edx, eax) — high 32 bits and low 32 bits respectively.
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (hi, lo)
}

// ── Mapping helper: scale a u32 value into 0-1000 ────────────────────────────
/// Maps `val` in range [0, max] → [0, 1000].
/// Uses integer arithmetic only; never divides by zero.
fn scale_to_1000(val: u32, max: u32) -> u16 {
    if max == 0 {
        return 0;
    }
    let scaled = (val as u64 * 1000u64) / max as u64;
    if scaled > 1000 {
        1000
    } else {
        scaled as u16
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_rapl() {
        crate::serial_println!(
            "[msr_rapl_pkg_perf_status] RAPL not supported on this CPU — module disabled"
        );
        return;
    }

    let raw_lo = unsafe {
        let (_hi, lo) = rdmsr(MSR_PKG_PERF_STATUS);
        lo
    };

    let mut state = STATE.lock();
    state.last_lo = raw_lo;

    crate::serial_println!(
        "[msr_rapl_pkg_perf_status] init: RAPL supported, seed last_lo={}",
        raw_lo
    );
}

pub fn tick(age: u32) {
    // Sampling gate
    if age % SAMPLE_EVERY != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let raw_lo = unsafe {
        let (_hi, lo) = rdmsr(MSR_PKG_PERF_STATUS);
        lo
    };

    let mut state = STATE.lock();

    // throttle_lo: low 16 bits of raw_lo mapped to 0-1000
    let lo16 = (raw_lo & 0xFFFF) as u32;
    let throttle_lo = scale_to_1000(lo16, 0xFFFF_u32);

    // throttle_delta: wrapping delta of raw_lo since last sample, mapped to 0-1000
    // Use wrapping subtraction to handle counter rollover gracefully.
    let delta = raw_lo.wrapping_sub(state.last_lo);
    // A delta of u32::MAX is the largest possible single-sample movement;
    // clamp the mapping domain at 0xFFFF for a useful 0-1000 range.
    let delta_clamped = if delta > 0xFFFF { 0xFFFF } else { delta };
    let throttle_delta = scale_to_1000(delta_clamped, 0xFFFF_u32);

    // throttle_active: 1000 if any throttle increment happened, 0 otherwise
    let throttle_active: u16 = if delta > 0 { 1000 } else { 0 };

    // throttle_ema: EMA of throttle_active
    // EMA formula: (old * 7 + new_val) / 8  — computed in u32, cast to u16
    let ema_u32 = (state.throttle_ema as u32 * 7 + throttle_active as u32) / 8;
    let throttle_ema = ema_u32 as u16;

    // Update state
    state.last_lo         = raw_lo;
    state.throttle_lo     = throttle_lo;
    state.throttle_delta  = throttle_delta;
    state.throttle_active = throttle_active;
    state.throttle_ema    = throttle_ema;

    crate::serial_println!(
        "[msr_rapl_pkg_perf_status] age={} throttle={} delta={} active={} ema={}",
        age,
        throttle_lo,
        throttle_delta,
        throttle_active,
        throttle_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_throttle_lo() -> u16 {
    STATE.lock().throttle_lo
}

pub fn get_throttle_delta() -> u16 {
    STATE.lock().throttle_delta
}

pub fn get_throttle_active() -> u16 {
    STATE.lock().throttle_active
}

pub fn get_throttle_ema() -> u16 {
    STATE.lock().throttle_ema
}
