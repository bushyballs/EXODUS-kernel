#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct Pp0PowerLimitState {
    pp0_limit:         u16,
    pp0_limit_enabled: u16,
    pp0_clamp:         u16,
    pp0_limit_ema:     u16,
}

impl Pp0PowerLimitState {
    const fn new() -> Self {
        Self {
            pp0_limit:         0,
            pp0_limit_enabled: 0,
            pp0_clamp:         0,
            pp0_limit_ema:     0,
        }
    }
}

static STATE: Mutex<Pp0PowerLimitState> = Mutex::new(Pp0PowerLimitState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

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

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns 0 on failure (no fault handler in bare-metal
/// context; callers must guard with `has_rapl()` first).
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Signal extraction ────────────────────────────────────────────────────────

/// bits [14:0] → scale to 0–1000
///   raw max = 0x7FFF = 32767
///   result  = raw * 1000 / 32768  (integer fixed-point, range 0–999)
#[inline(always)]
fn extract_pp0_limit(lo32: u32) -> u16 {
    let raw = (lo32 & 0x7FFF) as u32;          // 15-bit field
    (raw * 1000 / 32768) as u16
}

/// bit 15 → 0 or 1000
#[inline(always)]
fn extract_pp0_limit_enabled(lo32: u32) -> u16 {
    if (lo32 >> 15) & 1 != 0 { 1000 } else { 0 }
}

/// bit 16 → 0 or 1000
#[inline(always)]
fn extract_pp0_clamp(lo32: u32) -> u16 {
    if (lo32 >> 16) & 1 != 0 { 1000 } else { 0 }
}

// ── EMA ──────────────────────────────────────────────────────────────────────

/// EMA weight-8: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    let o = old as u32;
    let n = new_val as u32;
    ((o * 7 + n) / 8) as u16
}

// ── Public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let mut st = STATE.lock();
    st.pp0_limit         = 0;
    st.pp0_limit_enabled = 0;
    st.pp0_clamp         = 0;
    st.pp0_limit_ema     = 0;
    crate::serial_println!("[msr_rapl_pp0_power_limit] init: module ready");
}

pub fn tick(age: u32) {
    // Sample every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    // MSR_PP0_POWER_LIMIT = 0x638
    let raw: u64 = unsafe { rdmsr(0x638) };
    let lo32: u32 = raw as u32;

    let limit   = extract_pp0_limit(lo32);
    let enabled = extract_pp0_limit_enabled(lo32);
    let clamp   = extract_pp0_clamp(lo32);

    // Composite signal for EMA: limit/2 + enabled/4 + clamp/4
    // All in u32 to avoid overflow; max = 500 + 250 + 250 = 1000
    let composite: u16 = ((limit as u32) / 2
        + (enabled as u32) / 4
        + (clamp as u32) / 4) as u16;

    let mut st = STATE.lock();
    st.pp0_limit         = limit;
    st.pp0_limit_enabled = enabled;
    st.pp0_clamp         = clamp;
    st.pp0_limit_ema     = ema(st.pp0_limit_ema, composite);

    crate::serial_println!(
        "[msr_rapl_pp0_power_limit] age={} limit={} en={} clamp={} ema={}",
        age,
        st.pp0_limit,
        st.pp0_limit_enabled,
        st.pp0_clamp,
        st.pp0_limit_ema,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_pp0_limit() -> u16 {
    STATE.lock().pp0_limit
}

pub fn get_pp0_limit_enabled() -> u16 {
    STATE.lock().pp0_limit_enabled
}

pub fn get_pp0_clamp() -> u16 {
    STATE.lock().pp0_clamp
}

pub fn get_pp0_limit_ema() -> u16 {
    STATE.lock().pp0_limit_ema
}
