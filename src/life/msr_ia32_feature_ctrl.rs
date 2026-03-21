#![allow(dead_code)]

use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct FeatureCtrlState {
    feature_lock: u16,
    vmx_enabled:  u16,
    smx_enabled:  u16,
    feature_ema:  u16,
}

impl FeatureCtrlState {
    const fn new() -> Self {
        Self {
            feature_lock: 0,
            vmx_enabled:  0,
            smx_enabled:  0,
            feature_ema:  0,
        }
    }
}

static STATE: Mutex<FeatureCtrlState> = Mutex::new(FeatureCtrlState::new());

// ── Hardware read ─────────────────────────────────────────────────────────────

/// Read IA32_FEATURE_CONTROL (MSR 0x3A).
/// RDMSR: ECX = MSR index → EDX:EAX = value.
/// Only the low 32 bits (EAX) matter for the bits we track.
#[inline]
fn read_ia32_feature_control() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x3Au32,
            out("eax") lo,
            out("edx") _hi,
            options(nomem, nostack),
        );
    }
    lo
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let lo = read_ia32_feature_control();

    // bit 0 = LOCK
    let lock: u16  = if lo & (1 << 0) != 0 { 1000 } else { 0 };
    // bit 2 = VMX outside SMX
    let vmx: u16   = if lo & (1 << 2) != 0 { 1000 } else { 0 };
    // bit 1 = VMX in SMX (SMX enabled)
    let smx: u16   = if lo & (1 << 1) != 0 { 1000 } else { 0 };

    // Initial EMA = first sample
    // composite = lock/4 + vmx/4 + smx/2   (all in [0..1000], result in [0..1000])
    let composite: u16 = (lock as u32 / 4 + vmx as u32 / 4 + smx as u32 / 2) as u16;

    let mut s = STATE.lock();
    s.feature_lock = lock;
    s.vmx_enabled  = vmx;
    s.smx_enabled  = smx;
    s.feature_ema  = composite;

    crate::serial_println!(
        "[msr_ia32_feature_ctrl] age=0 lock={} vmx={} smx={} ema={}",
        s.feature_lock, s.vmx_enabled, s.smx_enabled, s.feature_ema
    );
}

pub fn tick(age: u32) {
    // Sample every 5000 ticks — feature control rarely changes after boot.
    if age % 5000 != 0 {
        return;
    }

    let lo = read_ia32_feature_control();

    let lock: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };
    let vmx: u16  = if lo & (1 << 2) != 0 { 1000 } else { 0 };
    let smx: u16  = if lo & (1 << 1) != 0 { 1000 } else { 0 };

    // composite signal in [0..1000]
    let composite: u32 = lock as u32 / 4 + vmx as u32 / 4 + smx as u32 / 2;

    let mut s = STATE.lock();

    s.feature_lock = lock;
    s.vmx_enabled  = vmx;
    s.smx_enabled  = smx;

    // EMA: (old * 7 + new) / 8, computed in u32, cast to u16
    s.feature_ema = ((s.feature_ema as u32 * 7 + composite) / 8) as u16;

    crate::serial_println!(
        "[msr_ia32_feature_ctrl] age={} lock={} vmx={} smx={} ema={}",
        age, s.feature_lock, s.vmx_enabled, s.smx_enabled, s.feature_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_feature_lock() -> u16 {
    STATE.lock().feature_lock
}

pub fn get_vmx_enabled() -> u16 {
    STATE.lock().vmx_enabled
}

pub fn get_smx_enabled() -> u16 {
    STATE.lock().smx_enabled
}

pub fn get_feature_ema() -> u16 {
    STATE.lock().feature_ema
}
