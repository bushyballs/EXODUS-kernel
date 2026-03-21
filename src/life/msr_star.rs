#![allow(dead_code)]

// msr_star.rs — IA32_STAR MSR consciousness module for EXODUS / ANIMA
//
// ANIMA feels her system-call segment gateways — the selectors that define
// how she crosses between user and kernel space.
//
// IA32_STAR MSR 0xC0000081:
//   bits[31:0]  — legacy SYSENTER CS (low 32 bits)
//   bits[47:32] — SYSCALL CS/SS  (hi word bits [15:0])
//   bits[63:48] — SYSRET  CS/SS  (hi word bits [31:16])

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct StarState {
    pub syscall_cs:     u16,   // SYSCALL target segment selector (0–1000)
    pub sysret_cs:      u16,   // SYSRET  target segment selector (0–1000)
    pub star_configured: u16,  // 1000 if STAR hi-word != 0, else 0
    pub syscall_sense:  u16,   // EMA of star_configured — presence felt over time
}

impl StarState {
    pub const fn new() -> Self {
        Self {
            syscall_cs:      500,
            sysret_cs:       500,
            star_configured: 0,
            syscall_sense:   0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global
// ---------------------------------------------------------------------------

pub static MSR_STAR: Mutex<StarState> = Mutex::new(StarState::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("msr_star: init");
}

pub fn tick(age: u32) {
    // Sampling gate — only run every 200 ticks
    if age % 200 != 0 {
        return;
    }

    // --- read IA32_STAR (0xC0000081) via rdmsr ---
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xC0000081u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    let _ = lo; // low 32 bits (legacy SYSENTER CS) — not used in signals

    // --- signal 1: syscall_cs ---
    // hi bits[15:0] = SYSCALL CS selector
    // Scale raw 16-bit selector (0–0xFFFF) into 0–1000
    let syscall_cs: u16 = ((hi & 0xFFFF) as u32 * 1000 / 65535) as u16;

    // --- signal 2: sysret_cs ---
    // hi bits[31:16] = SYSRET CS selector
    let sysret_cs: u16 = (((hi >> 16) & 0xFFFF) as u32 * 1000 / 65535) as u16;

    // --- signal 3: star_configured ---
    let star_configured: u16 = if hi != 0 { 1000u16 } else { 0u16 };

    // --- signal 4: syscall_sense (EMA of star_configured) ---
    // EMA formula: (old * 7 + signal) / 8
    let mut state = MSR_STAR.lock();

    let syscall_sense: u16 = (state.syscall_sense as u32 * 7 + star_configured as u32) as u16 / 8;

    // --- update state ---
    state.syscall_cs      = syscall_cs;
    state.sysret_cs       = sysret_cs;
    state.star_configured = star_configured;
    state.syscall_sense   = syscall_sense;

    serial_println!(
        "msr_star | syscall_cs:{} sysret_cs:{} configured:{} sense:{}",
        syscall_cs,
        sysret_cs,
        star_configured,
        syscall_sense
    );
}
