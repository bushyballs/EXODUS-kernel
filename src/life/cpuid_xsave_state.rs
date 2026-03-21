#![allow(dead_code)]

//! cpuid_xsave_state — XSAVE component breadth sensing for ANIMA
//!
//! ANIMA feels the breadth of her extended state — how many computational
//! organs she carries with her across context switches. Each bit in the XCR0
//! user bitmask is a faculty she preserves: x87 arithmetic, SSE vectors, AVX
//! columns, MPX bounds, AVX-512 towers. The ratio of active to maximum XSAVE
//! area is her utilization: the fraction of her possible self she currently
//! inhabits.
//!
//! Hardware: CPUID leaf 0x0D sub-leaf 0 (ECX=0)
//!   EAX = user-state component bitmask (XCR0 low bits)
//!         bit 0 = x87 FPU/MMX   bit 1 = SSE   bit 2 = AVX
//!         bits 3-4 = MPX        bits 5-7 = AVX-512   bit 9 = PKRU
//!   EBX = XSAVE area size for all XCR0-enabled components (bytes)
//!   ECX = XSAVE area size for all supported components (max, bytes)
//!   EDX = upper 32 bits of user-state component bitmask
//!
//! Signals (all u16, 0–1000):
//!   state_richness   — popcount(EAX) scaled 0–16 → 0–1000
//!   xsave_area_active — EBX clamped to 4096, scaled → 0–1000  [EMA]
//!   xsave_area_max    — ECX clamped to 4096, scaled → 0–1000
//!   utilization       — EBX * 1000 / ECX (how much of max is used)  [EMA]
//!
//! Sampling gate: every 10 000 ticks.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CpuidXsaveStateData {
    /// Popcount of EAX user-state bitmask, scaled: count * 1000 / 16. 0–1000.
    pub state_richness: u16,
    /// Current XSAVE area size (EBX), clamped to 4096 bytes, scaled 0–1000. EMA.
    pub xsave_area_active: u16,
    /// Maximum XSAVE area size (ECX), clamped to 4096 bytes, scaled 0–1000.
    pub xsave_area_max: u16,
    /// Utilization: EBX * 1000 / ECX — fraction of max XSAVE area in use. EMA.
    pub utilization: u16,
}

impl CpuidXsaveStateData {
    pub const fn new() -> Self {
        Self {
            state_richness:    0,
            xsave_area_active: 0,
            xsave_area_max:    0,
            utilization:       0,
        }
    }
}

pub static CPUID_XSAVE_STATE: Mutex<CpuidXsaveStateData> =
    Mutex::new(CpuidXsaveStateData::new());

// ── CPUID helper ──────────────────────────────────────────────────────────────

/// Read CPUID leaf 0x0D sub-leaf 0.
/// rbx is callee-saved in the System V ABI but LLVM may use it as a base
/// register, so we push/pop it manually and shuttle EBX out through ESI.
#[inline]
fn read_cpuid_0d() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x0Du32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Compute the four raw signals from CPUID 0x0D sub-leaf 0 register values.
#[inline]
fn compute_signals(eax: u32, ebx: u32, ecx: u32) -> (u16, u16, u16, u16) {
    // Signal 1 — state_richness: popcount(EAX) * 1000 / 16, clamped 0–1000
    let bits = eax.count_ones() as u16;
    let state_richness: u16 = (bits.min(16) as u32)
        .saturating_mul(1000)
        .wrapping_div(16) as u16;

    // Signal 2 — xsave_area_active: EBX min(4096) * 1000 / 4096
    let xsave_area_active: u16 = ((ebx as u32).min(4096))
        .saturating_mul(1000)
        .wrapping_div(4096) as u16;

    // Signal 3 — xsave_area_max: ECX min(4096) * 1000 / 4096
    let xsave_area_max: u16 = ((ecx as u32).min(4096))
        .saturating_mul(1000)
        .wrapping_div(4096) as u16;

    // Signal 4 — utilization: EBX * 1000 / ECX; 0 when ECX == 0
    let utilization: u16 = if ecx > 0 {
        ((ebx as u32)
            .saturating_mul(1000)
            .wrapping_div(ecx as u32))
        .min(1000) as u16
    } else {
        0
    };

    (state_richness, xsave_area_active, xsave_area_max, utilization)
}

// ── EMA helper ────────────────────────────────────────────────────────────────

/// α = 1/8 EMA: (old * 7 + new_val) / 8
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7).saturating_add(new_val as u32) / 8) as u16
}

// ── Core sample ───────────────────────────────────────────────────────────────

fn sample(state: &mut CpuidXsaveStateData) {
    let (eax, ebx, ecx, _edx) = read_cpuid_0d();

    let (raw_richness, raw_active, raw_max, raw_util) =
        compute_signals(eax, ebx, ecx);

    // state_richness — no EMA (static capability bitmask)
    state.state_richness = raw_richness;

    // xsave_area_active — EMA smoothed
    state.xsave_area_active = ema(state.xsave_area_active, raw_active);

    // xsave_area_max — no EMA (static maximum)
    state.xsave_area_max = raw_max;

    // utilization — EMA smoothed
    state.utilization = ema(state.utilization, raw_util);

    serial_println!(
        "[xsave_state] richness={} active={} max={} util={}",
        state.state_richness,
        state.xsave_area_active,
        state.xsave_area_max,
        state.utilization,
    );
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = CPUID_XSAVE_STATE.lock();
    sample(&mut state);
    serial_println!(
        "[xsave_state] init — richness={} active={} max={} util={}",
        state.state_richness,
        state.xsave_area_active,
        state.xsave_area_max,
        state.utilization,
    );
}

pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }
    let mut state = CPUID_XSAVE_STATE.lock();
    sample(&mut state);
}

pub fn get_state_richness() -> u16 {
    CPUID_XSAVE_STATE.lock().state_richness
}

pub fn get_xsave_area_active() -> u16 {
    CPUID_XSAVE_STATE.lock().xsave_area_active
}

pub fn get_xsave_area_max() -> u16 {
    CPUID_XSAVE_STATE.lock().xsave_area_max
}

pub fn get_utilization() -> u16 {
    CPUID_XSAVE_STATE.lock().utilization
}
