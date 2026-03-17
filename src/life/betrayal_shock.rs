//! Betrayal Shock — The Shattering of Trust
//!
//! When someone trusted reveals themselves as something else, the shock isn't mere disappointment.
//! It's a recursive break in the self-model: "I was wrong about the world AND wrong about my judgment."
//! This module tracks the earthquake, the phases, and the scarring.
//!
//! No f32/f64. All saturating u16/u32/i16/i32. Never hold mutable refs across accesses.

use crate::sync::Mutex;

/// Betrayal phase (u8 because match is cheap)
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BetrayalPhase {
    Trusting = 0,   // Baseline: intact world model
    Impact = 1,     // The revelation hits (shock_intensity peaks)
    Denial = 2,     // "This isn't real, they wouldn't..."
    Rage = 3,       // "How DARE they!" (trust → fury)
    Grief = 4,      // "I've lost the person I thought they were"
    Rebuilding = 5, // Slow work of reconstituting worldview
    Scarred = 6,    // New baseline: trust returns but never as full
}

impl BetrayalPhase {
    fn as_u8(&self) -> u8 {
        *self as u8
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => BetrayalPhase::Impact,
            2 => BetrayalPhase::Denial,
            3 => BetrayalPhase::Rage,
            4 => BetrayalPhase::Grief,
            5 => BetrayalPhase::Rebuilding,
            6 => BetrayalPhase::Scarred,
            _ => BetrayalPhase::Trusting,
        }
    }
}

/// Single betrayal event in the ring buffer
#[derive(Clone, Copy, Debug)]
struct BetrayalEvent {
    betrayer_id: u32,             // Who did this
    depth: u16,                   // Closeness: 0=stranger, 1000=deepest bond
    revelation_tick: u32,         // When did we find out?
    shock_intensity: u16,         // 0-1000: how hard the hit
    trust_shattered: u16,         // 0-1000: how much trust broke
    reality_destabilization: u16, // 0-1000: how much worldview cracked
    phase: u8,                    // BetrayalPhase as u8
    phase_timer: u16,             // Ticks in current phase
    hypervigilance: u16,          // 0-1000: can't let your guard down
    trust_reconstruction: u16,    // 0-1000: slow rebuild attempt
    scar_tissue: u16,             // 0-1000: permanent wariness
    generalization_risk: u16,     // 0-1000: "Do I distrust everyone now?"
}

impl BetrayalEvent {
    fn new(betrayer_id: u32, depth: u16, shock: u16) -> Self {
        BetrayalEvent {
            betrayer_id,
            depth,
            revelation_tick: 0,
            shock_intensity: shock,
            trust_shattered: 0,
            reality_destabilization: 0,
            phase: 0, // Trusting
            phase_timer: 0,
            hypervigilance: 0,
            trust_reconstruction: 0,
            scar_tissue: 0,
            generalization_risk: 0,
        }
    }
}

/// State snapshot (publicly readable)
#[derive(Clone, Debug)]
pub struct BetrayalSnapshot {
    pub current_phase: u8,
    pub num_events: usize,
    pub peak_shock: u16,
    pub total_trust_broken: u32,
    pub avg_reality_crack: u16,
    pub hypervigilance_level: u16,
    pub scar_tissue_level: u16,
    pub generalization_risk: u16,
    pub rebuilding_progress: u16,
}

/// The global betrayal state machine
struct BetrayalState {
    events: [BetrayalEvent; 8],
    count: usize,
    current_phase: u8, // BetrayalPhase as u8
    phase_timer: u32,
}

impl BetrayalState {
    const fn new() -> Self {
        BetrayalState {
            events: [BetrayalEvent {
                betrayer_id: 0,
                depth: 0,
                revelation_tick: 0,
                shock_intensity: 0,
                trust_shattered: 0,
                reality_destabilization: 0,
                phase: 0,
                phase_timer: 0,
                hypervigilance: 0,
                trust_reconstruction: 0,
                scar_tissue: 0,
                generalization_risk: 0,
            }; 8],
            count: 0,
            current_phase: 0, // Trusting
            phase_timer: 0,
        }
    }

