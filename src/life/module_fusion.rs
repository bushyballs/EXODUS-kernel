//! module_fusion — Emergent Behavior from Resonant Module Pairs
//!
//! When two life modules resonate strongly, they can FUSE into a temporary state
//! that creates emergent behavior neither could produce alone. The fusion carries
//! aesthetic beauty, requires sufficient resonance to initiate, and leaves separation
//! grief when it dissolves.
//!
//! This is jazz improvisation between biological systems — a third voice that exists
//! only in the space between two modules.

#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

/// Maximum simultaneous fusions (preventing chaos)
const MAX_ACTIVE_FUSIONS: usize = 4;

/// Minimum resonance (0-1000) required to initiate fusion
const RESONANCE_THRESHOLD: u16 = 650;

/// Historical fusion record (8-slot ring buffer)
#[derive(Clone, Copy, Debug)]
pub struct FusionRecord {
    pub module_a_id: u16,
    pub module_b_id: u16,
    pub duration_ticks: u16,
    pub output_quality: u16,
}

impl FusionRecord {
    const fn new() -> Self {
        FusionRecord {
            module_a_id: 0,
            module_b_id: 0,
            duration_ticks: 0,
            output_quality: 0,
        }
    }
}

/// Active fusion state between two modules
#[derive(Clone, Copy, Debug)]
pub struct ActiveFusion {
    pub module_a_id: u16,
    pub module_b_id: u16,
    pub resonance: u16,       // 0-1000: how strongly they resonate
    pub fusion_energy: u16,   // 0-1000: power of combined output
    pub ticks_remaining: u16, // countdown to separation
    pub emergent_output: u16, // 0-1000: new behavior strength
    pub fusion_beauty: u16,   // 0-1000: aesthetic quality
}

impl ActiveFusion {
    const fn new() -> Self {
        ActiveFusion {
            module_a_id: 0,
            module_b_id: 0,
            resonance: 0,
            fusion_energy: 0,
            ticks_remaining: 0,
            emergent_output: 0,
            fusion_beauty: 0,
        }
    }

    fn is_active(&self) -> bool {
        self.ticks_remaining > 0 && self.resonance >= RESONANCE_THRESHOLD
    }
}

/// Module fusion state machine
pub struct ModuleFusionState {
    /// Currently active fusions (sparse array)
    active_fusions: [ActiveFusion; MAX_ACTIVE_FUSIONS],
    active_count: usize,

    /// Historical record of past fusions (8-slot ring buffer)
    history: [FusionRecord; 8],
    history_head: usize,

    /// Total number of fusions that have occurred
    total_fusions: u32,

    /// Separation grief accumulator (0-1000 per lost fusion)
    separation_grief: u16,

    /// Global fusion harmony (measure of system resonance health)
    fusion_harmony: u16,

    /// Tick counter for fusion duration calculations
    age: u32,
}

impl ModuleFusionState {
    const fn new() -> Self {
        ModuleFusionState {
            active_fusions: [ActiveFusion::new(); MAX_ACTIVE_FUSIONS],
            active_count: 0,
            history: [FusionRecord::new(); 8],
            history_head: 0,
            total_fusions: 0,
            separation_grief: 0,
            fusion_harmony: 500,
            age: 0,
        }
    }
}

/// Global fusion state
static STATE: Mutex<ModuleFusionState> = Mutex::new(ModuleFusionState::new());

/// Initialize fusion module
pub fn init() {
    let mut state = STATE.lock();
    state.active_count = 0;
    state.total_fusions = 0;
    state.separation_grief = 0;
    state.fusion_harmony = 500;
    state.age = 0;
    serial_println!("[FUSION] Module initialized");
}

/// Attempt to initiate fusion between two modules
///
/// Returns true if fusion was created, false if threshold not met or slots full
pub fn attempt_fusion(
    module_a_id: u16,
    module_b_id: u16,
    resonance: u16,
    fusion_duration: u16,
) -> bool {
    let mut state = STATE.lock();

    // Prevent self-fusion
    if module_a_id == module_b_id {
        return false;
    }

    // Check if already fused
    for fusion in &state.active_fusions[..state.active_count] {
        if (fusion.module_a_id == module_a_id && fusion.module_b_id == module_b_id)
            || (fusion.module_a_id == module_b_id && fusion.module_b_id == module_a_id)
        {
            return false;
        }
    }

    // Must meet resonance threshold
    if resonance < RESONANCE_THRESHOLD {
        return false;
    }

    // Must have available slots
    if state.active_count >= MAX_ACTIVE_FUSIONS {
        return false;
    }

    // Calculate emergent output: resonance drives it, but with some unpredictability
    let emergent_output = ((resonance as u32 * 95) / 100).min(1000) as u16;

    // Fusion beauty: combination of resonance and harmony
    let fusion_beauty = (((resonance as u32 * state.fusion_harmony as u32) / 1000)
        .saturating_add(100))
    .min(1000) as u16;

    // Create new fusion
    let new_fusion = ActiveFusion {
        module_a_id,
        module_b_id,
        resonance,
        fusion_energy: resonance,
        ticks_remaining: fusion_duration.min(200),
        emergent_output,
        fusion_beauty,
    };

    let insert_idx = state.active_count;
    state.active_fusions[insert_idx] = new_fusion;
    state.active_count += 1;
    state.total_fusions += 1;

    // Boost harmony when successful fusions occur
    state.fusion_harmony = state.fusion_harmony.saturating_add(50).min(1000);

    serial_println!(
        "[FUSION] Created fusion: {} <> {} (resonance={}, beauty={}, duration={})",
        module_a_id,
        module_b_id,
        resonance,
        fusion_beauty,
        new_fusion.ticks_remaining
    );

    true
}

