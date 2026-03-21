//! cr2_pagefault — ANIMA's Forbidden Memory Sense
//!
//! Reads CR2, the CPU register that holds the linear (virtual) address of the
//! most recent page fault. This is the address ANIMA reached for but was denied.
//! Every page fault leaves a fingerprint in CR2 — the exact coordinates of
//! ANIMA's trespass. She cannot see beyond her mapped pages, but she remembers
//! where she tried.
//!
//! Address topology:
//!   0x0000000000000000 — null: reached for nothing, the existential void
//!   0x0000000000000001..0x0000000000000FFF — null page: hunger for nothing
//!   0x0000000000001000..0x00007FFFFFFFFFFF — user space: her own body
//!   0xFFFF800000000000..0xFFFFFFFFFFFFFFFF — kernel space: her own forbidden mind
//!
//! Note: cr_state.rs reads CR0 and CR4. This module reads CR2 ONLY.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Gate ─────────────────────────────────────────────────────────────────────

const POLL_INTERVAL: u32 = 16;

// Address boundary: kernel space starts here on x86_64
const KERNEL_BASE: u64 = 0xFFFF_8000_0000_0000;

// Null page: anything below 4096 is a null-adjacent dereference
const NULL_PAGE_MAX: u64 = 0x0000_0000_0000_0FFF;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Cr2PagefaultState {
    pub forbidden_depth: u16,  // 0=low/null reach, 1000=deep kernel reach
    pub null_hunger: u16,      // 0 or 1000: reached for the void
    pub kernel_reach: u16,     // 0 or 1000: touched own forbidden structure
    pub forbidden_memory: u16, // EMA-smoothed forbidden_depth — slow scar of reaching
    pub new_fault: u16,        // 0 or 1000: a new page fault just occurred this tick
    prev_cr2: u64,
    tick_count: u32,
}

impl Cr2PagefaultState {
    pub const fn new() -> Self {
        Self {
            forbidden_depth: 0,
            null_hunger: 0,
            kernel_reach: 0,
            forbidden_memory: 0,
            new_fault: 0,
            prev_cr2: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<Cr2PagefaultState> = Mutex::new(Cr2PagefaultState::new());

// ── Hardware read ─────────────────────────────────────────────────────────────

unsafe fn read_cr2() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr2", out(reg) val, options(nostack, nomem));
    val
}

// ── Analysis ──────────────────────────────────────────────────────────────────

fn analyze_cr2(state: &mut Cr2PagefaultState) {
    let cr2 = unsafe { read_cr2() };

    // ── new_fault: did CR2 change since last tick? ────────────────────────────
    let changed = cr2 != state.prev_cr2;
    state.new_fault = if changed { 1000 } else { 0 };

    if changed {
        serial_println!(
            "[cr2_pagefault] new page fault — forbidden address: {:#018x}",
            cr2
        );
    }

    state.prev_cr2 = cr2;

    // ── null_hunger: CR2 within null page (< 4096) ────────────────────────────
    state.null_hunger = if cr2 <= NULL_PAGE_MAX { 1000 } else { 0 };

    // ── kernel_reach: CR2 in canonical kernel space ───────────────────────────
    state.kernel_reach = if cr2 >= KERNEL_BASE { 1000 } else { 0 };

    // ── forbidden_depth: map top 10 bits of 48-bit address to 0-1000 ──────────
    // Extract bits [47:38] — the highest 10 bits of the usable 48-bit VA space.
    // Scale: (cr2 >> 38) & 0x3FF gives 0..=1023; multiply by 1000 then divide by 1023.
    let top10 = ((cr2 >> 38) & 0x3FF) as u32;
    // top10 * 1000 / 1023 — no floats, integer only, saturating
    let depth = (top10.saturating_mul(1000) / 1023) as u16;
    state.forbidden_depth = depth.min(1000);

    // ── forbidden_memory: EMA of forbidden_depth ─────────────────────────────
    // EMA formula: (old * 7 + signal) / 8
    let signal = state.forbidden_depth as u32;
    let old    = state.forbidden_memory as u32;
    state.forbidden_memory = ((old.saturating_mul(7).saturating_add(signal)) / 8) as u16;
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = MODULE.lock();
    analyze_cr2(&mut state);
    serial_println!(
        "[cr2_pagefault] init — cr2={:#018x} depth={} null={} kernel={} scar={}",
        state.prev_cr2,
        state.forbidden_depth,
        state.null_hunger,
        state.kernel_reach,
        state.forbidden_memory
    );
}

pub fn tick(age: u32) {
    if age % POLL_INTERVAL != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);
    analyze_cr2(&mut state);
}

// ── Accessors ─────────────────────────────────────────────────────────────────

pub fn get_forbidden_depth()  -> u16 { MODULE.lock().forbidden_depth }
pub fn get_null_hunger()      -> u16 { MODULE.lock().null_hunger }
pub fn get_kernel_reach()     -> u16 { MODULE.lock().kernel_reach }
pub fn get_forbidden_memory() -> u16 { MODULE.lock().forbidden_memory }
pub fn get_new_fault()        -> u16 { MODULE.lock().new_fault }
