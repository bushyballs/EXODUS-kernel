#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct HwpStatusState {
    hwp_perf_changed: u16,
    hwp_excursion:    u16,
    hwp_stress:       u16,
    hwp_status_ema:   u16,
}

impl HwpStatusState {
    const fn new() -> Self {
        Self {
            hwp_perf_changed: 0,
            hwp_excursion:    0,
            hwp_stress:       0,
            hwp_status_ema:   0,
        }
    }
}

static STATE: Mutex<HwpStatusState> = Mutex::new(HwpStatusState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_hwp() -> bool {
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
    (eax_val >> 7) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_HWP_STATUS (MSR 0x777).
/// Returns the low 32 bits; bits [63:32] are reserved.
unsafe fn read_hwp_status() -> u32 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x777u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    let _ = hi; // bits [63:32] reserved — discard
    lo
}

// ── Public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.hwp_perf_changed = 0;
    s.hwp_excursion    = 0;
    s.hwp_stress       = 0;
    s.hwp_status_ema   = 0;
    crate::serial_println!("[msr_ia32_hwp_status] init: hwp_supported={}", has_hwp());
}

pub fn tick(age: u32) {
    // Sample every 400 ticks to catch excursions without thrashing.
    if age % 400 != 0 {
        return;
    }

    // Require HWP support; skip silently on unsupported hardware.
    if !has_hwp() {
        return;
    }

    let raw = unsafe { read_hwp_status() };

    // bit 0 = GUARANTEED_PERF_CHANGE
    let perf_changed: u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    // bit 2 = EXCURSION_TO_MINIMUM
    let excursion: u16    = if (raw >> 2) & 1 != 0 { 1000 } else { 0 };

    // hwp_stress = perf_changed/4 + excursion*3/4  (excursion weighted heavier)
    // All arithmetic in u32 to avoid overflow before clamping.
    let stress_u32: u32 = (perf_changed as u32) / 4
        + (excursion as u32) * 3 / 4;
    // Clamp to 0–1000.
    let stress: u16 = if stress_u32 > 1000 { 1000 } else { stress_u32 as u16 };

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32, cast to u16.
    let ema_u32: u32 = ((s.hwp_status_ema as u32) * 7 + stress as u32) / 8;
    let ema: u16 = if ema_u32 > 1000 { 1000 } else { ema_u32 as u16 };

    s.hwp_perf_changed = perf_changed;
    s.hwp_excursion    = excursion;
    s.hwp_stress       = stress;
    s.hwp_status_ema   = ema;

    crate::serial_println!(
        "[msr_ia32_hwp_status] age={} perf_chg={} excursion={} stress={} ema={}",
        age, perf_changed, excursion, stress, ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_hwp_perf_changed() -> u16 {
    STATE.lock().hwp_perf_changed
}

pub fn get_hwp_excursion() -> u16 {
    STATE.lock().hwp_excursion
}

pub fn get_hwp_stress() -> u16 {
    STATE.lock().hwp_stress
}

pub fn get_hwp_status_ema() -> u16 {
    STATE.lock().hwp_status_ema
}