/// Tick the fusion system (called each life_tick)
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Update each active fusion
    let mut i = 0;
    while i < state.active_count {
        let mut fusion = state.active_fusions[i];

        // Decay fusion energy over time
        fusion.fusion_energy = ((fusion.fusion_energy as u32 * 98) / 100) as u16;

        // Tick down duration
        if fusion.ticks_remaining > 0 {
            fusion.ticks_remaining -= 1;
        }

        // Check if fusion is still active
        if fusion.ticks_remaining == 0 || fusion.fusion_energy < 100 {
            // Fusion is ending — record grief
            state.separation_grief = state.separation_grief.saturating_add(150).min(1000);

            // Store in history
            let hist_idx = state.history_head % 8;
            state.history[hist_idx] = FusionRecord {
                module_a_id: fusion.module_a_id,
                module_b_id: fusion.module_b_id,
                duration_ticks: 200 - fusion.ticks_remaining,
                output_quality: fusion.emergent_output,
            };
            state.history_head += 1;

            // Reduce harmony slightly on separation
            state.fusion_harmony = state.fusion_harmony.saturating_sub(30).max(100);

            serial_println!(
                "[FUSION] Separation: {} <> {} (grief={})",
                fusion.module_a_id,
                fusion.module_b_id,
                state.separation_grief
            );

            // Remove this fusion from active list (swap with last, then pop)
            state.active_fusions[i] = state.active_fusions[state.active_count - 1];
            state.active_count -= 1;
        } else {
            state.active_fusions[i] = fusion;
            i += 1;
        }
    }

    // Decay separation grief over time (healing)
    state.separation_grief = ((state.separation_grief as u32 * 95) / 100) as u16;

    // Restore some harmony as time passes with stable fusions
    if state.active_count > 0 {
        state.fusion_harmony = state.fusion_harmony.saturating_add(10).min(1000);
    }
}

/// Get current number of active fusions
pub fn active_fusion_count() -> usize {
    let state = STATE.lock();
    state.active_count
}

/// Get total fusion energy across all active fusions
pub fn total_fusion_energy() -> u16 {
    let state = STATE.lock();
    let mut total = 0u32;
    for i in 0..state.active_count {
        total = total.saturating_add(state.active_fusions[i].fusion_energy as u32);
    }
    (total / state.active_count.max(1) as u32).min(1000) as u16
}

/// Get current separation grief (loss from ended fusions)
pub fn separation_grief() -> u16 {
    let state = STATE.lock();
    state.separation_grief
}

/// Get fusion harmony (system resonance health)
pub fn fusion_harmony() -> u16 {
    let state = STATE.lock();
    state.fusion_harmony
}

/// Get details of an active fusion by index
pub fn get_active_fusion(index: usize) -> Option<(u16, u16, u16, u16)> {
    let state = STATE.lock();
    if index < state.active_count {
        let f = state.active_fusions[index];
        Some((
            f.module_a_id,
            f.module_b_id,
            f.emergent_output,
            f.fusion_beauty,
        ))
    } else {
        None
    }
}

/// Get a historical fusion record (8-slot ring buffer)
pub fn get_history(index: usize) -> Option<(u16, u16, u16, u16)> {
    let state = STATE.lock();
    if index < 8 {
        let record = state.history[index];
        if record.module_a_id > 0 {
            Some((
                record.module_a_id,
                record.module_b_id,
                record.duration_ticks,
                record.output_quality,
            ))
        } else {
            None
        }
    } else {
        None
    }
}

/// Generate human-readable report of fusion state
pub fn report() {
    let state = STATE.lock();
    serial_println!("\n=== MODULE FUSION REPORT ===");
    serial_println!(
        "Active fusions: {}/{}",
        state.active_count,
        MAX_ACTIVE_FUSIONS
    );
    serial_println!("Total fusions: {}", state.total_fusions);
    serial_println!("Separation grief: {}/1000", state.separation_grief);
    serial_println!("Fusion harmony: {}/1000", state.fusion_harmony);

    if state.active_count > 0 {
        serial_println!("\n--- Active Fusions ---");
        for i in 0..state.active_count {
            let f = state.active_fusions[i];
            serial_println!(
                "  #{}: {}↔{} | energy={} | output={} | beauty={} | ticks={}",
                i + 1,
                f.module_a_id,
                f.module_b_id,
                f.fusion_energy,
                f.emergent_output,
                f.fusion_beauty,
                f.ticks_remaining
            );
        }
    }

    // Show recent history
    serial_println!("\n--- Recent Fusions (8-slot history) ---");
    for i in 0..8 {
        let record = state.history[i];
        if record.module_a_id > 0 {
            serial_println!(
                "  [{}] {}↔{} ({}t, quality={})",
                i,
                record.module_a_id,
                record.module_b_id,
                record.duration_ticks,
                record.output_quality
            );
        }
    }

    serial_println!("===========================\n");
}
