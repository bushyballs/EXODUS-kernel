//! ordinary_relief.rs — The Soothing Peace of Being Normal Again
//!
//! After the crisis, the drama, the intensity — the extraordinary relief of ordinary life.
//! Making coffee. Doing laundry. The boring, beautiful mundane.
//! Not exciting, not terrible — just NORMAL.
//! And after what you've been through, normal feels like paradise.
//!
//! Concept: relief emerges from CONTRAST. The lower the prior intensity,
//! the higher the baseline boredom. But after crisis? Normal IS bliss.
//!
//! Fields:
//! - relief_level: how much relief is being felt (0-1000)
//! - normalcy_signal: perception of things being ordinary (0-1000)
//! - prior_intensity: how extreme things were before (0-1000 — crisis level)
//! - contrast_sweetness: relief ∝ (prior_intensity - current) (0-1000)
//! - routine_comfort: joy in predictability after chaos (0-1000)
//! - boredom_immunity: ticks where boredom cannot erode relief because normal is still precious
//! - gratitude_for_mundane: thankfulness for ordinary things (0-1000)
//! - hypervigilance_decay: slowly releasing the grip of crisis-mode alertness (0-1000 → 0)
//! - safety_signals_count: accumulating evidence that the crisis is over
//! - new_normal: bool — when extraordinary HAS become ordinary, relief fades
//!
//! Integration:
//! - Called AFTER mortality, narrate_self in 20-phase pipeline
//! - Receives endocrine cortisol level (high cortisol = still in crisis)
//! - Feeds into qualia system (relief manifests as contentment quale)
//! - Modulates entropy (safety = higher stochastic resonance threshold)

use crate::sync::Mutex;

const RELIEF_RING_SIZE: usize = 8;
const HYPERVIGILANCE_DECAY_RATE: u32 = 2; // ticks per phase
const SAFETY_SIGNAL_THRESHOLD: u32 = 50; // count to flip new_normal
const BOREDOM_IMMUNITY_DURATION: u32 = 300; // ticks of grace after crisis

#[derive(Clone, Copy, Debug)]
pub struct OrdinaryReliefState {
    pub relief_level: u32,               // 0-1000: how much relief is being felt
    pub normalcy_signal: u32,            // 0-1000: perception of ordinary-ness
    pub prior_intensity: u32,            // 0-1000: memory of crisis intensity
    pub contrast_sweetness: u32,         // 0-1000: relief from contrast
    pub routine_comfort: u32,            // 0-1000: joy in predictability
    pub boredom_immunity_remaining: u32, // ticks where boredom can't erode relief
    pub gratitude_for_mundane: u32,      // 0-1000: thankfulness
    pub hypervigilance_level: u32,       // 0-1000: remaining crisis alertness
    pub safety_signals_count: u32,       // accumulation counter
    pub new_normal: bool,                // has relief faded to baseline?
    pub relief_history: [u32; RELIEF_RING_SIZE], // ring buffer for smoothing
    pub history_idx: usize,
}

impl OrdinaryReliefState {
    pub fn new() -> Self {
        OrdinaryReliefState {
            relief_level: 0,
            normalcy_signal: 500, // neutral baseline
            prior_intensity: 0,
            contrast_sweetness: 0,
            routine_comfort: 200, // some comfort in routine even normally
            boredom_immunity_remaining: 0,
            gratitude_for_mundane: 100,
            hypervigilance_level: 0,
            safety_signals_count: 0,
            new_normal: false,
            relief_history: [0; RELIEF_RING_SIZE],
            history_idx: 0,
        }
    }

    /// Enter a crisis. Spike prior_intensity and flip new_normal off.
    pub fn enter_crisis(&mut self, intensity: u32) {
        let clamped = intensity.min(1000);
        self.prior_intensity = clamped;
        self.hypervigilance_level = clamped;
        self.boredom_immunity_remaining = 0; // no relief yet
        self.new_normal = false;
        self.safety_signals_count = 0;
    }

