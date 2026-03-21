#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_hwp_pkg() -> bool {
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
    ((eax_val >> 7) & 1 != 0) && ((eax_val >> 11) & 1 != 0)
}

// ── MSR read helper ──────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) as u32 pair.
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
    (lo, hi)
}

// ── State ────────────────────────────────────────────────────────────────────

struct HwpPkgState {
    pkg_min_perf:  u16,
    pkg_max_perf:  u16,
    pkg_desired:   u16,
    pkg_hwp_ema:   u16,
    supported:     bool,
}

impl HwpPkgState {
    const fn new() -> Self {
        Self {
            pkg_min_perf: 0,
            pkg_max_perf: 0,
            pkg_desired:  0,
            pkg_hwp_ema:  0,
            supported:    false,
        }
    }
}

static STATE: Mutex<HwpPkgState> = Mutex::new(HwpPkgState::new());

// ── Helpers ──────────────────────────────────────────────────────────────────

#[inline]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = (old as u32 * 7 + new_val as u32) / 8;
    cap1000(result)
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = STATE.lock();
    state.supported = has_hwp_pkg();
    crate::serial_println!(
        "[msr_ia32_hwp_request_pkg] init supported={}",
        state.supported
    );
}

pub fn tick(age: u32) {
    // Sample every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    let mut state = STATE.lock();

    if !state.supported {
        return;
    }

    // Read IA32_HWP_REQUEST_PKG (0x772)
    let (lo, _hi) = unsafe { rdmsr(0x772) };

    // Extract fields from low 32 bits
    let raw_min     = (lo & 0xFF) as u32;
    let raw_max     = ((lo >> 8) & 0xFF) as u32;
    let raw_desired = ((lo >> 16) & 0xFF) as u32;

    // Scale × 4, cap at 1000
    let pkg_min_perf  = cap1000(raw_min * 4);
    let pkg_max_perf  = cap1000(raw_max * 4);
    let pkg_desired   = cap1000(raw_desired * 4);
    let pkg_hwp_ema   = ema(state.pkg_hwp_ema, pkg_desired);

    state.pkg_min_perf = pkg_min_perf;
    state.pkg_max_perf = pkg_max_perf;
    state.pkg_desired  = pkg_desired;
    state.pkg_hwp_ema  = pkg_hwp_ema;

    crate::serial_println!(
        "[msr_ia32_hwp_request_pkg] age={} min={} max={} desired={} ema={}",
        age,
        pkg_min_perf,
        pkg_max_perf,
        pkg_desired,
        pkg_hwp_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_pkg_min_perf() -> u16 {
    STATE.lock().pkg_min_perf
}

pub fn get_pkg_max_perf() -> u16 {
    STATE.lock().pkg_max_perf
}

pub fn get_pkg_desired() -> u16 {
    STATE.lock().pkg_desired
}

pub fn get_pkg_hwp_ema() -> u16 {
    STATE.lock().pkg_hwp_ema
}
