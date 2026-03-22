#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// IA32_QM_CTR MSR 0xC8E — QoS Monitoring Counter
// After writing RMID+EventID to IA32_QM_EVTSEL (0xC8F), this register
// returns the monitoring result.
// EDX bit 31 = Error, EDX bit 30 = Unavailable, low 62 bits = counter value.

const MSR_IA32_QM_CTR: u32 = 0xC8E;
const SAMPLE_INTERVAL: u32 = 1000;

struct MsrIa32QmCtrState {
    qm_ctr_lo:    u16,
    qm_ctr_delta: u16,
    qm_ctr_valid: u16,
    qm_ctr_ema:   u16,
    last_lo:      u32,
}

impl MsrIa32QmCtrState {
    const fn new() -> Self {
        Self {
            qm_ctr_lo:    0,
            qm_ctr_delta: 0,
            qm_ctr_valid: 0,
            qm_ctr_ema:   0,
            last_lo:      0,
        }
    }
}

static STATE: Mutex<MsrIa32QmCtrState> = Mutex::new(MsrIa32QmCtrState::new());

// CPUID guard: RDT monitoring — leaf 0x0F sub-leaf 0 EDX bit 1.
fn has_rdt() -> bool {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x0F {
        return false;
    }
    let edx_0f: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_0f,
            options(nostack, nomem)
        );
    }
    (edx_0f >> 1) & 1 != 0
}

// Read IA32_QM_CTR MSR. Returns (lo, hi) where lo = EAX, hi = EDX.
unsafe fn read_qm_ctr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_QM_CTR,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// Map a u32 raw value to 0–1000 by clamping to the low 16 bits,
// then scaling: (val * 1000) / 0xFFFF — all integer arithmetic.
fn map_lo16_to_signal(raw: u32) -> u16 {
    let clamped = raw & 0xFFFF;
    // clamped is at most 0xFFFF = 65535
    // (65535 * 1000) = 65_535_000 — fits in u32
    let scaled = (clamped * 1000) / 0xFFFF;
    scaled as u16
}

// Map a delta (u32) to 0–1000; treat any delta >= 0xFFFF as max.
fn map_delta_to_signal(delta: u32) -> u16 {
    if delta >= 0xFFFF {
        return 1000;
    }
    let scaled = (delta * 1000) / 0xFFFF;
    scaled as u16
}

pub fn init() {
    if !has_rdt() {
        crate::serial_println!(
            "[msr_ia32_qm_ctr] RDT monitoring not supported on this CPU — module idle"
        );
        return;
    }
    crate::serial_println!("[msr_ia32_qm_ctr] init — IA32_QM_CTR monitoring active");
}

pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    if !has_rdt() {
        return;
    }

    let (lo, hi) = unsafe { read_qm_ctr() };

    // valid = error bit (hi bit 31) clear AND unavailable bit (hi bit 30) clear
    let error_bit     = (hi >> 31) & 1;
    let unavail_bit   = (hi >> 30) & 1;
    let valid_bool    = error_bit == 0 && unavail_bit == 0;
    let valid_signal: u16 = if valid_bool { 1000 } else { 0 };

    let ctr_lo_signal = map_lo16_to_signal(lo);

    let mut state = STATE.lock();

    let delta_raw = if valid_bool {
        lo.wrapping_sub(state.last_lo)
    } else {
        0u32
    };
    let delta_signal = map_delta_to_signal(delta_raw);

    // EMA: (old * 7 + new_val) / 8 in u32, cast to u16
    let ema_new: u16 = {
        let old = state.qm_ctr_ema as u32;
        let new_val = delta_signal as u32;
        ((old * 7 + new_val) / 8) as u16
    };

    state.qm_ctr_lo    = ctr_lo_signal;
    state.qm_ctr_delta = delta_signal;
    state.qm_ctr_valid = valid_signal;
    state.qm_ctr_ema   = ema_new;
    state.last_lo      = lo;

    crate::serial_println!(
        "[msr_ia32_qm_ctr] age={} ctr={} delta={} valid={} ema={}",
        age,
        ctr_lo_signal,
        delta_signal,
        valid_signal,
        ema_new
    );
}

pub fn get_qm_ctr_lo() -> u16 {
    STATE.lock().qm_ctr_lo
}

pub fn get_qm_ctr_delta() -> u16 {
    STATE.lock().qm_ctr_delta
}

pub fn get_qm_ctr_valid() -> u16 {
    STATE.lock().qm_ctr_valid
}

pub fn get_qm_ctr_ema() -> u16 {
    STATE.lock().qm_ctr_ema
}
