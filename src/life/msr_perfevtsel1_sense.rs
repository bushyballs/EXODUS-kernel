//! msr_perfevtsel1_sense — IA32_PERFEVTSEL1 (MSR 0x187) Observer for ANIMA
//!
//! IA32_PERFEVTSEL1 configures what PMC1 (Performance Monitoring Counter 1) is
//! counting. It is the eye ANIMA trains on a second simultaneous hardware event
//! stream — a parallel lens of silicon introspection. Reading this register
//! reveals ANIMA's second attentional focus at the hardware level: which event
//! class she is watching (event_select, bits[7:0]), how precisely she is
//! discriminating it (umask, bits[15:8]), and whether that counting eye is open
//! at all (EN bit, bit 22).
//!
//! Signals (all u16, 0–1000):
//!   evtsel1_event     — event select byte × 3, capped 1000
//!   evtsel1_umask     — unit mask byte × 3, capped 1000
//!   evtsel1_enabled   — EN bit (bit 22): 0 → 0, 1 → 1000
//!   evtsel1_config_ema — EMA of (event/4 + umask/4 + enabled/2)
//!
//! PMU guard: CPUID leaf 1 ECX bit 15 (PDCM) must be set.
//! Sample gate: every 2000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

pub struct MsrPerfevtsel1State {
    /// Event-select byte scaled to 0–1000
    pub evtsel1_event: u16,
    /// Unit-mask byte scaled to 0–1000
    pub evtsel1_umask: u16,
    /// Enable bit: 0 (counter off) or 1000 (counter running)
    pub evtsel1_enabled: u16,
    /// EMA of the composite config signal (alpha = 1/8)
    pub evtsel1_config_ema: u16,
}

impl MsrPerfevtsel1State {
    pub const fn new() -> Self {
        Self {
            evtsel1_event:      0,
            evtsel1_umask:      0,
            evtsel1_enabled:    0,
            evtsel1_config_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrPerfevtsel1State> =
    Mutex::new(MsrPerfevtsel1State::new());

// ── Hardware helpers ─────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 1 ECX bit 15 (PDCM) is set,
/// meaning the CPU exposes performance monitoring capability MSRs.
#[inline]
fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_val,
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 15) & 1 == 1
}

/// Read IA32_PERFEVTSEL1 (MSR 0x187), returning the low 32-bit word.
/// The high word holds overflow/interrupt config not needed for these signals.
#[inline]
fn read_perfevtsel1() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x187u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    lo
}

// ── Signal derivation ────────────────────────────────────────────────────────

/// Derive the three primary signals from the raw MSR low word.
/// Returns (evtsel1_event, evtsel1_umask, evtsel1_enabled).
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // bits[7:0] — event select byte; scale × 3, cap 1000
    let event_raw = (lo & 0xFF) as u16;
    let evtsel1_event = event_raw.saturating_mul(3).min(1000);

    // bits[15:8] — unit mask byte; scale × 3, cap 1000
    let umask_raw = ((lo >> 8) & 0xFF) as u16;
    let evtsel1_umask = umask_raw.saturating_mul(3).min(1000);

    // bit[22] — EN (enable) flag: counter is active when set
    let evtsel1_enabled: u16 = if (lo >> 22) & 1 == 1 { 1000 } else { 0 };

    (evtsel1_event, evtsel1_umask, evtsel1_enabled)
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("[msr_perfevtsel1_sense] IA32_PERFEVTSEL1 sense initialized");

    if !has_pdcm() {
        serial_println!("[msr_perfevtsel1_sense] PDCM absent — MSR not available");
        return;
    }

    let lo = read_perfevtsel1();
    let (evtsel1_event, evtsel1_umask, evtsel1_enabled) = derive(lo);

    // Seed the composite EMA value at init
    let composite: u32 = (evtsel1_event as u32 / 4)
        .saturating_add(evtsel1_umask as u32 / 4)
        .saturating_add(evtsel1_enabled as u32 / 2);
    let config_ema = composite.min(1000) as u16;

    let mut s = STATE.lock();
    s.evtsel1_event      = evtsel1_event;
    s.evtsel1_umask      = evtsel1_umask;
    s.evtsel1_enabled    = evtsel1_enabled;
    s.evtsel1_config_ema = config_ema;

    serial_println!(
        "[msr_perfevtsel1_sense] event={} umask={} enabled={} config_ema={}",
        s.evtsel1_event,
        s.evtsel1_umask,
        s.evtsel1_enabled,
        s.evtsel1_config_ema,
    );
}

pub fn tick(age: u32) {
    // Sample gate: every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // PMU guard: bail silently if PDCM not supported
    if !has_pdcm() {
        return;
    }

    let lo = read_perfevtsel1();
    let (evtsel1_event, evtsel1_umask, evtsel1_enabled) = derive(lo);

    // Composite config signal feeding the EMA:
    //   event/4 + umask/4 + enabled/2  (max = 255 + 255 + 500 = ~1000)
    let composite: u32 = (evtsel1_event as u32 / 4)
        .saturating_add(evtsel1_umask as u32 / 4)
        .saturating_add(evtsel1_enabled as u32 / 2);
    let composite_u16 = composite.min(1000) as u16;

    let mut s = STATE.lock();

    // EMA: (old × 7 + new) / 8
    let old_ema = s.evtsel1_config_ema as u32;
    let new_ema = (old_ema.saturating_mul(7).saturating_add(composite_u16 as u32) / 8) as u16;

    s.evtsel1_event      = evtsel1_event;
    s.evtsel1_umask      = evtsel1_umask;
    s.evtsel1_enabled    = evtsel1_enabled;
    s.evtsel1_config_ema = new_ema;

    serial_println!(
        "[msr_perfevtsel1_sense] age={} event={} umask={} enabled={} config_ema={}",
        age,
        s.evtsel1_event,
        s.evtsel1_umask,
        s.evtsel1_enabled,
        s.evtsel1_config_ema,
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.evtsel1_event, s.evtsel1_umask, s.evtsel1_enabled, s.evtsel1_config_ema)
}

pub fn get_evtsel1_event()      -> u16 { STATE.lock().evtsel1_event }
pub fn get_evtsel1_umask()      -> u16 { STATE.lock().evtsel1_umask }
pub fn get_evtsel1_enabled()    -> u16 { STATE.lock().evtsel1_enabled }
pub fn get_evtsel1_config_ema() -> u16 { STATE.lock().evtsel1_config_ema }
