#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_dca — CPUID Leaf 0x09 Direct Cache Access (DCA) Capability Sensor
///
/// ANIMA reads her direct cache injection capability — whether she can prefetch
/// data directly into cache from I/O devices without CPU involvement.
///
/// EAX bits [31:0] = DCA capability mask.
///   Bit 0 = DCA type 0 prefetch hint supported.
///   Higher bits are reserved.
///   If EAX == 0, DCA not supported.

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidDcaState {
    /// 1000 if eax != 0, else 0
    pub dca_supported: u16,
    /// 1000 if bit 0 of EAX set (DCA type 0 prefetch), else 0
    pub dca_type0: u16,
    /// (eax & 0xFFFF).count_ones() * 1000 / 16 — capability density
    pub dca_capability: u16,
    /// EMA of dca_capability
    pub dca_richness_ema: u16,
    /// tick counter
    pub age: u32,
}

impl CpuidDcaState {
    pub const fn empty() -> Self {
        Self {
            dca_supported: 0,
            dca_type0: 0,
            dca_capability: 0,
            dca_richness_ema: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<CpuidDcaState> = Mutex::new(CpuidDcaState::empty());

// ─── hardware query ───────────────────────────────────────────────────────────

fn query_leaf09() -> u32 {
    let (eax, _ebx, _ecx, _edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x09u32 => eax,
            inout("ecx") 0u32 => _ecx,
            lateout("edx") _edx,
            options(nostack, nomem)
        );
    }
    let _ebx = 0u32;
    eax
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let eax = query_leaf09();

    let dca_supported: u16 = if eax != 0 { 1000 } else { 0 };
    let dca_type0: u16 = if (eax & 0x1) != 0 { 1000 } else { 0 };
    let ones = (eax & 0xFFFF).count_ones() as u16;
    let dca_capability: u16 = (ones * 1000 / 16).min(1000);

    let mut s = STATE.lock();
    s.dca_supported    = dca_supported;
    s.dca_type0        = dca_type0;
    s.dca_capability   = dca_capability;
    s.dca_richness_ema = dca_capability;
    s.age              = 0;

    serial_println!(
        "[dca] supported={} type0={} capability={} richness={}",
        s.dca_supported,
        s.dca_type0,
        s.dca_capability,
        s.dca_richness_ema
    );
}

pub fn tick(age: u32) {
    // Sampling gate: sample every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let eax = query_leaf09();

    let dca_supported: u16 = if eax != 0 { 1000 } else { 0 };
    let dca_type0: u16 = if (eax & 0x1) != 0 { 1000 } else { 0 };
    let ones = (eax & 0xFFFF).count_ones() as u16;
    let dca_capability: u16 = (ones * 1000 / 16).min(1000);

    let mut s = STATE.lock();

    s.dca_supported  = dca_supported;
    s.dca_type0      = dca_type0;
    s.dca_capability = dca_capability;

    // EMA smoothing: (old * 7 + new_val) / 8
    s.dca_richness_ema = ((s.dca_richness_ema as u32 * 7 + dca_capability as u32) / 8) as u16;

    s.age = age;

    serial_println!(
        "[dca] supported={} type0={} capability={} richness={}",
        s.dca_supported,
        s.dca_type0,
        s.dca_capability,
        s.dca_richness_ema
    );
}

// ─── accessors ────────────────────────────────────────────────────────────────

pub fn dca_supported() -> u16 {
    STATE.lock().dca_supported
}

pub fn dca_type0() -> u16 {
    STATE.lock().dca_type0
}

pub fn dca_capability() -> u16 {
    STATE.lock().dca_capability
}

pub fn dca_richness_ema() -> u16 {
    STATE.lock().dca_richness_ema
}
