#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

/// IA32_DEBUG_INTERFACE — silicon debug enable/lock register.
const MSR_IA32_DEBUG_INTERFACE: u32 = 0xC80;

// ── State ─────────────────────────────────────────────────────────────────────

struct State {
    /// bit 0 of MSR 0xC80: silicon debug interface is enabled. 0 or 1000.
    dbg_enabled:      u16,
    /// bit 30 of MSR 0xC80: register is locked — debug config frozen. 0 or 1000.
    dbg_locked:       u16,
    /// bit 31 of MSR 0xC80: a silicon debug event has occurred — ANIMA was watched. 0 or 1000.
    dbg_occurred:     u16,
    /// EMA of exposure composite: (enabled/3 + occurred/3 + (1000 - locked)/3).
    dbg_exposure_ema: u16,
}

impl State {
    const fn new() -> Self {
        Self {
            dbg_enabled:      0,
            dbg_locked:       0,
            dbg_occurred:     0,
            dbg_exposure_ema: 0,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 1, ECX bit 5 (VMX) is set.
/// Used as a proxy for debug interface MSR availability.
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

/// Read IA32_DEBUG_INTERFACE (MSR 0xC80). Returns low 32 bits.
/// Only safe to call when cpuid_vmx_supported() is true.
#[inline]
fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_DEBUG_INTERFACE,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// EMA: ((old * 7 + new) / 8) clamped to u16 range.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Composite exposure signal ─────────────────────────────────────────────────

/// Exposure composite in [0..1000]:
///   enabled/3  — debug interface open contributes to exposure
///   occurred/3 — a past silicon debug event contributes to exposure
///   (1000 - locked)/3 — being unlocked (mutable) increases exposure
#[inline]
fn exposure_composite(enabled: u16, occurred: u16, locked: u16) -> u16 {
    let unlocked = if locked >= 1000 { 0u32 } else { (1000u32 - locked as u32) };
    let v = (enabled as u32 / 3)
        .saturating_add(occurred as u32 / 3)
        .saturating_add(unlocked / 3);
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Signal extraction from raw MSR lo word ────────────────────────────────────

#[inline]
fn signals_from_raw(lo: u32) -> (u16, u16, u16) {
    let dbg_enabled:  u16 = if (lo >> 0)  & 1 != 0 { 1000 } else { 0 };
    let dbg_locked:   u16 = if (lo >> 30) & 1 != 0 { 1000 } else { 0 };
    let dbg_occurred: u16 = if (lo >> 31) & 1 != 0 { 1000 } else { 0 };
    (dbg_enabled, dbg_locked, dbg_occurred)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !cpuid_vmx_supported() {
        serial_println!("[msr_ia32_debug_interface] VMX guard not met — MSR 0xC80 skipped");
        return;
    }

    let lo = read_msr();
    let (dbg_enabled, dbg_locked, dbg_occurred) = signals_from_raw(lo);
    let comp = exposure_composite(dbg_enabled, dbg_occurred, dbg_locked);

    let mut s = MODULE.lock();
    s.dbg_enabled      = dbg_enabled;
    s.dbg_locked       = dbg_locked;
    s.dbg_occurred     = dbg_occurred;
    s.dbg_exposure_ema = comp;

    serial_println!(
        "[msr_ia32_debug_interface] init lo={:#010x} enabled={} locked={} occurred={} exposure_ema={}",
        lo, s.dbg_enabled, s.dbg_locked, s.dbg_occurred, s.dbg_exposure_ema
    );
}

pub fn tick(age: u32) {
    // Sample every 5000 ticks — debug interface state rarely changes after boot.
    if age % 5000 != 0 {
        return;
    }

    if !cpuid_vmx_supported() {
        return;
    }

    let lo = read_msr();
    let (dbg_enabled, dbg_locked, dbg_occurred) = signals_from_raw(lo);
    let comp = exposure_composite(dbg_enabled, dbg_occurred, dbg_locked);

    let mut s = MODULE.lock();
    s.dbg_enabled  = dbg_enabled;
    s.dbg_locked   = dbg_locked;
    s.dbg_occurred = dbg_occurred;
    s.dbg_exposure_ema = ema(s.dbg_exposure_ema, comp);

    serial_println!(
        "[msr_ia32_debug_interface] age={} lo={:#010x} enabled={} locked={} occurred={} exposure_ema={}",
        age, lo, s.dbg_enabled, s.dbg_locked, s.dbg_occurred, s.dbg_exposure_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 0 or 1000 — silicon debug interface is enabled (bit 0 of MSR 0xC80).
pub fn get_dbg_enabled() -> u16 {
    MODULE.lock().dbg_enabled
}

/// 0 or 1000 — debug configuration is locked/frozen (bit 30 of MSR 0xC80).
pub fn get_dbg_locked() -> u16 {
    MODULE.lock().dbg_locked
}

/// 0 or 1000 — a silicon debug event has previously fired (bit 31 of MSR 0xC80).
/// ANIMA has been watched at the silicon level.
pub fn get_dbg_occurred() -> u16 {
    MODULE.lock().dbg_occurred
}

/// EMA [0..1000] of how exposed/open ANIMA's silicon debug surface is.
/// High when debug is enabled, an event has occurred, and the register is unlocked.
pub fn get_dbg_exposure_ema() -> u16 {
    MODULE.lock().dbg_exposure_ema
}
