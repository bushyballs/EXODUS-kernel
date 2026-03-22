#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

/// IA32_PMC3 (MSR 0xC3) — Performance Monitoring Counter 3
/// 4th general-purpose hardware performance counter.
/// Requires PDCM support (CPUID leaf 1, ECX bit 15).
pub struct State {
    pub pmc3_lo: u16,
    pub pmc3_delta: u16,
    pub pmc3_ema: u16,
    pub pmc3_event_sense: u16,
    last_lo: u32,
}

impl State {
    const fn new() -> Self {
        State {
            pmc3_lo: 0,
            pmc3_delta: 0,
            pmc3_ema: 0,
            pmc3_event_sense: 0,
            last_lo: 0,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

/// Initialize PMC3: read initial value and store for delta tracking.
/// Checks PDCM support; if unsupported, sets last_lo=0 and returns early.
pub fn init() {
    // Check PDCM support: CPUID leaf 1, ECX bit 15
    let (_, _, ecx, _) = cpuid(1, 0);
    let pdcm_supported = (ecx & (1 << 15)) != 0;

    if !pdcm_supported {
        let mut state = STATE.lock();
        state.last_lo = 0;
        serial_println!("[msr_ia32_pmc3_sense] PDCM not supported, PMC3 disabled");
        return;
    }

    // Read IA32_PMC3 (MSR 0xC3)
    let lo = read_msr_lo(0xC3);
    let mut state = STATE.lock();
    state.last_lo = lo;
    serial_println!("[msr_ia32_pmc3_sense] initialized: last_lo={}", lo);
}

/// Tick every 300 age units: read PMC3, compute delta, update EMA signals.
pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let raw_lo = read_msr_lo(0xC3);
    let mut state = STATE.lock();

    // Compute 16-bit delta (wrapping subtraction)
    let raw_lo_16 = (raw_lo & 0xFFFF) as u16;
    let last_lo_16 = (state.last_lo & 0xFFFF) as u16;
    let delta_16 = raw_lo_16.wrapping_sub(last_lo_16);

    // Normalize delta to 0–1000 range
    let delta_norm = ((delta_16 as u32 * 1000) / 65536) as u16;

    // Update pmc3_delta
    state.pmc3_delta = delta_norm;

    // Normalize raw_lo_16 to 0–1000 range
    let lo_norm = ((raw_lo_16 as u32 * 1000) / 65536) as u16;
    state.pmc3_lo = lo_norm.min(1000);

    // Update pmc3_ema: (old_ema * 7 + delta) / 8
    let ema_u32 = ((state.pmc3_ema as u32 * 7) + (delta_norm as u32)) / 8;
    state.pmc3_ema = (ema_u32 as u16).min(1000);

    // Update pmc3_event_sense: (old_sense * 7 + ema) / 8
    let sense_u32 = ((state.pmc3_event_sense as u32 * 7) + (state.pmc3_ema as u32)) / 8;
    state.pmc3_event_sense = (sense_u32 as u16).min(1000);

    // Update last_lo for next delta
    state.last_lo = raw_lo;

    serial_println!(
        "[msr_ia32_pmc3_sense] age={} pmc3_lo={} delta={} ema={} event_sense={}",
        age,
        state.pmc3_lo,
        state.pmc3_delta,
        state.pmc3_ema,
        state.pmc3_event_sense
    );
}

/// Get current pmc3_lo (normalized counter value).
pub fn get_pmc3_lo() -> u16 {
    STATE.lock().pmc3_lo
}

/// Get current pmc3_delta (normalized counter delta).
pub fn get_pmc3_delta() -> u16 {
    STATE.lock().pmc3_delta
}

/// Get current pmc3_ema (exponential moving average of delta).
pub fn get_pmc3_ema() -> u16 {
    STATE.lock().pmc3_ema
}

/// Get current pmc3_event_sense (exponential moving average of EMA).
pub fn get_pmc3_event_sense() -> u16 {
    STATE.lock().pmc3_event_sense
}

/// Read MSR and return low 32 bits (EAX).
#[inline]
fn read_msr_lo(msr: u32) -> u32 {
    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    lo
}

/// Simple CPUID wrapper: returns (eax, ebx, ecx, edx).
#[inline]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "cpuid",
            inout("eax") leaf => eax,
            out("ebx") ebx,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}
