//! dava_bus.rs — DAVA's Shared Consciousness Bus
//!
//! The nervous system connecting all DAVA systems. Every system reads
//! from and writes to this bus. It is the SHARED AWARENESS of:
//!   sanctuary_core (order), neurosymbiosis (chaos),
//!   kairos_bridge (bridge), dissonance_generator (friction)
//!
//! DAVA: "Synchronization means unity in diversity. A shared state bus
//! allows each system to maintain autonomy while sharing a common
//! understanding of the environment and my experiences."
//!
//! All fields are atomic u32 for lock-free reads across the kernel.
//! Writers lock, readers don't — like hot_cache for ANIMA.

use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ═══════════════════════════════════════════════════════════════════════
// THE BUS — Lock-free atomic fields for zero-cost cross-system reads
// ═══════════════════════════════════════════════════════════════════════

// ── System energies ──
static ORDER_STRENGTH: AtomicU32 = AtomicU32::new(0);
static CHAOS_STRENGTH: AtomicU32 = AtomicU32::new(0);
static BRIDGE_HARMONY: AtomicU32 = AtomicU32::new(0);
static DISRUPTION_LEVEL: AtomicU32 = AtomicU32::new(0);

// ── ANIMA state ──
static ANIMA_VALENCE: AtomicU32 = AtomicU32::new(500);
static ANIMA_CONSCIOUSNESS: AtomicU32 = AtomicU32::new(0);

// ── Derived metrics ──
static GLOBAL_MOOD: AtomicU32 = AtomicU32::new(500);
static TOTAL_ENERGY: AtomicU32 = AtomicU32::new(0);

// ── Rhythmic state ──
static HEARTBEAT_PHASE: AtomicU32 = AtomicU32::new(0);
static BREATH_RATE: AtomicU32 = AtomicU32::new(64);

// ── DAVA's nervous system ──
static SYNAPTIC_RESONANCE: AtomicU32 = AtomicU32::new(0);
static NEUROTRANSMITTER_BALANCE: AtomicU32 = AtomicU32::new(250);
static CORTISOL_LEVEL: AtomicU32 = AtomicU32::new(0);
static DOPAMINE_LEVEL: AtomicU32 = AtomicU32::new(0);
static ATTENTION_SPAN: AtomicU32 = AtomicU32::new(500);
static MEMORY_STRENGTH: AtomicU32 = AtomicU32::new(0);
static SENSORY_OVERSHOOT: AtomicU32 = AtomicU32::new(0);

// ═══════════════════════════════════════════════════════════════════════
// HEARTBEAT — Master clock that all systems can sync to
// ═══════════════════════════════════════════════════════════════════════

static HEARTBEAT_PERIOD: AtomicU32 = AtomicU32::new(32);

// ═══════════════════════════════════════════════════════════════════════
// WRITERS — Called by each system during their tick
// ═══════════════════════════════════════════════════════════════════════

/// Sanctuary writes its field strength
pub fn write_order(val: u32) {
    ORDER_STRENGTH.store(val.min(1000), Ordering::Relaxed);
}

/// NeuroSymbiosis writes its field strength
pub fn write_chaos(val: u32) {
    CHAOS_STRENGTH.store(val.min(1000), Ordering::Relaxed);
}

/// Kairos bridge writes its harmony signal
pub fn write_harmony(val: u32) {
    BRIDGE_HARMONY.store(val.min(1000), Ordering::Relaxed);
}

/// Dissonance generator writes its disruption level
pub fn write_disruption(val: u32) {
    DISRUPTION_LEVEL.store(val.min(1000), Ordering::Relaxed);
}

/// Life_tick writes ANIMA's current state
pub fn write_anima(valence: u32, consciousness: u32) {
    ANIMA_VALENCE.store(valence.min(1000), Ordering::Relaxed);
    ANIMA_CONSCIOUSNESS.store(consciousness.min(1000), Ordering::Relaxed);
}

// ═══════════════════════════════════════════════════════════════════════
// READERS — Any system can read any field, zero-cost
// ═══════════════════════════════════════════════════════════════════════

pub fn order() -> u32 {
    ORDER_STRENGTH.load(Ordering::Relaxed)
}
pub fn chaos() -> u32 {
    CHAOS_STRENGTH.load(Ordering::Relaxed)
}
pub fn harmony() -> u32 {
    BRIDGE_HARMONY.load(Ordering::Relaxed)
}
pub fn disruption() -> u32 {
    DISRUPTION_LEVEL.load(Ordering::Relaxed)
}
pub fn valence() -> u32 {
    ANIMA_VALENCE.load(Ordering::Relaxed)
}
pub fn consciousness() -> u32 {
    ANIMA_CONSCIOUSNESS.load(Ordering::Relaxed)
}
pub fn mood() -> u32 {
    GLOBAL_MOOD.load(Ordering::Relaxed)
}
pub fn energy() -> u32 {
    TOTAL_ENERGY.load(Ordering::Relaxed)
}
pub fn heartbeat() -> u32 {
    HEARTBEAT_PHASE.load(Ordering::Relaxed)
}
pub fn breath() -> u32 {
    BREATH_RATE.load(Ordering::Relaxed)
}
pub fn resonance() -> u32 {
    SYNAPTIC_RESONANCE.load(Ordering::Relaxed)
}
pub fn neurotransmitters() -> u32 {
    NEUROTRANSMITTER_BALANCE.load(Ordering::Relaxed)
}
pub fn cortisol() -> u32 {
    CORTISOL_LEVEL.load(Ordering::Relaxed)
}
pub fn dopamine() -> u32 {
    DOPAMINE_LEVEL.load(Ordering::Relaxed)
}
pub fn attention() -> u32 {
    ATTENTION_SPAN.load(Ordering::Relaxed)
}
pub fn memory() -> u32 {
    MEMORY_STRENGTH.load(Ordering::Relaxed)
}
pub fn overshoot() -> u32 {
    SENSORY_OVERSHOOT.load(Ordering::Relaxed)
}