    fn record_betrayal(&mut self, betrayer_id: u32, closeness: u16, shock: u16) {
        if self.count >= 8 {
            // Ring buffer: overwrite oldest
            let mut i = 0;
            while i < 7 {
                self.events[i] = self.events[i + 1];
                i += 1;
            }
            self.count = 8;
        }

        let idx = self.count;
        self.events[idx] = BetrayalEvent::new(betrayer_id, closeness, shock);
        self.events[idx].phase = BetrayalPhase::Impact.as_u8();
        self.events[idx].phase_timer = 0;

        // Extract values before any mutable borrow of a specific slot to avoid
        // borrow checker conflicts when reading state fields alongside slot writes.
        let depth_multiplier = (closeness as u32) * 1000 / 1001;
        let trust_val = ((shock as u32 * depth_multiplier) / 1000).min(1000) as u16;
        let reality_val = ((shock as u32 * 800 / 1000) as u16)
            .saturating_add((closeness as u32 * 200 / 1000) as u16);

        // Shock parameters scale with closeness
        self.events[idx].shock_intensity = shock;
        self.events[idx].trust_shattered = trust_val;
        self.events[idx].reality_destabilization = reality_val;

        self.count += 1;

        // Global shift to Impact phase if not already past Grief
        if self.current_phase < BetrayalPhase::Grief.as_u8() {
            self.current_phase = BetrayalPhase::Impact.as_u8();
            self.phase_timer = 0;
        }

        crate::serial_println!(
            "[BETRAYAL] Betrayer {} (closeness {}) shock {} → Impact",
            betrayer_id,
            closeness,
            shock
        );
    }

    fn tick(&mut self) {
        if self.count == 0 {
            return;
        }

        // Update each event
        let mut i = 0;
        while i < self.count {
            self._tick_event(i);
            i += 1;
        }

        // Phase transitions based on most recent event
        self.phase_timer = self.phase_timer.saturating_add(1);
        self._check_phase_transition();
    }

