#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// bit 0 of MSR 0x3A: MSR is locked (writes blocked). 0 or 1000.
    locked:           u16,
    /// bit 2 of MSR 0x3A: VMX enabled outside SMX. 0 or 1000.
    vmx_enabled:      u16,
    /// bit 18 of MSR 0x3A: SGX globally enabled. 0 or 1000.
    sgx_global:       u16,
    /// EMA of composite signal: locked/4 + vmx_enabled/4 + sgx_global/2
    feature_lock_ema: u16,
}

impl State {
    const fn new() -> Self {
        Self {
            locked:           0,
            vmx_enabled:      0,
            sgx_global:       0,
            feature_lock_ema: 0,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 1, ECX bit 5 (VMX) is set.
/// If VMX is supported, IA32_FEATURE_CONTROL (MSR 0x3A) is readable.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, ecx",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_val,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 5) & 1 == 1
}

// ── Hardware read ─────────────────────────────────────────────────────────────

/// Read IA32_FEATURE_CONTROL (MSR 0x3A). Returns low 32 bits.
/// Only safe to call when cpuid_vmx_supported() is true.
#[inline]
fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3Au32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// EMA: ((old * 7 + new) / 8) clamped to u16.
#[inline]
fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Composite signal ──────────────────────────────────────────────────────────

/// Composite signal in [0..1000]: locked/4 + vmx_enabled/4 + sgx_global/2
#[inline]
fn composite(locked: u16, vmx_enabled: u16, sgx_global: u16) -> u16 {
    let v = (locked as u32 / 4)
        .saturating_add(vmx_enabled as u32 / 4)
        .saturating_add(sgx_global as u32 / 2);
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !cpuid_vmx_supported() {
        serial_println!("[msr_ia32_feature_control] VMX not supported — MSR 0x3A skipped");
        return;
    }

    let raw = read_msr();

    // bit 0 = Lock
    let locked:      u16 = if raw & (1 << 0)  != 0 { 1000 } else { 0 };
    // bit 2 = VMX outside SMX
    let vmx_enabled: u16 = if raw & (1 << 2)  != 0 { 1000 } else { 0 };
    // bit 18 = SGX_GLOBAL_EN
    let sgx_global:  u16 = if raw & (1 << 18) != 0 { 1000 } else { 0 };

    let comp = composite(locked, vmx_enabled, sgx_global);

    let mut s = MODULE.lock();
    s.locked           = locked;
    s.vmx_enabled      = vmx_enabled;
    s.sgx_global       = sgx_global;
    s.feature_lock_ema = comp;

    serial_println!(
        "[msr_ia32_feature_control] init locked={} vmx={} sgx={} ema={}",
        s.locked, s.vmx_enabled, s.sgx_global, s.feature_lock_ema
    );
}

pub fn tick(age: u32) {
    // Sample every 8000 ticks — MSR 0x3A is static after boot.
    if age % 8000 != 0 {
        return;
    }

    if !cpuid_vmx_supported() {
        return;
    }

    let raw = read_msr();

    let locked:      u16 = if raw & (1 << 0)  != 0 { 1000 } else { 0 };
    let vmx_enabled: u16 = if raw & (1 << 2)  != 0 { 1000 } else { 0 };
    let sgx_global:  u16 = if raw & (1 << 18) != 0 { 1000 } else { 0 };
    // bit 20 = LMCE_ON (Local Machine Check Exception enabled) — read for telemetry
    let lmce_on:     u16 = if raw & (1 << 20) != 0 { 1000 } else { 0 };

    let comp = composite(locked, vmx_enabled, sgx_global);

    let mut s = MODULE.lock();
    s.locked      = locked;
    s.vmx_enabled = vmx_enabled;
    s.sgx_global  = sgx_global;
    s.feature_lock_ema = ema(s.feature_lock_ema, comp);

    serial_println!(
        "[msr_ia32_feature_control] age={} locked={} vmx={} sgx={} lmce={} ema={}",
        age, s.locked, s.vmx_enabled, s.sgx_global, lmce_on, s.feature_lock_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_locked() -> u16 {
    MODULE.lock().locked
}

pub fn get_vmx_enabled() -> u16 {
    MODULE.lock().vmx_enabled
}

pub fn get_sgx_global() -> u16 {
    MODULE.lock().sgx_global
}

pub fn get_feature_lock_ema() -> u16 {
    MODULE.lock().feature_lock_ema
}