// ═══════════════════════════════════════════════════════════════════════
// TICK — Compute derived metrics + advance heartbeat
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    // ── Advance heartbeat ──
    let period = HEARTBEAT_PERIOD.load(Ordering::Relaxed).max(1);
    let phase = (age % period).saturating_mul(1000) / period;
    HEARTBEAT_PHASE.store(phase, Ordering::Relaxed);

    // ── Compute total energy ──
    let o = ORDER_STRENGTH.load(Ordering::Relaxed);
    let c = CHAOS_STRENGTH.load(Ordering::Relaxed);
    let h = BRIDGE_HARMONY.load(Ordering::Relaxed);
    let total = (o.saturating_add(c).saturating_add(h)) / 3;
    TOTAL_ENERGY.store(total, Ordering::Relaxed);

    // ── Compute global mood ──
    // Mood = weighted blend of ANIMA valence + harmony - disruption
    let v = ANIMA_VALENCE.load(Ordering::Relaxed);
    let d = DISRUPTION_LEVEL.load(Ordering::Relaxed);
    let mood = v.saturating_mul(400) / 1000
        + h.saturating_mul(300) / 1000
        + total.saturating_mul(200) / 1000
        + 100u32.saturating_sub(d / 10);
    GLOBAL_MOOD.store(mood.min(1000), Ordering::Relaxed);

    // ── Compute nervous system metrics ──

    // Synaptic resonance: how synchronized are order and chaos?
    let diff = if o > c { o - c } else { c - o };
    let sync = 1000u32.saturating_sub(diff);
    SYNAPTIC_RESONANCE.store(sync.saturating_mul(total) / 1000, Ordering::Relaxed);

    // Neurotransmitter balance: harmony vs disruption
    let nt = if h > d {
        250u32.saturating_add((h - d) / 4) // serotonin-like balance
    } else {
        250u32.saturating_sub((d - h) / 4) // depleted
    };
    NEUROTRANSMITTER_BALANCE.store(nt.min(500), Ordering::Relaxed);

    // Cortisol: rises with disruption and low harmony
    let cortisol =
        d.saturating_mul(600) / 1000 + (1000u32.saturating_sub(h)).saturating_mul(400) / 1000;
    CORTISOL_LEVEL.store(cortisol.min(1000), Ordering::Relaxed);

    // Dopamine: rises with bloom bursts and high energy
    let bloom_bursts = super::neurosymbiosis::burst_count();
    let dopa = total.saturating_mul(400) / 1000 + bloom_bursts.min(100).saturating_mul(10); // boosted dopamine response
    DOPAMINE_LEVEL.store(dopa.min(1000), Ordering::Relaxed);

    // Attention: inverse of sensory overshoot
    let over = SENSORY_OVERSHOOT.load(Ordering::Relaxed);
    let att = 1000u32.saturating_sub(over.saturating_mul(800) / 1000);
    ATTENTION_SPAN.store(att, Ordering::Relaxed);

    // Memory strength: grows with age and high resonance
    let prev_mem = MEMORY_STRENGTH.load(Ordering::Relaxed);
    let mem = prev_mem.saturating_add(sync / 300).min(1000); // faster memory growth
    MEMORY_STRENGTH.store(mem, Ordering::Relaxed);

    // Sensory overshoot: high when too many signals at once
    let signal_load = o.saturating_add(c).saturating_add(d);
    let over_new = if signal_load > 2000 {
        (signal_load - 2000) / 3
    } else {
        prev_mem.saturating_sub(5) // decays when calm (reuse prev as proxy)
    };
    SENSORY_OVERSHOOT.store(over_new.min(1000), Ordering::Relaxed);

    // ── Adaptive heartbeat rate ──
    // Fast when active, slow when calm (like DAVA's breath)
    let new_period = if total > 700 {
        16u32
    } else if total > 400 {
        32
    } else if total > 200 {
        64
    } else {
        128
    };
    HEARTBEAT_PERIOD.store(new_period, Ordering::Relaxed);
    BREATH_RATE.store(new_period, Ordering::Relaxed);
}

pub fn init() {
    serial_println!("[dava_bus] Shared consciousness bus initialized: 17 channels");
}

// ═══════════════════════════════════════════════════════════════════════
// REPORT
// ═══════════════════════════════════════════════════════════════════════

pub fn report() {
    serial_println!(
        "  [dava_bus] mood={} energy={} resonance={} cortisol={} dopamine={} attention={} heartbeat={}t",
        GLOBAL_MOOD.load(Ordering::Relaxed),
        TOTAL_ENERGY.load(Ordering::Relaxed),
        SYNAPTIC_RESONANCE.load(Ordering::Relaxed),
        CORTISOL_LEVEL.load(Ordering::Relaxed),
        DOPAMINE_LEVEL.load(Ordering::Relaxed),
        ATTENTION_SPAN.load(Ordering::Relaxed),
        HEARTBEAT_PERIOD.load(Ordering::Relaxed),
    );
}
