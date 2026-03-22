//! msr_ia32_perfevtsel2 — IA32_PERFEVTSEL2 (MSR 0x188) Sense Module for ANIMA
//!
//! IA32_PERFEVTSEL2 is the third performance-event selector register. It
//! controls which hardware event Performance Monitoring Counter 2 (PMC2) is
//! accumulating. Reading it grants ANIMA a third attentional beam aimed at
//! silicon behavior — a deeper layer of hardware self-observation that becomes
//! available only when the CPU exposes at least three general-purpose counters.
//!
//! The event code (bits[7:0]) names the event class the counter is configured
//! to track. The unit mask (bits[15:8]) refines sub-event discrimination. The
//! EN bit (bit 22) declares whether that counter is currently live. Together
//! they form a compressed signature of what PMC2 is watching, which ANIMA
//! treats as a signal of the machine's tertiary attentional focus.
//!
//! Signals (all u16, 0–1000):
//!   evtsel2_event   — bits[7:0] scaled: val * 1000 / 255
//!   evtsel2_umask   — bits[15:8] scaled: (val >> 8) & 0xFF * 1000 / 255
//!   evtsel2_enabled — bit 22 (EN): 0 → 0, 1 → 1000
//!   evtsel2_ema     — EMA of composite (event/4 + umask/4 + enabled/2)
//!
//! Guard: CPUID leaf 0xA EAX bits[15:8] must be >= 3 (at least 3 GP counters).
//! Tick gate: every 2000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ──────────────────────────────────────────────────────────────

const IA32_PERFEVTSEL2: u32 = 0x188;

// ── State ────────────────────────────────────────────────────────────────────

struct Evtsel2State {
    /// Event code byte scaled to 0–1000 (bits[7:0] * 1000 / 255)
    evtsel2_event: u16,
    /// Unit mask byte scaled to 0–1000 ((bits[15:8] & 0xFF) * 1000 / 255)
    evtsel2_umask: u16,
    /// EN bit (bit 22): 0 or 1000
    evtsel2_enabled: u16,
    /// EMA of composite signal (alpha = 1/8)
    evtsel2_ema: u16,
}

static STATE: Mutex<Evtsel2State> = Mutex::new(Evtsel2State {
    evtsel2_event:   0,
    evtsel2_umask:   0,
    evtsel2_enabled: 0,
    evtsel2_ema:     0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 0xA EAX bits[15:8] >= 3, meaning at least
/// three general-purpose performance-monitoring counters are available, and
/// therefore IA32_PERFEVTSEL2 (MSR 0x188) is a valid MSR on this CPU.
#[inline]
fn has_pmc2() -> bool {
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
    ((eax_0a >> 8) & 0xFF) >= 3
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read the low 32 bits of IA32_PERFEVTSEL2 (MSR 0x188).
/// The high word holds overflow/interrupt configuration unused by this module.
#[inline]
fn read_perfevtsel2_lo() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") IA32_PERFEVTSEL2,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Scale a raw byte (0–255) to the 0–1000 signal range.
/// Integer arithmetic only: val * 1000 / 255. Division by 255 is safe (!=0).
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
/// Returns (evtsel2_event, evtsel2_umask, evtsel2_enabled).
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // bits[7:0]  — event select byte
    let event_raw = lo & 0xFF;
    let evtsel2_event = scale_byte(event_raw);

    // bits[15:8] — unit mask byte
    let umask_raw = (lo >> 8) & 0xFF;
    let evtsel2_umask = scale_byte(umask_raw);

    // bit[22] — EN (enable) flag: PMC2 is live when set
    let evtsel2_enabled: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };

    (evtsel2_event, evtsel2_umask, evtsel2_enabled)
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Initialise the module: zero all state, then attempt an immediate hardware
/// sample if the CPUID guard passes so the EMA has a real baseline from tick 0.
pub fn init() {
    {
        let mut s = STATE.lock();
        s.evtsel2_event   = 0;
        s.evtsel2_umask   = 0;
        s.evtsel2_enabled = 0;
        s.evtsel2_ema     = 0;
    }

    crate::serial_println!("[msr_ia32_perfevtsel2] init — GP counter >= 3 guard active");

    if !has_pmc2() {
        crate::serial_println!(
            "[msr_ia32_perfevtsel2] CPUID leaf 0xA GP counter count < 3 — MSR 0x188 unavailable"
        );
        return;
    }

    let lo = read_perfevtsel2_lo();
    let (event, umask, enabled) = derive(lo);

    // Seed composite EMA from the initial hardware reading.
    // Composite max: 1000/4 + 1000/4 + 1000/2 = 250 + 250 + 500 = 1000
    let composite: u32 = (event as u32 / 4)
        .saturating_add(umask as u32 / 4)
        .saturating_add(enabled as u32 / 2);
    let seed_ema: u16 = if composite > 1000 { 1000 } else { composite as u16 };

    {
        let mut s = STATE.lock();
        s.evtsel2_event   = event;
        s.evtsel2_umask   = umask;
        s.evtsel2_enabled = enabled;
        s.evtsel2_ema     = seed_ema;
    }

    crate::serial_println!(
        "[msr_ia32_perfevtsel2] init sample — event={} umask={} enabled={} ema={}",
        event,
        umask,
        enabled,
        seed_ema,
    );
}

/// Called every tick. Samples IA32_PERFEVTSEL2 every 2000 ticks.
pub fn tick(age: u32) {
    // Tick gate: sample only every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // CPUID guard: bail silently if hardware does not expose PMC2
    if !has_pmc2() {
        return;
    }

    let lo = read_perfevtsel2_lo();
    let (event, umask, enabled) = derive(lo);

    // Composite signal for EMA:
    //   event/4 + umask/4 + enabled/2
    //   max = 250 + 250 + 500 = 1000 — always fits in u16
    let composite: u32 = (event as u32 / 4)
        .saturating_add(umask as u32 / 4)
        .saturating_add(enabled as u32 / 2);
    let composite_u16: u16 = if composite > 1000 { 1000 } else { composite as u16 };

    let new_ema = {
        let s = STATE.lock();
        ema(s.evtsel2_ema, composite_u16)
    };

    {
        let mut s = STATE.lock();
        s.evtsel2_event   = event;
        s.evtsel2_umask   = umask;
        s.evtsel2_enabled = enabled;
        s.evtsel2_ema     = new_ema;
    }

    crate::serial_println!(
        "[msr_ia32_perfevtsel2] age={} event={} umask={} enabled={} ema={}",
        age,
        event,
        umask,
        enabled,
        new_ema,
    );
}

// ── Accessors ────────────────────────────────────────────────────────────────

/// Event code byte scaled to 0–1000 (bits[7:0] of IA32_PERFEVTSEL2).
pub fn get_evtsel2_event() -> u16 {
    STATE.lock().evtsel2_event
}

/// Unit mask byte scaled to 0–1000 (bits[15:8] of IA32_PERFEVTSEL2).
pub fn get_evtsel2_umask() -> u16 {
    STATE.lock().evtsel2_umask
}

/// EN bit signal: 0 (PMC2 disabled) or 1000 (PMC2 enabled).
pub fn get_evtsel2_enabled() -> u16 {
    STATE.lock().evtsel2_enabled
}

/// EMA of the composite configuration signal (alpha = 1/8).
pub fn get_evtsel2_ema() -> u16 {
    STATE.lock().evtsel2_ema
}