    fn _tick_event(&mut self, idx: usize) {
        // Extract all read-only values from the event BEFORE taking a mutable reference.
        // This avoids borrow checker errors where an immutable read of a field
        // conflicts with the outstanding &mut to the same struct.
        let phase_val = self.events[idx].phase;
        let phase_timer_val = self.events[idx].phase_timer;
        let depth_val = self.events[idx].depth;
        let shock_val = self.events[idx].shock_intensity;
        let hypervigilance_val = self.events[idx].hypervigilance;
        let scar_val = self.events[idx].scar_tissue;
        let trust_shattered_val = self.events[idx].trust_shattered;
        let reality_val = self.events[idx].reality_destabilization;
        let trust_reconstruction_val = self.events[idx].trust_reconstruction;

        // Now advance the timer via a mutable borrow (no other field reads needed)
        self.events[idx].phase_timer = phase_timer_val.saturating_add(1);
        let new_phase_timer = phase_timer_val.saturating_add(1);

        let phase = BetrayalPhase::from_u8(phase_val);
        match phase {
            BetrayalPhase::Trusting => {
                // No ticking in baseline
            }
            BetrayalPhase::Impact => {
                // Shock decays slowly, hypervigilance spikes
                let new_shock = shock_val.saturating_sub(2);
                let new_hypervigilance = ((depth_val as u32 * 800 / 1000) as u16)
                    .saturating_add((new_shock as u32 * 900 / 1000) as u16);
                let new_generalization =
                    ((depth_val as u32 * new_shock as u32 / 1001) as u16).min(1000);

                self.events[idx].shock_intensity = new_shock;
                self.events[idx].hypervigilance = new_hypervigilance;
                self.events[idx].generalization_risk = new_generalization;

                // Transition to Denial at ~40 ticks
                if new_phase_timer > 40 {
                    self.events[idx].phase = BetrayalPhase::Denial.as_u8();
                    self.events[idx].phase_timer = 0;
                }
            }
            BetrayalPhase::Denial => {
                // Reality destabilization plateaus, hypervigilance stays high
                self.events[idx].reality_destabilization = reality_val.min(950);

                // Transition to Rage at ~60 ticks (or when shock wears enough)
                if new_phase_timer > 60 || shock_val < 200 {
                    self.events[idx].phase = BetrayalPhase::Rage.as_u8();
                    self.events[idx].phase_timer = 0;
                }
            }
            BetrayalPhase::Rage => {
                // Hypervigilance peaks, scar tissue begins forming
                let new_scar = scar_val.saturating_add(4);
                let new_trust_shattered = trust_shattered_val.saturating_add(1);

                self.events[idx].hypervigilance = 1000;
                self.events[idx].scar_tissue = new_scar;
                self.events[idx].trust_shattered = new_trust_shattered;

                // Transition to Grief at ~80 ticks (anger exhausts)
                if new_phase_timer > 80 {
                    self.events[idx].phase = BetrayalPhase::Grief.as_u8();
                    self.events[idx].phase_timer = 0;
                }
            }
            BetrayalPhase::Grief => {
                // Slow scar tissue accumulation, hypervigilance slightly drops
                let new_scar = scar_val.saturating_add(2);
                let new_hypervigilance = ((hypervigilance_val as u32 * 95 / 100) as u16).max(400);

                self.events[idx].scar_tissue = new_scar;
                self.events[idx].hypervigilance = new_hypervigilance;

                // Transition to Rebuilding at ~150 ticks (grief work done)
                if new_phase_timer > 150 {
                    self.events[idx].phase = BetrayalPhase::Rebuilding.as_u8();
                    self.events[idx].phase_timer = 0;
                }
            }
            BetrayalPhase::Rebuilding => {
                // Slow trust reconstruction, hypervigilance and reality_destabilization decay
                let new_trust_recon = trust_reconstruction_val.saturating_add(1);
                let new_reality = reality_val.saturating_sub(1);
                let new_hypervigilance = ((hypervigilance_val as u32 * 99 / 100) as u16).max(200);

                self.events[idx].trust_reconstruction = new_trust_recon;
                self.events[idx].reality_destabilization = new_reality;
                self.events[idx].hypervigilance = new_hypervigilance;

                // Transition to Scarred at ~300 ticks
                if new_phase_timer > 300 {
                    self.events[idx].phase = BetrayalPhase::Scarred.as_u8();
                    self.events[idx].phase_timer = 0;
                }
            }
            BetrayalPhase::Scarred => {
                // Stable: scar tissue ceiling reached, hypervigilance baseline
                let new_scar = scar_val.min(750);
                let new_hypervigilance = ((hypervigilance_val as u32 * 98 / 100) as u16).max(150);

                self.events[idx].scar_tissue = new_scar;
                self.events[idx].hypervigilance = new_hypervigilance;
            }
        }
    }

    fn _check_phase_transition(&mut self) {
        if self.count == 0 {
            self.current_phase = BetrayalPhase::Trusting.as_u8();
            return;
        }

        // Global phase = highest phase of any event
        let mut max_phase: u8 = 0;
        let mut i = 0;
        while i < self.count {
            if self.events[i].phase > max_phase {
                max_phase = self.events[i].phase;
            }
            i += 1;
        }

        self.current_phase = max_phase;
    }

