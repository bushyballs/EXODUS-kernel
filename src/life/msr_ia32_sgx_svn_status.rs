#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct SgxSvnState {
    sgx_svn_locked:  u16,
    sgx_svn_value:   u16,
    sgx_svn_nonzero: u16,
    sgx_svn_ema:     u16,
}

impl SgxSvnState {
    const fn new() -> Self {
        Self {
            sgx_svn_locked:  0,
            sgx_svn_value:   0,
            sgx_svn_nonzero: 0,
            sgx_svn_ema:     0,
        }
    }
}

static STATE: Mutex<SgxSvnState> = Mutex::new(SgxSvnState::new());

// ── CPUID Guard ───────────────────────────────────────────────────────────────

fn has_sgx() -> bool {
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
            options(nostack, nomem)
        );
    }
    (ebx_val >> 2) & 1 != 0
}

// ── MSR Read ──────────────────────────────────────────────────────────────────

/// Read MSR 0x500 (IA32_SGX_SVN_STATUS).
/// Returns the low 32 bits.
unsafe fn read_msr_0x500() -> u32 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x500u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    let _ = hi;
    lo
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.sgx_svn_locked  = 0;
    s.sgx_svn_value   = 0;
    s.sgx_svn_nonzero = 0;
    s.sgx_svn_ema     = 0;
    crate::serial_println!("[msr_ia32_sgx_svn_status] init: sgx={}", has_sgx());
}

pub fn tick(age: u32) {
    // Sample every 7000 ticks
    if age % 7000 != 0 {
        return;
    }

    // CPUID guard
    if !has_sgx() {
        return;
    }

    // Read MSR 0x500
    let raw: u32 = unsafe { read_msr_0x500() };

    // bit 0 — LOCK
    let locked_bit = raw & 1;
    let sgx_svn_locked: u16 = if locked_bit != 0 { 1000 } else { 0 };

    // bits [23:16] — SGX_SVN (0–255 field, meaningful range 0–63)
    let svn_raw = (raw >> 16) & 0xFF;
    // scale: svn × 15, capped at 1000
    let svn_scaled = (svn_raw * 15).min(1000) as u16;
    let sgx_svn_value: u16 = svn_scaled;

    // nonzero: 1000 if SVN != 0, else 0
    let sgx_svn_nonzero: u16 = if svn_raw != 0 { 1000 } else { 0 };

    // EMA composite: locked/4 + value/4 + nonzero/2
    let composite: u16 = (sgx_svn_locked / 4)
        .saturating_add(sgx_svn_value / 4)
        .saturating_add(sgx_svn_nonzero / 2);

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8, computed in u32
    let old_ema = s.sgx_svn_ema as u32;
    let new_ema = ((old_ema * 7) + composite as u32) / 8;
    let sgx_svn_ema = new_ema as u16;

    s.sgx_svn_locked  = sgx_svn_locked;
    s.sgx_svn_value   = sgx_svn_value;
    s.sgx_svn_nonzero = sgx_svn_nonzero;
    s.sgx_svn_ema     = sgx_svn_ema;

    crate::serial_println!(
        "[msr_ia32_sgx_svn_status] age={} locked={} svn={} nonzero={} ema={}",
        age,
        sgx_svn_locked,
        sgx_svn_value,
        sgx_svn_nonzero,
        sgx_svn_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_sgx_svn_locked() -> u16 {
    STATE.lock().sgx_svn_locked
}

pub fn get_sgx_svn_value() -> u16 {
    STATE.lock().sgx_svn_value
}

pub fn get_sgx_svn_nonzero() -> u16 {
    STATE.lock().sgx_svn_nonzero
}

pub fn get_sgx_svn_ema() -> u16 {
    STATE.lock().sgx_svn_ema
}
