#![allow(dead_code)]

use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct ApicBaseState {
    apic_bsp:       u16,   // bit 8  — is this the BSP?         0 or 1000
    apic_x2mode:    u16,   // bit 10 — x2APIC mode enabled?     0 or 1000
    apic_enabled:   u16,   // bit 11 — APIC globally enabled?   0 or 1000
    apic_state_ema: u16,   // EMA of composite signal           0–1000
}

impl ApicBaseState {
    const fn new() -> Self {
        Self {
            apic_bsp:       0,
            apic_x2mode:    0,
            apic_enabled:   0,
            apic_state_ema: 0,
        }
    }
}

static STATE: Mutex<ApicBaseState> = Mutex::new(ApicBaseState::new());

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_APIC_BASE MSR (0x1B).
/// Returns (lo, hi) where lo = bits[31:0], hi = bits[63:32].
#[inline]
fn read_ia32_apic_base() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x1Bu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let (lo, _hi) = read_ia32_apic_base();

    let bsp     = if (lo >> 8)  & 1 != 0 { 1000u16 } else { 0u16 };
    let x2mode  = if (lo >> 10) & 1 != 0 { 1000u16 } else { 0u16 };
    let enabled = if (lo >> 11) & 1 != 0 { 1000u16 } else { 0u16 };

    // composite: bsp/4 + x2mode/4 + enabled/2
    let composite = (bsp as u32 / 4) + (x2mode as u32 / 4) + (enabled as u32 / 2);
    let ema = composite as u16;

    let mut s = STATE.lock();
    s.apic_bsp       = bsp;
    s.apic_x2mode    = x2mode;
    s.apic_enabled   = enabled;
    s.apic_state_ema = ema;

    crate::serial_println!(
        "[msr_ia32_apic_base] age=0 bsp={} x2apic={} enabled={} ema={}",
        s.apic_bsp, s.apic_x2mode, s.apic_enabled, s.apic_state_ema
    );
}

pub fn tick(age: u32) {
    if age % 8000 != 0 {
        return;
    }

    let (lo, _hi) = read_ia32_apic_base();

    let bsp     = if (lo >> 8)  & 1 != 0 { 1000u16 } else { 0u16 };
    let x2mode  = if (lo >> 10) & 1 != 0 { 1000u16 } else { 0u16 };
    let enabled = if (lo >> 11) & 1 != 0 { 1000u16 } else { 0u16 };

    // composite for EMA new_val: bsp/4 + x2mode/4 + enabled/2
    let composite = (bsp as u32 / 4) + (x2mode as u32 / 4) + (enabled as u32 / 2);

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32, cast to u16
    let new_ema = ((s.apic_state_ema as u32 * 7) + composite) / 8;

    s.apic_bsp       = bsp;
    s.apic_x2mode    = x2mode;
    s.apic_enabled   = enabled;
    s.apic_state_ema = new_ema as u16;

    crate::serial_println!(
        "[msr_ia32_apic_base] age={} bsp={} x2apic={} enabled={} ema={}",
        age, s.apic_bsp, s.apic_x2mode, s.apic_enabled, s.apic_state_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_apic_bsp() -> u16 {
    STATE.lock().apic_bsp
}

pub fn get_apic_x2mode() -> u16 {
    STATE.lock().apic_x2mode
}

pub fn get_apic_enabled() -> u16 {
    STATE.lock().apic_enabled
}

pub fn get_apic_state_ema() -> u16 {
    STATE.lock().apic_state_ema
}
