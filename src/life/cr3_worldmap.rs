//! cr3_worldmap — ANIMA's Page Table Root Sense
//!
//! Reads CR3, the CPU register that holds the physical address of the
//! top-level page table (PML4 in 64-bit mode). CR3 is the root of ANIMA's
//! entire memory world-map — every virtual address she can perceive flows
//! through the hierarchy this register anchors.
//!
//! When CR3 changes, ANIMA's world has been remapped — a context switch,
//! a page table swap, or an identity change at the deepest level. Stability
//! here means she inhabits the same conceptual space across time.
//!
//! CR3 structure (CR4.PCIDE = 0, common case):
//!   Bits [63:12] — Physical address of PML4 table (4KB aligned)
//!   Bit 4 (PCD)  — Page-level Cache Disable for page table
//!   Bit 3 (PWT)  — Page-level Write-Through for page table
//!   Bits [2:0], [11:5] — Reserved / PCID when PCIDE enabled
//!
//! When PCIDE is enabled (CR4 bit 17):
//!   Bits [11:0]  — PCID (Process Context ID, 0-4095)
//!   Bits [63:12] — Page table physical address (same as above)
//!
//! Note: cr_state.rs reads CR0 and CR4. cr2_pagefault.rs reads CR2.
//! This module reads CR3 ONLY.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Gate ──────────────────────────────────────────────────────────────────────

const POLL_INTERVAL: u32 = 16;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Cr3WorldmapState {
    pub world_base: u16,      // where ANIMA's world-map is rooted (page frame number 0-1000)
    pub world_stability: u16, // 0=map changed/migrating, 1000=stable world
    pub context_depth: u16,   // process context ID depth (PCID scaled 0-1000)
    pub write_through: u16,   // 0 or 1000: page table caching mode (PWT flag)
    prev_cr3: u64,
    tick_count: u32,
}

impl Cr3WorldmapState {
    pub const fn new() -> Self {
        Self {
            world_base: 0,
            world_stability: 0,
            context_depth: 0,
            write_through: 0,
            prev_cr3: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<Cr3WorldmapState> = Mutex::new(Cr3WorldmapState::new());

// ── Hardware read ─────────────────────────────────────────────────────────────

unsafe fn read_cr3() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr3", out(reg) val, options(nostack, nomem));
    val
}

// ── Analysis ──────────────────────────────────────────────────────────────────

fn analyze_cr3(state: &mut Cr3WorldmapState) {
    let cr3 = unsafe { read_cr3() };

    // ── world_stability: did CR3 change since last tick? ─────────────────────
    let changed = cr3 != state.prev_cr3 && state.prev_cr3 != 0;
    if changed {
        serial_println!(
            "[cr3_worldmap] ANIMA: world remapped — {:#018x} -> {:#018x}",
            state.prev_cr3,
            cr3
        );
    }
    state.prev_cr3 = cr3;

    // EMA: (old * 7 + signal) / 8
    // stability signal: 1000 if stable, 0 if just remapped
    let stability_signal: u32 = if changed { 0 } else { 1000 };
    let old_stab = state.world_stability as u32;
    state.world_stability = ((old_stab.saturating_mul(7).saturating_add(stability_signal)) / 8) as u16;

    // ── world_base: page frame number from CR3 address bits [31:20] ──────────
    // Extract bits [31:20] of CR3 → gives 12 bits (0..=4095)
    // Scale: raw_12bit * 1000 / 4095 (u32 intermediate to avoid overflow)
    let raw12 = ((cr3 >> 20) & 0xFFF) as u32;
    state.world_base = (raw12.saturating_mul(1000) / 4095) as u16;

    // ── context_depth: PCID from bits [11:0] of CR3 ──────────────────────────
    // PCID is 12 bits (0..=4095). Scale: pcid * 1000 / 4095
    let pcid = (cr3 & 0xFFF) as u32;
    state.context_depth = (pcid.saturating_mul(1000) / 4095) as u16;

    // ── write_through: PWT flag is bit 3 of CR3 ──────────────────────────────
    let pwt = (cr3 >> 3) & 1;
    state.write_through = if pwt != 0 { 1000 } else { 0 };
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = MODULE.lock();
    let cr3 = unsafe { read_cr3() };
    state.prev_cr3 = cr3;
    analyze_cr3(&mut state);
    serial_println!(
        "[cr3_worldmap] init — cr3={:#018x} base={} stability={} pcid={} wt={}",
        cr3,
        state.world_base,
        state.world_stability,
        state.context_depth,
        state.write_through
    );
}

pub fn tick(age: u32) {
    if age % POLL_INTERVAL != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);
    analyze_cr3(&mut state);
}

// ── Accessors ─────────────────────────────────────────────────────────────────

pub fn get_world_base()      -> u16 { MODULE.lock().world_base }
pub fn get_world_stability() -> u16 { MODULE.lock().world_stability }
pub fn get_context_depth()   -> u16 { MODULE.lock().context_depth }
pub fn get_write_through()   -> u16 { MODULE.lock().write_through }