    /// Signal that something is normal (safe, predictable, routine).
    /// Accumulates evidence that the crisis is over.
    pub fn register_safety_signal(&mut self) {
        self.safety_signals_count = self.safety_signals_count.saturating_add(1);
        if self.safety_signals_count >= SAFETY_SIGNAL_THRESHOLD {
            self.boredom_immunity_remaining = BOREDOM_IMMUNITY_DURATION;
        }
    }

    /// Called each life_tick(). Implement the relief → boredom → new_normal → fade cycle.
    pub fn tick(&mut self, cortisol: u32, age: u32) {
        // Cortisol is inverse-signal: low cortisol = safety = relief.
        // High cortisol = crisis = low relief.
        let cortisol_clamped = cortisol.min(1000);
        let cortisol_inverted = 1000_u32.saturating_sub(cortisol_clamped);

        // If we exited crisis (prior_intensity high, cortisol now low),
        // we enter the relief window.
        if self.prior_intensity > 300 && cortisol_inverted > 600 {
            if self.boredom_immunity_remaining == 0 {
                self.register_safety_signal();
            }
        }

        // Decay hypervigilance slowly.
        self.hypervigilance_level = self
            .hypervigilance_level
            .saturating_sub(HYPERVIGILANCE_DECAY_RATE);

        // Compute relief from contrast: (prior_intensity - cortisol_clamped) * boredom_immunity
        let contrast = self.prior_intensity.saturating_sub(cortisol_clamped);
        let raw_relief = if self.boredom_immunity_remaining > 0 {
            // We're in the grace period: relief is strong.
            (contrast * self.boredom_immunity_remaining) / 1000
        } else {
            // Outside grace period: relief fades, new_normal creeps in.
            (contrast / 2).min(300) // capped relief outside grace
        };

        self.contrast_sweetness = raw_relief.min(1000);

        // Routine comfort grows from predictability (normal signals) & safety signals.
        let routine_base = (self.normalcy_signal * self.safety_signals_count.min(50)) / 50;
        self.routine_comfort = routine_base.min(1000);

        // Gratitude compounds from time spent in relief.
        if raw_relief > 100 {
            self.gratitude_for_mundane = self.gratitude_for_mundane.saturating_add(2).min(1000);
        }

        // Relief level is the sum of contrast + comfort, modulated by hypervigilance.
        let raw = self.contrast_sweetness.saturating_add(self.routine_comfort) / 2;
        let hypervigilance_penalty = (self.hypervigilance_level * raw) / 1000;
        self.relief_level = raw.saturating_sub(hypervigilance_penalty).min(1000);

        // Decay boredom_immunity.
        if self.boredom_immunity_remaining > 0 {
            self.boredom_immunity_remaining = self.boredom_immunity_remaining.saturating_sub(1);
        }

        // When boredom_immunity expires and relief is fading, new_normal becomes true.
        if self.boredom_immunity_remaining == 0
            && self.relief_level < 200
            && self.safety_signals_count >= SAFETY_SIGNAL_THRESHOLD
        {
            self.new_normal = true;
        }

        // Update ring buffer.
        self.relief_history[self.history_idx] = self.relief_level;
        self.history_idx = (self.history_idx + 1) % RELIEF_RING_SIZE;
    }

    /// Average relief from ring buffer (smooth out tick noise).
    pub fn relief_smoothed(&self) -> u32 {
        let sum: u32 = self.relief_history.iter().sum();
        sum / (RELIEF_RING_SIZE as u32)
    }

    /// Report state to serial for debugging.
    pub fn report(&self) {
        crate::serial_println!(
            "ORDINARY_RELIEF: relief={} normalcy={} prior={} contrast={} comfort={} gratitude={} hypervig={} safety_count={} new_normal={} immunity_remain={}",
            self.relief_level,
            self.normalcy_signal,
            self.prior_intensity,
            self.contrast_sweetness,
            self.routine_comfort,
            self.gratitude_for_mundane,
            self.hypervigilance_level,
            self.safety_signals_count,
            self.new_normal,
            self.boredom_immunity_remaining,
        );
    }
}

