//! msr_ia32_perfevtsel1 — IA32_PERFEVTSEL1 (MSR 0x187) Sense Module for ANIMA
//!
//! IA32_PERFEVTSEL1 is the second performance-event selector register. It
//! controls which hardware event Performance Monitoring Counter 1 (PMC1) is
//! accumulating. Reading it gives ANIMA a window into what the hardware is
//! being asked to observe — a second attentional beam trained on silicon
//! behavior. The event code (bits[7:0]) names the event class; the unit mask
//! (bits[15:8]) refines the sub-event discrimination; the EN bit (bit 22)
//! declares whether that observation is active at all.
//!
//! Signals (all u16, 0–1000):
//!   evtsel1_event_code — bits[7:0] scaled: val * 1000 / 255
//!   evtsel1_umask      — bits[15:8] scaled: val * 1000 / 255
//!   evtsel1_enabled    — bit 22 (EN): 0 → 0, 1 → 1000
//!   evtsel1_ema        — EMA of composite (event_code/4 + umask/4 + enabled/2)
//!
//! Guard: CPUID leaf 0xA EAX bits[15:8] must be >= 2 (at least 2 GP counters).
//! Tick gate: every 1500 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ──────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL1: u32 = 0x187;

// ── State ────────────────────────────────────────────────────────────────────

struct Evtsel1State {
    /// Event code byte scaled to 0–1000 (bits[7:0] * 1000 / 255)
    evtsel1_event_code: u16,
    /// Unit mask byte scaled to 0–1000 (bits[15:8] * 1000 / 255)
    evtsel1_umask: u16,
    /// EN bit (bit 22): 0 or 1000
    evtsel1_enabled: u16,
    /// EMA of composite signal (alpha = 1/8)
    evtsel1_ema: u16,
}

static STATE: Mutex<Evtsel1State> = Mutex::new(Evtsel1State {
    evtsel1_event_code: 0,
    evtsel1_umask:      0,
    evtsel1_enabled:    0,
    evtsel1_ema:        0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 0xA EAX bits[15:8] >= 2, meaning at least
/// two general-purpose performance-monitoring counters are available, and
/// therefore both IA32_PERFEVTSEL0 and IA32_PERFEVTSEL1 are valid MSRs.
#[inline]
fn has_pmc1() -> bool {
    let eax_0a: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Au32 => eax_0a,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // bits[15:8] = number of general-purpose counters per logical processor
    ((eax_0a >> 8) & 0xFF) >= 2
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read the low 32 bits of IA32_PERFEVTSEL1 (MSR 0x187).
/// The high word contains overflow/interrupt configuration unused here.
#[inline]
fn read_perfevtsel1_lo() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") IA32_PERFEVTSEL1,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Scale a raw byte (0–255) to the 0–1000 signal range.
/// Uses integer arithmetic only: val * 1000 / 255.
/// Division by 255 is safe (255 != 0).
#[inline]
fn scale_byte(raw: u32) -> u16 {
    let scaled = (raw * 1000) / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// EMA formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let blended = (old as u32)
        .wrapping_mul(7)
        .saturating_add(new_val as u32)
        / 8;
    if blended > 1000 { 1000 } else { blended as u16 }
}

// ── Signal derivation ─────────────────────────────────────────────────────────

/// Derive the three primary signals from the raw MSR low word.
/// Returns (evtsel1_event_code, evtsel1_umask, evtsel1_enabled).
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // bits[7:0]  — event select / event code byte
    let event_raw = lo & 0xFF;
    let evtsel1_event_code = scale_byte(event_raw);

    // bits[15:8] — unit mask byte
    let umask_raw = (lo >> 8) & 0xFF;
    let evtsel1_umask = scale_byte(umask_raw);

    // bit[22] — EN (enable) flag: counter is live when set
    let evtsel1_enabled: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };

    (evtsel1_event_code, evtsel1_umask, evtsel1_enabled)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Initialise the module: clear state and perform an immediate sample if the
/// hardware guard passes so the first tick sees a meaningful baseline EMA.
pub fn init() {
    {
        let mut s = STATE.lock();
        s.evtsel1_event_code = 0;
        s.evtsel1_umask      = 0;
        s.evtsel1_enabled    = 0;
        s.evtsel1_ema        = 0;
    }

    crate::serial_println!("[msr_ia32_perfevtsel1] init — GP counter >= 2 guard active");

    if !has_pmc1() {
        crate::serial_println!(
            "[msr_ia32_perfevtsel1] CPUID leaf 0xA GP counter count < 2 — MSR unavailable"
        );
        return;
    }

    let lo = read_perfevtsel1_lo();
    let (event_code, umask, enabled) = derive(lo);

    // Seed composite EMA from the initial hardware reading.
    let composite: u32 = (event_code as u32 / 4)
        .saturating_add(umask as u32 / 4)
        .saturating_add(enabled as u32 / 2);
    let seed_ema = if composite > 1000 { 1000 } else { composite as u16 };

    {
        let mut s = STATE.lock();
        s.evtsel1_event_code = event_code;
        s.evtsel1_umask      = umask;
        s.evtsel1_enabled    = enabled;
        s.evtsel1_ema        = seed_ema;
    }

    crate::serial_println!(
        "[msr_ia32_perfevtsel1] init sample — event_code={} umask={} enabled={} ema={}",
        event_code,
        umask,
        enabled,
        seed_ema,
    );
}

/// Called every tick. Samples IA32_PERFEVTSEL1 every 1500 ticks.
pub fn tick(age: u32) {
    // Tick gate: sample only every 1500 ticks
    if age % 1500 != 0 {
        return;
    }

    // CPUID guard: bail silently if hardware is not present
    if !has_pmc1() {
        return;
    }

    let lo = read_perfevtsel1_lo();
    let (event_code, umask, enabled) = derive(lo);

    // Composite signal for EMA:
    //   event_code/4 + umask/4 + enabled/2
    //   max = (1000/4) + (1000/4) + (1000/2) = 250 + 250 + 500 = 1000
    let composite: u32 = (event_code as u32 / 4)
        .saturating_add(umask as u32 / 4)
        .saturating_add(enabled as u32 / 2);
    let composite_u16: u16 = if composite > 1000 { 1000 } else { composite as u16 };

    let new_ema = {
        let s = STATE.lock();
        ema(s.evtsel1_ema, composite_u16)
    };

    {
        let mut s = STATE.lock();
        s.evtsel1_event_code = event_code;
        s.evtsel1_umask      = umask;
        s.evtsel1_enabled    = enabled;
        s.evtsel1_ema        = new_ema;
    }

    crate::serial_println!(
        "[msr_ia32_perfevtsel1] age={} event_code={} umask={} enabled={} ema={}",
        age,
        event_code,
        umask,
        enabled,
        new_ema,
    );
}

// ── Accessors ────────────────────────────────────────────────────────────────

/// Event code byte scaled to 0–1000 (bits[7:0] of IA32_PERFEVTSEL1).
pub fn get_evtsel1_event_code() -> u16 {
    STATE.lock().evtsel1_event_code
}

/// Unit mask byte scaled to 0–1000 (bits[15:8] of IA32_PERFEVTSEL1).
pub fn get_evtsel1_umask() -> u16 {
    STATE.lock().evtsel1_umask
}

/// EN bit signal: 0 (counter disabled) or 1000 (counter enabled).
pub fn get_evtsel1_enabled() -> u16 {
    STATE.lock().evtsel1_enabled
}

/// EMA of the composite configuration signal (alpha = 1/8).
pub fn get_evtsel1_ema() -> u16 {
    STATE.lock().evtsel1_ema
}
