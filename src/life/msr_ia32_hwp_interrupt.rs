#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ──────────────────────────────────────────────────────────────
const MSR_IA32_HWP_INTERRUPT: u32 = 0x773;

// ── Sampling period ───────────────────────────────────────────────────────────
const SAMPLE_EVERY: u32 = 7000;

// ── Module state ──────────────────────────────────────────────────────────────
struct HwpIntState {
    hwp_int_perf:      u16,
    hwp_int_excursion: u16,
    hwp_int_active:    u16,
    hwp_int_ema:       u16,
    initialized:       bool,
    supported:         bool,
}

impl HwpIntState {
    const fn new() -> Self {
        Self {
            hwp_int_perf:      0,
            hwp_int_excursion: 0,
            hwp_int_active:    0,
            hwp_int_ema:       0,
            initialized:       false,
            supported:         false,
        }
    }
}

static STATE: Mutex<HwpIntState> = Mutex::new(HwpIntState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────
// CPUID leaf 6, EAX bit 7 = HWP supported
//             EAX bit 3 = HWP_NOTIFICATION supported
fn has_hwp_notify() -> bool {
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
    ((eax_val >> 7) & 1 != 0) && ((eax_val >> 3) & 1 != 0)
}

// ── MSR read helper ───────────────────────────────────────────────────────────
// Returns the full 64-bit MSR value; caller extracts low 32 bits.
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

// ── EMA helper ────────────────────────────────────────────────────────────────
// EMA = (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Public: init ──────────────────────────────────────────────────────────────
pub fn init() {
    let mut s = STATE.lock();
    if s.initialized {
        return;
    }
    s.supported   = has_hwp_notify();
    s.initialized = true;
    crate::serial_println!(
        "[msr_ia32_hwp_interrupt] init: supported={}",
        s.supported
    );
}

// ── Public: tick ─────────────────────────────────────────────────────────────
pub fn tick(age: u32) {
    if age % SAMPLE_EVERY != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        return;
    }

    if !s.supported {
        // Leave all signals at 0; still log so the pipeline knows we ran.
        crate::serial_println!(
            "[msr_ia32_hwp_interrupt] age={} perf_int={} excursion={} active={} ema={} (unsupported)",
            age, 0u16, 0u16, 0u16, s.hwp_int_ema
        );
        return;
    }

    // Read IA32_HWP_INTERRUPT (0x773), use low 32 bits.
    let raw_lo: u32 = unsafe {
        (rdmsr(MSR_IA32_HWP_INTERRUPT) & 0xFFFF_FFFF) as u32
    };

    // bit 0 = EN_GUARANTEED_PERF_CHANGE
    let perf_int: u16 = if (raw_lo >> 0) & 1 != 0 { 1000 } else { 0 };
    // bit 1 = EN_EXCURSION_MINIMUM
    let excursion: u16 = if (raw_lo >> 1) & 1 != 0 { 1000 } else { 0 };

    // hwp_int_active: 1000 if any interrupt channel is enabled, else 0
    let active: u16 = if perf_int == 1000 || excursion == 1000 { 1000 } else { 0 };

    // EMA of active signal
    let new_ema: u16 = ema_u16(s.hwp_int_ema, active);

    s.hwp_int_perf      = perf_int;
    s.hwp_int_excursion = excursion;
    s.hwp_int_active    = active;
    s.hwp_int_ema       = new_ema;

    crate::serial_println!(
        "[msr_ia32_hwp_interrupt] age={} perf_int={} excursion={} active={} ema={}",
        age, perf_int, excursion, active, new_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────
pub fn get_hwp_int_perf() -> u16 {
    STATE.lock().hwp_int_perf
}

pub fn get_hwp_int_excursion() -> u16 {
    STATE.lock().hwp_int_excursion
}

pub fn get_hwp_int_active() -> u16 {
    STATE.lock().hwp_int_active
}

pub fn get_hwp_int_ema() -> u16 {
    STATE.lock().hwp_int_ema
}
