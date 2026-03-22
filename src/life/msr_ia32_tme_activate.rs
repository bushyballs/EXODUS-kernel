#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_TME_ACTIVATE MSR 0x982 — Total Memory Encryption activation control.
// lo bit 0 = LOCK          — TME config is frozen
// lo bit 1 = TME_ENABLE    — encryption globally active
// lo bit 2 = KEY_SELECT    — hardware-generated key active
// lo bit 31 = TME_BYPASS_ENABLE — bypass for some regions (not tracked here)
// lo bits[7:4] = algorithm select (not tracked here)
// Guard: CPUID leaf 7, sub-leaf 0, ECX bit 13 (TME supported)

const MSR_IA32_TME_ACTIVATE: u32 = 0x982;
const TICK_GATE: u32 = 6000;

struct State {
    tme_locked:         u16,  // bit 0: config frozen
    tme_enabled:        u16,  // bit 1: encryption active
    tme_hw_key:         u16,  // bit 2: hardware key generation active
    tme_activation_ema: u16,  // EMA(locked/4 + enabled/4 + hw_key/2)
}

static MODULE: Mutex<State> = Mutex::new(State {
    tme_locked:         0,
    tme_enabled:        0,
    tme_hw_key:         0,
    tme_activation_ema: 0,
});

/// Returns true when CPUID leaf 7, sub-leaf 0, ECX bit 13 is set (TME supported).
fn has_tme() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => ecx,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx >> 13) & 1 == 1
}

/// Read the raw lo dword of IA32_TME_ACTIVATE (0x982).
fn read_msr() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_TME_ACTIVATE,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

pub fn init() {
    serial_println!("[msr_ia32_tme_activate] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_tme() {
        serial_println!("[msr_ia32_tme_activate] age={} TME not supported — skipping", age);
        return;
    }

    let lo = read_msr();

    // Decode bits — each signal is 0 or 1000 (u16, 0–1000 range).
    let tme_locked:  u16 = if lo & (1 << 0) != 0 { 1000 } else { 0 };
    let tme_enabled: u16 = if lo & (1 << 1) != 0 { 1000 } else { 0 };
    let tme_hw_key:  u16 = if lo & (1 << 2) != 0 { 1000 } else { 0 };

    // Composite for EMA: locked/4 + enabled/4 + hw_key/2 — max = 250+250+500 = 1000.
    let composite: u16 = (tme_locked / 4)
        .saturating_add(tme_enabled / 4)
        .saturating_add(tme_hw_key / 2);

    let mut s = MODULE.lock();

    // EMA: ((old * 7) + new) / 8, capped at 1000.
    let ema: u16 = ((s.tme_activation_ema as u32)
        .wrapping_mul(7)
        .saturating_add(composite as u32)
        / 8)
        .min(1000) as u16;

    s.tme_locked         = tme_locked;
    s.tme_enabled        = tme_enabled;
    s.tme_hw_key         = tme_hw_key;
    s.tme_activation_ema = ema;

    serial_println!(
        "[msr_ia32_tme_activate] age={} lo={:#010x} locked={} enabled={} hw_key={} act_ema={}",
        age, lo, tme_locked, tme_enabled, tme_hw_key, ema
    );
}

/// 1000 if IA32_TME_ACTIVATE LOCK bit is set (config frozen), else 0.
pub fn get_tme_locked() -> u16 {
    MODULE.lock().tme_locked
}

/// 1000 if TME_ENABLE bit is set (all memory encrypted), else 0.
pub fn get_tme_enabled() -> u16 {
    MODULE.lock().tme_enabled
}

/// 1000 if KEY_SELECT bit is set (hardware key generation active), else 0.
pub fn get_tme_hw_key() -> u16 {
    MODULE.lock().tme_hw_key
}

/// EMA of (locked/4 + enabled/4 + hw_key/2), range 0–1000.
pub fn get_tme_activation_ema() -> u16 {
    MODULE.lock().tme_activation_ema
}
