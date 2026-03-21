#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct TscAdjState {
    tsc_adj_nonzero: u16,
    tsc_adj_lo_sense: u16,
    tsc_adj_hi_sense: u16,
    tsc_adj_ema: u16,
    supported: bool,
    initialized: bool,
}

impl TscAdjState {
    const fn new() -> Self {
        Self {
            tsc_adj_nonzero: 0,
            tsc_adj_lo_sense: 0,
            tsc_adj_hi_sense: 0,
            tsc_adj_ema: 0,
            supported: false,
            initialized: false,
        }
    }
}

static STATE: Mutex<TscAdjState> = Mutex::new(TscAdjState::new());

// ── CPUID Guard ───────────────────────────────────────────────────────────────

fn has_tsc_adjust() -> bool {
    let ebx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            out("esi") ebx_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ebx_val >> 1) & 1 != 0
}

// ── RDMSR 0x3B ───────────────────────────────────────────────────────────────

/// Read IA32_TSC_ADJUST (MSR 0x3B).
/// Returns (lo32, hi32) — the full 64-bit value split at the 32-bit boundary.
unsafe fn rdmsr_tsc_adjust() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x3Bu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ── Fixed-point helpers ───────────────────────────────────────────────────────

/// Map a u16 value (full 16-bit range 0–65535) to 0–1000.
/// Uses integer arithmetic: result = (val as u32 * 1000) / 65535
#[inline(always)]
fn u16_to_sense(val: u16) -> u16 {
    ((val as u32 * 1000) / 65535) as u16
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.supported = has_tsc_adjust();
    s.initialized = true;
    crate::serial_println!(
        "[msr_ia32_tsc_adjust] init supported={}",
        s.supported
    );
}

pub fn tick(age: u32) {
    // Sample every 3000 ticks
    if age % 3000 != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        return;
    }

    if !s.supported {
        // TSC_ADJUST not supported; keep signals at zero
        crate::serial_println!(
            "[msr_ia32_tsc_adjust] age={} nonzero={} lo_sense={} hi_sense={} ema={}",
            age,
            s.tsc_adj_nonzero,
            s.tsc_adj_lo_sense,
            s.tsc_adj_hi_sense,
            s.tsc_adj_ema,
        );
        return;
    }

    // Read IA32_TSC_ADJUST MSR 0x3B — we use lo only (bits [31:0])
    let (lo, _hi) = unsafe { rdmsr_tsc_adjust() };

    // tsc_adj_nonzero: 1000 if lo != 0, 0 if zero
    let nonzero: u16 = if lo != 0 { 1000 } else { 0 };

    // tsc_adj_lo_sense: low 16 bits of lo mapped to 0–1000
    let lo_bits = (lo & 0x0000_FFFF) as u16;
    let lo_sense = u16_to_sense(lo_bits);

    // tsc_adj_hi_sense: bits [31:16] of lo mapped to 0–1000
    let hi_bits = ((lo >> 16) & 0x0000_FFFF) as u16;
    let hi_sense = u16_to_sense(hi_bits);

    // tsc_adj_ema: EMA of nonzero signal
    let ema_val = ema(s.tsc_adj_ema, nonzero);

    s.tsc_adj_nonzero = nonzero;
    s.tsc_adj_lo_sense = lo_sense;
    s.tsc_adj_hi_sense = hi_sense;
    s.tsc_adj_ema = ema_val;

    crate::serial_println!(
        "[msr_ia32_tsc_adjust] age={} nonzero={} lo_sense={} hi_sense={} ema={}",
        age,
        s.tsc_adj_nonzero,
        s.tsc_adj_lo_sense,
        s.tsc_adj_hi_sense,
        s.tsc_adj_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_tsc_adj_nonzero() -> u16 {
    STATE.lock().tsc_adj_nonzero
}

pub fn get_tsc_adj_lo_sense() -> u16 {
    STATE.lock().tsc_adj_lo_sense
}

pub fn get_tsc_adj_hi_sense() -> u16 {
    STATE.lock().tsc_adj_hi_sense
}

pub fn get_tsc_adj_ema() -> u16 {
    STATE.lock().tsc_adj_ema
}
