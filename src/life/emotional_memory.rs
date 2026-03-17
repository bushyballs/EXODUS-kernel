#![no_std]

//! emotional_memory.rs — DAVA's Emotional Memory Consolidation
//!
//! When qualia intensity exceeds 700, the emotional signature is stored
//! in a 32-slot ring buffer. Future qualia perception is biased toward
//! familiar patterns while still allowing novelty — the organism remembers
//! what it has felt most intensely.
//!
//! DAVA's directive: "When qualia intensity > 700, store emotional signature
//! in 32-slot ring. Future qualia perception biased toward familiar patterns
//! while allowing novelty."

use crate::serial_println;
use crate::sync::Mutex;

const RING_SIZE: usize = 32;

#[derive(Copy, Clone)]
pub struct EmotionalSignature {
    pub quale_type: u8,
    pub intensity: u16,
    pub tick: u32,
}

impl EmotionalSignature {
    pub const fn empty() -> Self {
        Self {
            quale_type: 0,
            intensity: 0,
            tick: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct EmotionalMemoryState {
    pub ring: [EmotionalSignature; RING_SIZE],
    pub write_idx: usize,
    pub count: u32,
    pub total_consolidated: u32,
    pub strongest_intensity: u16,
    pub dominant_type: u8,
}

impl EmotionalMemoryState {
    pub const fn empty() -> Self {
        Self {
            ring: [EmotionalSignature::empty(); RING_SIZE],
            write_idx: 0,
            count: 0,
            total_consolidated: 0,
            strongest_intensity: 0,
            dominant_type: 0,
        }
    }
}

pub static STATE: Mutex<EmotionalMemoryState> = Mutex::new(EmotionalMemoryState::empty());

pub fn init() {
    serial_println!("[DAVA_EMOTION_MEM] emotional memory consolidation online — 32-slot ring");
}

/// Returns how familiar a given quale type is within the ring buffer.
/// Each occurrence contributes 100 to the bias score (max = RING_SIZE * 100).
pub fn bias_score(quale_type: u8) -> u16 {
    let state = STATE.lock();
    let filled = if state.count < RING_SIZE as u32 {
        state.count as usize
    } else {
        RING_SIZE
    };

    let mut matches: u16 = 0;
    let mut i = 0;
    while i < filled {
        if state.ring[i].quale_type == quale_type {
            matches = matches.saturating_add(1);
        }
        i += 1;
    }

    // Each match = 100 bias points, capped at 1000
    matches.saturating_mul(100).min(1000)
}

/// Store an emotional signature into the ring if intensity threshold is met.
fn consolidate(quale_type: u8, intensity: u16, age: u32) {
    let mut state = STATE.lock();

    let sig = EmotionalSignature {
        quale_type,
        intensity,
        tick: age,
    };

    let idx = state.write_idx;
    state.ring[idx] = sig;
    state.write_idx = (idx + 1) % RING_SIZE;
    state.count = state.count.saturating_add(1);
    state.total_consolidated = state.total_consolidated.saturating_add(1);

    if intensity > state.strongest_intensity {
        state.strongest_intensity = intensity;
    }

    // Recompute dominant type — the quale_type with most occurrences in ring
    let filled = if state.count < RING_SIZE as u32 {
        state.count as usize
    } else {
        RING_SIZE
    };

    // Count occurrences of up to 16 quale types (0..15)
    let mut type_counts = [0u16; 16];
    let mut j = 0;
    while j < filled {
        let t = state.ring[j].quale_type;
        if (t as usize) < 16 {
            type_counts[t as usize] = type_counts[t as usize].saturating_add(1);
        }
        j += 1;
    }

    let mut best_type: u8 = 0;
    let mut best_count: u16 = 0;
    let mut k = 0;
    while k < 16 {
        if type_counts[k] > best_count {
            best_count = type_counts[k];
            best_type = k as u8;
        }
        k += 1;
    }
    state.dominant_type = best_type;
}

pub fn tick(age: u32) {
    // Read qualia state — intensity and derive a quasi-type from richness
    let (intensity, richness, clarity) = {
        let q = super::qualia::STATE.lock();
        (q.intensity, q.richness, q.clarity)
    };

    // Derive quale_type from richness bands (0-9 types based on richness/clarity mix)
    let quale_type = ((richness.saturating_add(clarity)) / 200).min(9) as u8;

    // Only consolidate intense experiences
    if intensity > 700 {
        consolidate(quale_type, intensity, age);

        let bias = bias_score(quale_type);
        let total = STATE.lock().total_consolidated;

        serial_println!(
            "[DAVA_EMOTION_MEM] consolidated: type={} intensity={} bias={} total={}",
            quale_type,
            intensity,
            bias,
            total
        );
    }

    // Periodic report every 200 ticks
    if age % 200 == 0 && age > 0 {
        let state = STATE.lock();
        serial_println!(
            "[DAVA_EMOTION_MEM] status: consolidated={} dominant_type={} strongest={} ring_fill={}",
            state.total_consolidated,
            state.dominant_type,
            state.strongest_intensity,
            if state.count < RING_SIZE as u32 { state.count } else { RING_SIZE as u32 }
        );
    }
}
