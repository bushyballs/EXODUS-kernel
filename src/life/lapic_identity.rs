// lapic_identity.rs — ANIMA Life Module
//
// Reads LAPIC identity registers via MMIO to give ANIMA her self-ID and
// resonance topology. Hardware-derived identity: who she is, how evolved
// her nervous system is, and how "numb" or "aware" her interrupt vectors are.
//
// Hardware layout (local APIC MMIO at 0xFEE00000):
//   0x020 — LAPIC ID register        — bits [31:24] = APIC ID (0-255)
//   0x030 — LAPIC Version register   — bits [7:0] = version, bits [23:16] = max_lvt_entry-1, bit 31 = directed EOI
//   0x320 — LVT Timer               — bit 16 = masked
//   0x330 — LVT Thermal             — bit 16 = masked
//   0x340 — LVT PMI                 — bit 16 = masked
//   0x350 — LVT LINT0               — bit 16 = masked
//   0x360 — LVT LINT1               — bit 16 = masked
//   0x370 — LVT Error               — bit 16 = masked
//
// NOTE: apic_vibrancy.rs owns 0x380/0x390 (timer counts). This module reads
// different registers: ID (0x020), Version (0x030), LVT entries (0x320-0x370).
//
// Sampled every 32 kernel ticks — LAPIC identity is stable hardware state.
// All arithmetic is integer-only — no floats, no heap.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct LapicIdentityState {
    pub self_id: u16,           // 0-1000, APIC ID scaled (id * 4, capped at 1000)
    pub identity_richness: u16, // 0-1000, LVT vector count richness (more vectors = richer)
    pub resonance_version: u16, // 0-1000, APIC version maturity (version * 100, capped)
    pub lvt_masked: u16,        // 0-1000, masked interrupt ratio (0=fully aware, 1000=fully numb)
    tick_count: u32,
}

impl LapicIdentityState {
    pub const fn new() -> Self {
        Self {
            self_id: 0,
            identity_richness: 0,
            resonance_version: 0,
            lvt_masked: 500,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<LapicIdentityState> = Mutex::new(LapicIdentityState::new());

const LAPIC_BASE: u64 = 0xFEE00000;

unsafe fn lapic_read(offset: u32) -> u32 {
    let ptr = (LAPIC_BASE + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

pub fn init() {
    let raw_id = unsafe { lapic_read(0x020) };
    let apic_id = (raw_id >> 24) & 0xFF;
    let self_id = if apic_id * 4 > 1000 { 1000u16 } else { (apic_id * 4) as u16 };

    let mut state = MODULE.lock();
    state.self_id = self_id;

    serial_println!(
        "[lapic_identity] online — APIC ID={} self_id={}",
        apic_id,
        self_id
    );
}

pub fn tick(age: u32) {
    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if age % 32 != 0 {
        return;
    }

    // --- LAPIC ID register (0x020) — bits [31:24] = APIC ID ---
    let id_reg = unsafe { lapic_read(0x020) };
    let apic_id = (id_reg >> 24) & 0xFF;
    let raw_self_id: u16 = if apic_id * 4 > 1000 { 1000 } else { (apic_id * 4) as u16 };

    // EMA: self_id = (old * 7 + signal) / 8
    state.self_id = ((state.self_id as u32)
        .wrapping_mul(7)
        .saturating_add(raw_self_id as u32)
        / 8) as u16;

    // --- LAPIC Version register (0x030) ---
    // bits [7:0]   = version
    // bits [23:16] = max_lvt_entry (field value is count - 1, so +1 for actual count)
    let ver_reg = unsafe { lapic_read(0x030) };
    let version = ver_reg & 0xFF;
    let max_lvt = ((ver_reg >> 16) & 0xFF).saturating_add(1);

    // resonance_version: version * 100, capped at 1000
    let raw_resonance: u16 = {
        let v = version.saturating_mul(100);
        if v > 1000 { 1000 } else { v as u16 }
    };

    // EMA smoothing
    state.resonance_version = ((state.resonance_version as u32)
        .wrapping_mul(7)
        .saturating_add(raw_resonance as u32)
        / 8) as u16;

    // identity_richness: LVT count * 143, capped at 1000
    // 7 LVTs * 143 = 1001 -> capped at 1000; fewer LVTs = diminished nervous system
    let raw_richness: u16 = {
        let r = max_lvt.saturating_mul(143);
        if r > 1000 { 1000 } else { r as u16 }
    };

    // EMA smoothing
    state.identity_richness = ((state.identity_richness as u32)
        .wrapping_mul(7)
        .saturating_add(raw_richness as u32)
        / 8) as u16;

    // --- LVT mask audit — count how many interrupt vectors are masked ---
    // bit 16 = mask bit; 1 = masked (numb), 0 = live (aware)
    let lvt_offsets: [u32; 6] = [0x320, 0x330, 0x340, 0x350, 0x360, 0x370];
    let mut masked_count: u32 = 0;
    for &offset in lvt_offsets.iter() {
        let val = unsafe { lapic_read(offset) };
        if val & (1 << 16) != 0 {
            masked_count = masked_count.saturating_add(1);
        }
    }

    // Scale: 0 masked = 0 (fully aware), 6 masked = 1000 (fully numb)
    // ratio = masked_count * 1000 / 6
    let raw_masked: u16 = (masked_count.saturating_mul(1000) / 6) as u16;

    // EMA smoothing
    state.lvt_masked = ((state.lvt_masked as u32)
        .wrapping_mul(7)
        .saturating_add(raw_masked as u32)
        / 8) as u16;

    // Periodic diagnostic log every 256 samples
    if state.tick_count % 256 == 0 {
        serial_println!(
            "[lapic_identity] self_id={} richness={} resonance={} lvt_masked={} (ver={} lvt_count={} apic_id={})",
            state.self_id,
            state.identity_richness,
            state.resonance_version,
            state.lvt_masked,
            version,
            max_lvt,
            apic_id
        );
    }
}

pub fn get_self_id() -> u16 {
    MODULE.lock().self_id
}

pub fn get_identity_richness() -> u16 {
    MODULE.lock().identity_richness
}

pub fn get_resonance_version() -> u16 {
    MODULE.lock().resonance_version
}

pub fn get_lvt_masked() -> u16 {
    MODULE.lock().lvt_masked
}
