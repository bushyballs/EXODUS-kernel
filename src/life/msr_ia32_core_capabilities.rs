#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_CORE_CAPABILITIES MSR address (Intel SDM vol. 4)
const MSR_IA32_CORE_CAPABILITIES: u32 = 0xCF;

// CPUID leaf 7, sub-leaf 0, ECX bit 30 — CORE_CAPABILITIES MSR supported
const CPUID_LEAF7_ECX_CORE_CAP_BIT: u32 = 30;

// Re-sample every 10000 ticks — static capability register, no need to poll often
const TICK_GATE: u32 = 10000;

// ── state ─────────────────────────────────────────────────────────────────────

struct State {
    core_stlb:    u16,   // bit 0 of lo — split TLB supported       (0 or 1000)
    core_fusa:    u16,   // bit 2 of lo — functional safety support  (0 or 1000)
    core_uc_lock: u16,   // bit 4 of lo — UC lock disable            (0 or 1000)
    core_cap_ema: u16,   // EMA of (stlb/3 + fusa/3 + uc_lock/3)
}

static MODULE: Mutex<State> = Mutex::new(State {
    core_stlb:    0,
    core_fusa:    0,
    core_uc_lock: 0,
    core_cap_ema: 0,
});

// ── CPUID guard ───────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 7 ECX bit 30 is set, indicating that
/// IA32_CORE_CAPABILITIES (0xCF) is a valid MSR on this logical processor.
fn has_core_capabilities() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> CPUID_LEAF7_ECX_CORE_CAP_BIT) & 1 == 1
}

// ── MSR read ─────────────────────────────────────────────────────────────────

fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_CORE_CAPABILITIES,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if has_core_capabilities() {
        serial_println!("[msr_ia32_core_capabilities] init OK (CORE_CAPABILITIES MSR present)");
    } else {
        serial_println!("[msr_ia32_core_capabilities] init — CORE_CAPABILITIES MSR not supported on this CPU");
    }
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_core_capabilities() {
        return;
    }

    let lo = read_msr();

    // bit 0: STLB_SUPPORTED — split TLB available
    let core_stlb: u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };

    // bit 2: FUSA_SUPPORTED — functional safety
    let core_fusa: u16 = if lo & (1 << 2) != 0 { 1000 } else { 0 };

    // bit 4: UC_LOCK_DISABLE — uncacheable lock disable
    let core_uc_lock: u16 = if lo & (1 << 4) != 0 { 1000 } else { 0 };

    // composite: evenly weight the three capability signals (each /3 => max ~333 each => max 999)
    let composite: u16 = (core_stlb / 3)
        .saturating_add(core_fusa / 3)
        .saturating_add(core_uc_lock / 3);

    let mut s = MODULE.lock();
    let new_ema = ema(s.core_cap_ema, composite);

    s.core_stlb    = core_stlb;
    s.core_fusa    = core_fusa;
    s.core_uc_lock = core_uc_lock;
    s.core_cap_ema = new_ema;

    serial_println!(
        "[msr_ia32_core_capabilities] age={} lo={:#010x} stlb={} fusa={} uc_lock={} ema={}",
        age, lo, core_stlb, core_fusa, core_uc_lock, new_ema
    );
}

// ── accessors ─────────────────────────────────────────────────────────────────

/// Split TLB supported (0 or 1000).
pub fn get_core_stlb() -> u16 {
    MODULE.lock().core_stlb
}

/// Functional safety supported (0 or 1000).
pub fn get_core_fusa() -> u16 {
    MODULE.lock().core_fusa
}

/// Uncacheable lock disable (0 or 1000).
pub fn get_core_uc_lock() -> u16 {
    MODULE.lock().core_uc_lock
}

/// EMA of the three capability signals.
pub fn get_core_cap_ema() -> u16 {
    MODULE.lock().core_cap_ema
}