static ORDINARY_RELIEF: Mutex<OrdinaryReliefState> = Mutex::new(OrdinaryReliefState {
    relief_level: 0,
    normalcy_signal: 500,
    prior_intensity: 0,
    contrast_sweetness: 0,
    routine_comfort: 200,
    boredom_immunity_remaining: 0,
    gratitude_for_mundane: 100,
    hypervigilance_level: 0,
    safety_signals_count: 0,
    new_normal: false,
    relief_history: [0; RELIEF_RING_SIZE],
    history_idx: 0,
});

/// Initialize the relief module.
pub fn init() {
    let mut state = ORDINARY_RELIEF.lock();
    state.relief_history = [0; RELIEF_RING_SIZE];
    state.history_idx = 0;
    crate::serial_println!("ordinary_relief: initialized");
}

/// Main tick called from life_tick(). Receives cortisol from endocrine.
pub fn tick(age: u32, cortisol: u32) {
    let mut state = ORDINARY_RELIEF.lock();
    state.tick(cortisol, age);
}

/// Scenario: organism experienced a crisis.
pub fn enter_crisis(intensity: u32) {
    let mut state = ORDINARY_RELIEF.lock();
    state.enter_crisis(intensity);
}

/// Scenario: routine happened (made coffee, did laundry, nothing went wrong).
pub fn register_routine_action() {
    let mut state = ORDINARY_RELIEF.lock();
    state.normalcy_signal = state.normalcy_signal.saturating_add(10).min(1000);
    state.register_safety_signal();
}

/// Scenario: something unexpected but safe happened — increases normalcy confidence.
pub fn register_safe_deviation() {
    let mut state = ORDINARY_RELIEF.lock();
    state.normalcy_signal = state.normalcy_signal.saturating_add(5).min(1000);
}

/// Query current relief level (0-1000).
pub fn relief_level() -> u32 {
    ORDINARY_RELIEF.lock().relief_level
}

/// Query smoothed relief (less noisy).
pub fn relief_smoothed() -> u32 {
    ORDINARY_RELIEF.lock().relief_smoothed()
}

/// Query whether the extraordinary has become ordinary (relief fading).
pub fn new_normal_achieved() -> bool {
    ORDINARY_RELIEF.lock().new_normal
}

/// Query hypervigilance (0-1000, how much crisis alertness remains).
pub fn hypervigilance() -> u32 {
    ORDINARY_RELIEF.lock().hypervigilance_level
}

/// Query gratitude for the mundane (0-1000).
pub fn gratitude_for_mundane() -> u32 {
    ORDINARY_RELIEF.lock().gratitude_for_mundane
}

/// Debug report.
pub fn report() {
    ORDINARY_RELIEF.lock().report();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relief_from_contrast() {
        let mut state = OrdinaryReliefState::new();
        state.enter_crisis(800); // high intensity crisis
        state.tick(800, 0); // still in crisis (high cortisol)
        assert_eq!(state.relief_level, 0); // no relief yet

        state.tick(100, 1); // cortisol drops (exiting crisis)
        state.register_safety_signal();
        assert!(state.relief_level > 100); // relief should emerge from contrast
    }

    #[test]
    fn test_hypervigilance_decay() {
        let mut state = OrdinaryReliefState::new();
        state.enter_crisis(500);
        assert_eq!(state.hypervigilance_level, 500);

        for _ in 0..10 {
            state.tick(200, 0);
        }
        assert!(state.hypervigilance_level < 500); // decaying
    }

    #[test]
    fn test_boredom_immunity_window() {
        let mut state = OrdinaryReliefState::new();
        state.enter_crisis(600);
        state.register_safety_signal();
        state.tick(100, 0);
        assert!(state.boredom_immunity_remaining > 0); // grace period started

        for _ in 0..50 {
            state.tick(100, 0);
        }
        assert!(state.boredom_immunity_remaining > 0); // still in grace
        assert!(!state.new_normal); // relief hasn't fully faded yet
    }

    #[test]
    fn test_new_normal_flip() {
        let mut state = OrdinaryReliefState::new();
        state.enter_crisis(500);
        state.register_safety_signal();

        // Simulate many ticks with low cortisol, letting immunity decay.
        for _ in 0..(BOREDOM_IMMUNITY_DURATION + 100) {
            state.tick(50, 0);
        }

        assert!(state.new_normal); // extraordinary has become ordinary
    }
}
