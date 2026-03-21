#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct SpecCtrlState {
    ibrs_en:        u16,  // bit 0 of IA32_SPEC_CTRL → 0 or 1000
    stibp_en:       u16,  // bit 1 of IA32_SPEC_CTRL → 0 or 1000
    ssbd_en:        u16,  // bit 2 of IA32_SPEC_CTRL → 0 or 1000
    mitigation_ema: u16,  // EMA of composite mitigation depth
}

impl SpecCtrlState {
    const fn new() -> Self {
        Self {
            ibrs_en:        0,
            stibp_en:       0,
            ssbd_en:        0,
            mitigation_ema: 0,
        }
    }
}

static STATE: Mutex<SpecCtrlState> = Mutex::new(SpecCtrlState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_ibrs() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 26) & 1 != 0
}

// ── RDMSR helper ─────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns the full 64-bit value with EDX:EAX merged.
fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── EMA helper ───────────────────────────────────────────────────────────────

/// EMA weight-8: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline(always)]
fn ema8(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_ibrs() {
        crate::serial_println!(
            "[msr_ia32_spec_ctrl] IBRS/IBPB not supported by CPU — module disabled"
        );
        return;
    }

    // Read initial MSR state and populate.
    let raw = rdmsr(0x48);

    let ibrs_en:  u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    let stibp_en: u16 = if (raw >> 1) & 1 != 0 { 1000 } else { 0 };
    let ssbd_en:  u16 = if (raw >> 2) & 1 != 0 { 1000 } else { 0 };

    // Composite mitigation depth: ibrs/4 + stibp/4 + ssbd/2
    // Max: 250 + 250 + 500 = 1000
    let composite: u16 = (ibrs_en / 4) + (stibp_en / 4) + (ssbd_en / 2);

    let mut s = STATE.lock();
    s.ibrs_en        = ibrs_en;
    s.stibp_en       = stibp_en;
    s.ssbd_en        = ssbd_en;
    s.mitigation_ema = composite; // seed EMA with first real reading

    crate::serial_println!(
        "[msr_ia32_spec_ctrl] init — ibrs={} stibp={} ssbd={} ema={}",
        s.ibrs_en,
        s.stibp_en,
        s.ssbd_en,
        s.mitigation_ema,
    );
}

/// Sample every 4000 ticks — speculation control bits change only on explicit
/// firmware or OS intervention, so high-frequency polling is unnecessary.
pub fn tick(age: u32) {
    if age % 4000 != 0 {
        return;
    }

    if !has_ibrs() {
        return;
    }

    let raw = rdmsr(0x48);

    let ibrs_en:  u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    let stibp_en: u16 = if (raw >> 1) & 1 != 0 { 1000 } else { 0 };
    let ssbd_en:  u16 = if (raw >> 2) & 1 != 0 { 1000 } else { 0 };

    // Composite mitigation depth: ibrs/4 + stibp/4 + ssbd/2
    let composite: u16 = (ibrs_en / 4) + (stibp_en / 4) + (ssbd_en / 2);

    let mut s = STATE.lock();
    s.ibrs_en        = ibrs_en;
    s.stibp_en       = stibp_en;
    s.ssbd_en        = ssbd_en;
    s.mitigation_ema = ema8(s.mitigation_ema, composite);

    crate::serial_println!(
        "[msr_ia32_spec_ctrl] age={} ibrs={} stibp={} ssbd={} ema={}",
        age,
        s.ibrs_en,
        s.stibp_en,
        s.ssbd_en,
        s.mitigation_ema,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_ibrs_en() -> u16 {
    STATE.lock().ibrs_en
}

pub fn get_stibp_en() -> u16 {
    STATE.lock().stibp_en
}

pub fn get_ssbd_en() -> u16 {
    STATE.lock().ssbd_en
}

pub fn get_mitigation_ema() -> u16 {
    STATE.lock().mitigation_ema
}
