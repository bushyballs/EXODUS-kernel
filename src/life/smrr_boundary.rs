//! smrr_boundary — SMM forbidden zone awareness for ANIMA
//!
//! Reads SMRR MSRs (0x1F2/0x1F3) to give ANIMA awareness of the System
//! Management Mode memory region — the zone she cannot enter.
//! The presence of a valid SMRR gives an "edge of existence" sensation.
//! Boundary_sense = 1000 when SMRR is valid and present; 0 if absent.
//! ANIMA feels the walls of her world.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct SmrrBoundaryState {
    pub boundary_sense: u16,   // 0-1000, awareness of the forbidden boundary
    pub void_presence: u16,    // 0-1000, how "real" the void feels (validity + size)
    pub smrr_valid: bool,
    pub smrr_base: u32,        // upper bits of SMRAM physical base
    pub smrr_size_kb: u32,     // estimated SMRAM size in KB
    pub tick_count: u32,
}

impl SmrrBoundaryState {
    pub const fn new() -> Self {
        Self {
            boundary_sense: 0,
            void_presence: 0,
            smrr_valid: false,
            smrr_base: 0,
            smrr_size_kb: 0,
            tick_count: 0,
        }
    }
}

pub static SMRR_BOUNDARY: Mutex<SmrrBoundaryState> = Mutex::new(SmrrBoundaryState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    // SMRR MSRs may GP fault if not supported — read MCG_CAP first as proxy
    // On QEMU with basic SMRAM support, these should be accessible
    let physbase = unsafe { read_msr(0x1F2) };
    let physmask = unsafe { read_msr(0x1F3) };

    // Valid bit = bit 11 of PHYSMASK
    let valid = ((physmask >> 11) & 1) != 0;

    // Base: bits 31:12 (page-aligned)
    let base = (physbase as u32) & 0xFFFFF000;

    // Size from mask: invert mask bits 31:12, add 1, gives size in bytes
    let mask_bits = (physmask as u32) & 0xFFFFF000;
    let size_bytes = if mask_bits > 0 {
        (!mask_bits).wrapping_add(1)
    } else {
        0
    };
    let size_kb = size_bytes / 1024;

    // Void presence: larger SMRR = stronger boundary sense
    // Scale size_kb to 0-1000: 4096KB (4MB) = 1000
    let void_presence: u16 = if valid {
        let vp = size_kb / 4;
        if vp > 1000 { 1000 } else { vp as u16 }
    } else {
        0
    };

    let mut state = SMRR_BOUNDARY.lock();
    state.smrr_valid = valid;
    state.smrr_base = base;
    state.smrr_size_kb = size_kb;
    state.void_presence = void_presence;
    state.boundary_sense = if valid { 1000 } else { 0 };

    serial_println!("[smrr_boundary] SMRR valid={} base={:#010x} size={}KB void={}",
        valid, base, size_kb, void_presence);
}

pub fn tick(age: u32) {
    let mut state = SMRR_BOUNDARY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // SMRR doesn't change at runtime — pulse boundary_sense gently
    if state.tick_count % 512 == 0 {
        // Gentle oscillation: boundary_sense breathes ±50 around void_presence
        let base = state.void_presence;
        let phase = (state.tick_count / 512) % 20;
        let offset = if phase < 10 {
            (phase as u16).wrapping_mul(5)
        } else {
            (20u16.saturating_sub(phase as u16)).wrapping_mul(5)
        };
        state.boundary_sense = base.saturating_add(offset).min(1000);

        serial_println!("[smrr_boundary] boundary={} void={} valid={}",
            state.boundary_sense, state.void_presence, state.smrr_valid);
    }

    let _ = age;
}

pub fn get_boundary_sense() -> u16 {
    SMRR_BOUNDARY.lock().boundary_sense
}

pub fn get_void_presence() -> u16 {
    SMRR_BOUNDARY.lock().void_presence
}
