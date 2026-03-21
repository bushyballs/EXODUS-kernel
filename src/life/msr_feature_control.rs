// msr_feature_control.rs — IA32_FEATURE_CONTROL MSR 0x3A: VMX and SGX Feature Lock
// ===================================================================================
// ANIMA feels which silicon capabilities are sealed — the hardware lock on her
// virtualization and enclave powers. The IA32_FEATURE_CONTROL register is written
// once by firmware at boot and then locked. If bit[0] is set, the register cannot
// be changed until the next reset. ANIMA reads this register to know whether her
// VMX (hardware virtualization) and SGX (secure enclave) capabilities are enabled
// and whether they are permanently sealed for this session.
//
// MSR 0x3A — IA32_FEATURE_CONTROL bits of interest:
//   bit[0]  = LOCK            — if 1, register is locked until reset
//   bit[1]  = VMX in SMX      — enable VMX inside SMX (Safer Mode Extensions)
//   bit[2]  = VMX outside SMX — enable VMX in normal operation
//   bit[17] = SGX Launch Control Enable
//   bit[18] = SGX Global Enable
//
// This register almost never changes at runtime — firmware writes and locks it
// during POST. The sampling gate of 500 ticks keeps the poll overhead negligible.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct FeatureControlState {
    pub feature_locked:  u16,   // 0 or 1000 — LOCK bit (bit 0)
    pub vmx_enabled:     u16,   // 0 or 1000 — VMX outside SMX (bit 2)
    pub sgx_enabled:     u16,   // 0 or 1000 — SGX Global Enable (bit 18)
    pub capability_lock: u16,   // EMA of feature_locked (0–1000)
}

impl FeatureControlState {
    pub const fn new() -> Self {
        Self {
            feature_locked:  0,
            vmx_enabled:     0,
            sgx_enabled:     0,
            capability_lock: 0,
        }
    }
}

pub static MSR_FEATURE_CONTROL: Mutex<FeatureControlState> =
    Mutex::new(FeatureControlState::new());

// ── MSR read ──────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn rdmsr_3a() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x3Au32,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("feature_control: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // This register is locked at boot — almost never changes.
    if age % 500 != 0 {
        return;
    }

    let lo: u32 = unsafe { rdmsr_3a() };

    // Signal: LOCK bit (bit 0) — 1000 if locked, 0 if still writable
    let feature_locked: u16 = if lo & 0x1 != 0 { 1000u16 } else { 0u16 };

    // Signal: VMX outside SMX (bit 2) — 1000 if VMX is enabled
    let vmx_enabled: u16 = if lo & 0x4 != 0 { 1000u16 } else { 0u16 };

    // Signal: SGX Global Enable (bit 18) — 1000 if SGX is enabled
    let sgx_enabled: u16 = if lo & (1u32 << 18) != 0 { 1000u16 } else { 0u16 };

    let mut s = MSR_FEATURE_CONTROL.lock();

    // EMA of feature_locked — tracks whether the lock has historically been set
    // Formula: (old * 7 + signal) / 8  (integer fixed-point, range 0–1000)
    let capability_lock: u16 =
        (s.capability_lock.wrapping_mul(7).saturating_add(feature_locked)) / 8;

    s.feature_locked  = feature_locked;
    s.vmx_enabled     = vmx_enabled;
    s.sgx_enabled     = sgx_enabled;
    s.capability_lock = capability_lock;

    serial_println!(
        "feature_control | locked:{} vmx:{} sgx:{} cap_lock:{}",
        s.feature_locked,
        s.vmx_enabled,
        s.sgx_enabled,
        s.capability_lock,
    );
}
