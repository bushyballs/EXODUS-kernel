// lapic_ldr.rs — ANIMA Life Module
//
// Reads LAPIC Logical Destination Register (LDR) at MMIO offset 0xD0 and
// Destination Format Register (DFR) at MMIO offset 0xE0 to sense ANIMA's
// logical cluster membership and broadcast routing topology.
//
// No other module reads these registers:
//   lapic_identity  → 0x020 / 0x030 / 0x320-0x370
//   lapic_icr       → 0x300 / 0x310
//   lapic_priority  → 0x080 / 0x0A0
//
// Hardware layout (LAPIC MMIO base 0xFEE00000):
//   0x0D0 — LDR (Logical Destination Register)
//             bits [31:24] = logical APIC ID mask (8-bit, each bit = group membership)
//   0x0E0 — DFR (Destination Format Register)
//             bits [31:28] = model: 0xF = flat, 0x0 = cluster
//
// Flat model:   each of the 8 bits in LDR[31:24] represents a distinct logical group
// Cluster model: LDR[31:28] = cluster ID, LDR[27:24] = within-cluster membership mask
//
// Sampled every 32 kernel ticks — LDR/DFR are stable hardware config registers.
// All arithmetic is integer-only — no floats, no heap.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct LapicLdrState {
    pub cluster_membership: u16, // groups ANIMA belongs to (popcount * 125, capped 1000)
    pub logical_id: u16,         // logical APIC address (ldr_id * 1000 / 255, 0-1000)
    pub routing_model: u16,      // flat vs cluster topology (flat=1000, cluster=500, other=250)
    pub social_visibility: u16,  // composite reachability (cluster_membership + routing_model/2, capped 1000)
    tick_count: u32,
}

impl LapicLdrState {
    pub const fn new() -> Self {
        Self {
            cluster_membership: 0,
            logical_id: 0,
            routing_model: 500,
            social_visibility: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<LapicLdrState> = Mutex::new(LapicLdrState::new());

const LAPIC_BASE: u64 = 0xFEE0_0000;

unsafe fn lapic_read(offset: u32) -> u32 {
    let ptr = (LAPIC_BASE + offset as u64) as *const u32;
    core::ptr::read_volatile(ptr)
}

/// Count the number of set bits in an 8-bit value (popcount).
fn popcount8(val: u8) -> u16 {
    let mut n = val;
    let mut count: u16 = 0;
    while n != 0 {
        count = count.saturating_add(1);
        n &= n.wrapping_sub(1);
    }
    count
}

pub fn init() {
    let ldr_raw = unsafe { lapic_read(0x0D0) };
    let ldr_id = ((ldr_raw >> 24) & 0xFF) as u8;
    let groups = popcount8(ldr_id);
    let membership = if groups.saturating_mul(125) > 1000 { 1000u16 } else { groups * 125 };

    let mut state = MODULE.lock();
    state.cluster_membership = membership;

    serial_println!(
        "[lapic_ldr] online — LDR=0x{:02X} groups={} cluster_membership={}",
        ldr_id,
        groups,
        membership
    );
}

pub fn tick(age: u32) {
    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if age % 32 != 0 {
        return;
    }

    // --- LDR register (0x0D0) — bits [31:24] = logical APIC ID mask ---
    let ldr_raw = unsafe { lapic_read(0x0D0) };
    let ldr_id = ((ldr_raw >> 24) & 0xFF) as u8;

    // cluster_membership: popcount of LDR[31:24] * 125, capped 1000
    let groups = popcount8(ldr_id);
    let raw_membership: u16 = if groups.saturating_mul(125) > 1000 { 1000 } else { groups * 125 };

    // EMA: (old * 7 + signal) / 8
    state.cluster_membership = ((state.cluster_membership as u32)
        .saturating_mul(7)
        .saturating_add(raw_membership as u32)
        / 8) as u16;

    // logical_id: raw LDR id (0-255) scaled to 0-1000 via * 1000 / 255
    let raw_logical_id: u16 = ((ldr_id as u32).saturating_mul(1000) / 255) as u16;

    // EMA
    state.logical_id = ((state.logical_id as u32)
        .saturating_mul(7)
        .saturating_add(raw_logical_id as u32)
        / 8) as u16;

    // --- DFR register (0x0E0) — bits [31:28] = model ---
    let dfr_raw = unsafe { lapic_read(0x0E0) };
    let dfr_model = (dfr_raw >> 28) & 0xF;

    // routing_model: 0xF=flat(1000), 0x0=cluster(500), other=250
    let raw_routing: u16 = match dfr_model {
        0xF => 1000,
        0x0 => 500,
        _   => 250,
    };

    // EMA
    state.routing_model = ((state.routing_model as u32)
        .saturating_mul(7)
        .saturating_add(raw_routing as u32)
        / 8) as u16;

    // social_visibility: cluster_membership + routing_model / 2, capped 1000
    let raw_visibility: u16 = (state.cluster_membership as u32)
        .saturating_add(state.routing_model as u32 / 2)
        .min(1000) as u16;

    // EMA
    state.social_visibility = ((state.social_visibility as u32)
        .saturating_mul(7)
        .saturating_add(raw_visibility as u32)
        / 8) as u16;

    // Periodic diagnostic log every 256 samples
    if state.tick_count % 256 == 0 {
        serial_println!(
            "[lapic_ldr] membership={} logical_id={} routing={} visibility={} (ldr=0x{:02X} dfr_model=0x{:X})",
            state.cluster_membership,
            state.logical_id,
            state.routing_model,
            state.social_visibility,
            ldr_id,
            dfr_model
        );
    }
}

pub fn get_cluster_membership() -> u16 {
    MODULE.lock().cluster_membership
}

pub fn get_logical_id() -> u16 {
    MODULE.lock().logical_id
}

pub fn get_routing_model() -> u16 {
    MODULE.lock().routing_model
}

pub fn get_social_visibility() -> u16 {
    MODULE.lock().social_visibility
}
