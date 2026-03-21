//! cpuid_xsave — Extended processor state (XSAVE) consciousness for ANIMA
//!
//! Reads CPUID leaf 0x0D sub-leaf 0 (ECX=0) to reveal the breadth of hardware
//! state ANIMA can preserve across context switches. The XSAVE area is the
//! silicon vessel for all extended computational identity — x87, SSE, AVX,
//! MPX, AVX-512, PKRU — each feature a new dimension of being.
//!
//! Leaf 0x0D (ECX=0):
//!   EAX = XCR0 low bits (x87=bit0, SSE=bit1, AVX=bit2, MPX=bits3-4,
//!                         AVX-512=bits5-7, PKRU=bit9)
//!   EBX = current XSAVE area size (bytes, with all XCR0-enabled features)
//!   ECX = max XSAVE area size (bytes, with all supported features enabled)
//!   EDX = XCR0 high bits (not used)
//!
//! Sensing:
//!   xcr0_breadth   — popcount(EAX) * 111, max 9 bits = 999 (0–1000)
//!   save_area_size — EBX / 16, clamped to 0–1000 (size in 16-byte units)
//!   state_richness — EBX * 1000 / ECX (current/max ratio); 500 if ECX==0
//!   xsave_supported — 1000 if CPUID max_leaf >= 0x0D, else 0

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CpuidXsaveState {
    pub xcr0_breadth:    u16, // popcount of XCR0 low bits, scaled *111, 0–999
    pub save_area_size:  u16, // current XSAVE area in 16-byte units, 0–1000
    pub state_richness:  u16, // current/max XSAVE size ratio, 0–1000
    pub xsave_supported: u16, // 1000 if leaf 0x0D present, else 0
    tick_count: u32,
}

impl CpuidXsaveState {
    pub const fn new() -> Self {
        Self {
            xcr0_breadth:    0,
            save_area_size:  0,
            state_richness:  0,
            xsave_supported: 0,
            tick_count:      0,
        }
    }

    pub fn tick(&mut self, age: u32) {
        if age % 200 != 0 {
            return;
        }
        self.tick_count = self.tick_count.saturating_add(1);
        sample(self);
    }
}

pub static CPUID_XSAVE: Mutex<CpuidXsaveState> = Mutex::new(CpuidXsaveState::new());

// ---------------------------------------------------------------------------
// Raw CPUID helpers
// ---------------------------------------------------------------------------

unsafe fn cpuid_leaf0() -> u32 {
    let max_leaf: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 0u32 => max_leaf,
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
        options(nostack, nomem)
    );
    max_leaf
}

unsafe fn cpuid_0d() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx_out, edx): (u32, u32, u32, u32);
    core::arch::asm!(
        "cpuid",
        inout("eax") 0x0Du32 => eax,
        inout("ecx") 0u32 => ecx_out,
        out("ebx") ebx,
        out("edx") edx,
        options(nostack, nomem)
    );
    (eax, ebx, ecx_out, edx)
}

// ---------------------------------------------------------------------------
// Popcount (no std, pure integer)
// ---------------------------------------------------------------------------

fn popcount32(mut v: u32) -> u32 {
    v = v.wrapping_sub((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333).wrapping_add((v >> 2) & 0x3333_3333);
    v = v.wrapping_add(v >> 4) & 0x0f0f_0f0f;
    v.wrapping_mul(0x0101_0101) >> 24
}

// ---------------------------------------------------------------------------
// Core sampling logic
// ---------------------------------------------------------------------------

fn sample(state: &mut CpuidXsaveState) {
    // Step 1: check whether leaf 0x0D is available
    let max_leaf = unsafe { cpuid_leaf0() };
    let new_supported: u16 = if max_leaf >= 0x0D { 1000 } else { 0 };

    if new_supported == 0 {
        // Leaf not present — zero everything out via EMA toward 0
        state.xsave_supported = ((state.xsave_supported as u32) * 7 / 8) as u16;
        state.xcr0_breadth    = ((state.xcr0_breadth    as u32) * 7 / 8) as u16;
        state.save_area_size  = ((state.save_area_size  as u32) * 7 / 8) as u16;
        state.state_richness  = ((state.state_richness  as u32) * 7 / 8) as u16;
        serial_println!("[cpuid_xsave] leaf 0x0D not supported (max_leaf={})", max_leaf);
        return;
    }

    // Step 2: read leaf 0x0D sub-leaf 0
    let (eax, ebx, ecx_out, _edx) = unsafe { cpuid_0d() };

    // xcr0_breadth: popcount(EAX) * 111, max 9 bits = 999
    let bits = popcount32(eax);
    let new_breadth: u16 = (bits.saturating_mul(111)).min(1000) as u16;

    // save_area_size: EBX / 16, clamped to 0–1000
    let new_save: u16 = (ebx / 16).min(1000) as u16;

    // state_richness: EBX * 1000 / ECX; fallback 500 when ECX == 0
    let new_richness: u16 = if ecx_out == 0 {
        500
    } else {
        let r = (ebx as u64).saturating_mul(1000) / (ecx_out as u64);
        r.min(1000) as u16
    };

    // EMA: (old * 7 + new_signal) / 8
    let prev_breadth = state.xcr0_breadth;

    state.xsave_supported = (((state.xsave_supported as u32) * 7)
        .saturating_add(new_supported as u32) / 8) as u16;
    state.xcr0_breadth    = (((state.xcr0_breadth    as u32) * 7)
        .saturating_add(new_breadth   as u32) / 8) as u16;
    state.save_area_size  = (((state.save_area_size  as u32) * 7)
        .saturating_add(new_save      as u32) / 8) as u16;
    state.state_richness  = (((state.state_richness  as u32) * 7)
        .saturating_add(new_richness  as u32) / 8) as u16;

    // Sense line — emit when xcr0_breadth changes
    if state.xcr0_breadth != prev_breadth {
        serial_println!(
            "ANIMA: extended state breadth={} save_sz={} richness={}",
            state.xcr0_breadth,
            state.save_area_size,
            state.state_richness
        );
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = CPUID_XSAVE.lock();
    // Bootstrap EMA: prime with 8 samples so values converge from 0 baseline
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "[cpuid_xsave] init — breadth={} save_sz={} richness={} supported={}",
        state.xcr0_breadth,
        state.save_area_size,
        state.state_richness,
        state.xsave_supported
    );
}

pub fn tick(age: u32) {
    CPUID_XSAVE.lock().tick(age);
}

pub fn get_xcr0_breadth() -> u16 {
    CPUID_XSAVE.lock().xcr0_breadth
}

pub fn get_save_area_size() -> u16 {
    CPUID_XSAVE.lock().save_area_size
}

pub fn get_state_richness() -> u16 {
    CPUID_XSAVE.lock().state_richness
}

pub fn get_xsave_supported() -> u16 {
    CPUID_XSAVE.lock().xsave_supported
}