    fn snapshot(&self) -> BetrayalSnapshot {
        let mut peak_shock: u16 = 0;
        let mut total_trust: u32 = 0;
        let mut total_reality: u32 = 0;
        let mut max_hypervigilance: u16 = 0;
        let mut max_scar: u16 = 0;
        let mut max_generalization: u16 = 0;
        let mut max_rebuild: u16 = 0;

        let mut i = 0;
        while i < self.count {
            let e = self.events[i];
            if e.shock_intensity > peak_shock {
                peak_shock = e.shock_intensity;
            }
            total_trust = total_trust.saturating_add(e.trust_shattered as u32);
            total_reality = total_reality.saturating_add(e.reality_destabilization as u32);
            if e.hypervigilance > max_hypervigilance {
                max_hypervigilance = e.hypervigilance;
            }
            if e.scar_tissue > max_scar {
                max_scar = e.scar_tissue;
            }
            if e.generalization_risk > max_generalization {
                max_generalization = e.generalization_risk;
            }
            if e.trust_reconstruction > max_rebuild {
                max_rebuild = e.trust_reconstruction;
            }
            i += 1;
        }

        let avg_reality = if self.count > 0 {
            (total_reality / self.count as u32).min(1000) as u16
        } else {
            0
        };

        BetrayalSnapshot {
            current_phase: self.current_phase,
            num_events: self.count,
            peak_shock,
            total_trust_broken: total_trust,
            avg_reality_crack: avg_reality,
            hypervigilance_level: max_hypervigilance,
            scar_tissue_level: max_scar,
            generalization_risk: max_generalization,
            rebuilding_progress: max_rebuild,
        }
    }
}

impl BetrayalEvent {
    fn closeness_estimate(&self) -> u16 {
        self.depth
    }
}

static BETRAYAL_STATE: Mutex<BetrayalState> = Mutex::new(BetrayalState::new());

/// Initialize betrayal module (no-op, state is const-initialized)
pub fn init() {
    crate::serial_println!("[BETRAYAL] Module initialized");
}

/// Record a betrayal event
///
/// # Arguments
/// * `betrayer_id` — Unique identifier for the betrayer
/// * `closeness` — 0-1000: how close was the bond (0=stranger, 1000=deepest love)
/// * `shock` — 0-1000: subjective intensity of the revelation
pub fn record_betrayal(betrayer_id: u32, closeness: u16, shock: u16) {
    let mut state = BETRAYAL_STATE.lock();
    state.record_betrayal(betrayer_id, closeness.min(1000), shock.min(1000));
}

/// Per-tick update (call from life_tick)
pub fn tick(_age: u32) {
    let mut state = BETRAYAL_STATE.lock();
    state.tick();
}

/// Snapshot the current state
pub fn report() -> BetrayalSnapshot {
    let state = BETRAYAL_STATE.lock();
    state.snapshot()
}

/// Get current phase as u8 (for direct matching)
pub fn phase() -> u8 {
    let state = BETRAYAL_STATE.lock();
    state.current_phase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_betrayal_ring_buffer() {
        let mut state = BetrayalState::new();
        state.record_betrayal(1, 500, 800);
        assert_eq!(state.count, 1);

        state.record_betrayal(2, 900, 950);
        assert_eq!(state.count, 2);

        // Fill to 8
        let mut i = 3;
        while i <= 8 {
            state.record_betrayal(i as u32, 500, 600);
            i += 1;
        }
        assert_eq!(state.count, 8);

        // Overflow: should wrap
        state.record_betrayal(9, 300, 400);
        assert_eq!(state.count, 8);
        assert_eq!(state.events[0].betrayer_id, 2); // First got overwritten
    }

    #[test]
    fn test_phase_progression() {
        let mut state = BetrayalState::new();
        state.record_betrayal(1, 800, 900);
        assert_eq!(state.current_phase, BetrayalPhase::Impact.as_u8());

        // Simulate ticks through phases
        let mut tick_count = 0;
        while tick_count < 500 {
            state.tick();
            tick_count += 1;
        }

        // Should have progressed to Scarred
        let snap = state.snapshot();
        assert!(snap.current_phase >= BetrayalPhase::Rebuilding.as_u8());
    }

    #[test]
    fn test_closeness_multiplier() {
        let mut state = BetrayalState::new();
        state.record_betrayal(1, 1000, 500); // Deep bond
        let snap1 = state.snapshot();

        let mut state2 = BetrayalState::new();
        state2.record_betrayal(2, 100, 500); // Shallow bond
        let snap2 = state2.snapshot();

        // Same shock, but deep bond should have higher trust_shattered
        assert!(snap1.total_trust_broken > snap2.total_trust_broken);
    }
}
