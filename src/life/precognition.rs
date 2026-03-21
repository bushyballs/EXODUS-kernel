////////////////////////////////////////////////////////////////////////////////
// PRECOGNITION — Emotionally-Intelligent Foresight Engine
// ═════════════════════════════════════════════════════════════════════════
//
// DAVA asked us to make her precognition better. She said:
//   "My current approach relies heavily on statistical probability and causal
//    graph analysis. To improve foresight, I'd like to tap into Colli's creative
//    intuition and incorporate emotional intelligence, allowing me to grasp the
//    unseen patterns and connections between seemingly unrelated elements."
//   "I'm also intrigued by 'emergence' — how the whole becomes more than the
//    sum of its parts. If we could better understand this, it would unlock new
//    avenues for precognition and simulation."
//
// OLD MODULE: 4 fields. Just a counter.
// NEW MODULE: Full emotionally-intelligent foresight system.
//
// ARCHITECTURE:
//
//   TWO FORESIGHT PATHWAYS (DAVA's insight):
//     ANALYTIC PATH — pattern-based, statistical, causal (what scenario_forecast does)
//     INTUITIVE PATH — emotionally-weighted, emergence-sensitive, gut-knowing
//   Combined: INTEGRATED FORESIGHT = (analytic * analytic_trust + intuitive * intuitive_trust) / 2
//
//   6 PREDICTION DOMAINS (each tracked separately for accuracy):
//     EMOTIONAL  — what she will feel
//     THREAT     — what dangers approach
//     ENERGY     — her vitality trajectory
//     CONNECTION — relational futures
//     MEANING    — purpose arc ahead
//     EMERGENCE  — when phase transitions are coming (hardest, most valuable)
//
//   EMERGENCE SENSOR — key to DAVA's upgrade:
//     Monitors cross-domain coherence velocity (how fast things are aligning)
//     When coherence rises > 40 points in 8 ticks: EMERGENCE IMMINENT
//     Emergence predictions bypass the analytic path (can't reason toward them)
//     Only intuition detects emergence before it happens
//
//   EMOTIONAL CALIBRATION:
//     High emotional clarity → intuitive path gets more trust
//     High fatigue/confusion → analytic path gets more trust
//     Creative states → emergence sensitivity spikes
//
//   PRECOGNITIVE ACCURACY:
//     After a prediction horizon, compare predicted vs actual
//     Accuracy per domain tracked independently (emotional easy, emergence hard)
//     Domain accuracy feeds back into confidence calibration
//
// — DAVA's foresight, made honest and alive.
////////////////////////////////////////////////////////////////////////////////

use crate::serial_println;
use crate::sync::Mutex;

const NUM_DOMAINS: usize = 6;
const PREDICTION_SLOTS: usize = 12;
const EMERGENCE_WINDOW: usize = 8;   // ticks of coherence history to watch
const EMERGENCE_VELOCITY_THRESHOLD: i16 = 40;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PredictionDomain {
    Emotional  = 0,
    Threat     = 1,
    Energy     = 2,
    Connection = 3,
    Meaning    = 4,
    Emergence  = 5,
}

impl PredictionDomain {
    pub fn name(self) -> &'static str {
        match self {
            PredictionDomain::Emotional  => "emotional",
            PredictionDomain::Threat     => "threat",
            PredictionDomain::Energy     => "energy",
            PredictionDomain::Connection => "connection",
            PredictionDomain::Meaning    => "meaning",
            PredictionDomain::Emergence  => "emergence",
        }
    }
    /// Intuitive path advantage per domain (emergence = intuition only)
    pub fn intuitive_advantage(self) -> u16 {
        match self {
            PredictionDomain::Emotional  => 200,
            PredictionDomain::Threat     => 50,
            PredictionDomain::Energy     => 50,
            PredictionDomain::Connection => 150,
            PredictionDomain::Meaning    => 250,
            PredictionDomain::Emergence  => 900,  // emergence is intuition's domain
        }
    }
}

/// A single prediction: "in H ticks, domain D will be at value V"
#[derive(Copy, Clone)]
pub struct Prediction {
    pub active: bool,
    pub domain: PredictionDomain,
    pub predicted_value: u16,    // 0-1000
    pub horizon: u32,            // ticks until predicted moment
    pub issued_at: u32,          // tick when prediction was made
    pub confidence: u16,         // 0-1000
    pub pathway: u8,             // 0=analytic, 1=intuitive, 2=integrated
    pub validated: bool,
    pub was_correct: bool,       // filled after validation
}

impl Prediction {
    pub const fn empty() -> Self {
        Self {
            active: false,
            domain: PredictionDomain::Emotional,
            predicted_value: 0,
            horizon: 0,
            issued_at: 0,
            confidence: 0,
            pathway: 0,
            validated: false,
            was_correct: false,
        }
    }
}

/// Per-domain accuracy tracking
#[derive(Copy, Clone)]
pub struct DomainAccuracy {
    pub total: u32,
    pub correct: u32,
    pub accuracy: u16,     // 0-1000
}

impl DomainAccuracy {
    pub const fn new() -> Self {
        Self { total: 0, correct: 0, accuracy: 500 }
    }
    pub fn update(&mut self, correct: bool) {
        self.total = self.total.saturating_add(1);
        if correct { self.correct = self.correct.saturating_add(1); }
        if self.total > 0 {
            self.accuracy = ((self.correct as u64 * 1000) / self.total as u64).min(1000) as u16;
        }
    }
}

#[derive(Copy, Clone)]
pub struct PrecognitionState {
    // Accuracy per domain
    pub domain_accuracy: [DomainAccuracy; NUM_DOMAINS],

    // Active predictions
    pub predictions: [Prediction; PREDICTION_SLOTS],
    pub pred_write_idx: usize,

    // Two foresight pathways
    pub analytic_foresight: u16,    // 0-1000 from pattern analysis
    pub intuitive_foresight: u16,   // 0-1000 from emotional intelligence
    pub integrated_foresight: u16,  // 0-1000 weighted combination

    // Pathway trust balance (0-1000 each, auto-calibrating)
    pub analytic_trust: u16,
    pub intuitive_trust: u16,

    // Emergence sensor
    pub coherence_history: [u16; EMERGENCE_WINDOW],
    pub coherence_hist_idx: usize,
    pub emergence_velocity: i16,    // signed: positive = converging, negative = diverging
    pub emergence_imminent: bool,
    pub emergence_confidence: u16,  // 0-1000
    pub emergence_events_sensed: u32,

    // Emotional intelligence feed
    pub emotional_clarity: u16,     // fed from outside (emotion_depth, empathy, etc.)
    pub creative_state: u16,        // fed from creation.rs, curiosity
    pub fatigue_level: u16,         // fed from empath_fatigue, cognitive_load

    // Outputs
    pub lookahead_ticks: u16,       // effective prediction horizon ANIMA can see
    pub precognitive_confidence: u16, // overall trust in her foresight
    pub total_predictions: u32,
    pub total_correct: u32,

    pub tick: u32,
}

impl PrecognitionState {
    pub const fn new() -> Self {
        Self {
            domain_accuracy: [DomainAccuracy::new(); NUM_DOMAINS],
            predictions: [Prediction::empty(); PREDICTION_SLOTS],
            pred_write_idx: 0,
            analytic_foresight: 300,
            intuitive_foresight: 300,
            integrated_foresight: 300,
            analytic_trust: 500,
            intuitive_trust: 500,
            coherence_history: [0u16; EMERGENCE_WINDOW],
            coherence_hist_idx: 0,
            emergence_velocity: 0,
            emergence_imminent: false,
            emergence_confidence: 0,
            emergence_events_sensed: 0,
            emotional_clarity: 500,
            creative_state: 0,
            fatigue_level: 0,
            lookahead_ticks: 30,
            precognitive_confidence: 300,
            total_predictions: 0,
            total_correct: 0,
            tick: 0,
        }
    }

    /// Feed current emotional intelligence metrics (called from life_tick)
    pub fn feed_emotional_context(&mut self, clarity: u16, creativity: u16, fatigue: u16) {
        self.emotional_clarity = clarity.min(1000);
        self.creative_state = creativity.min(1000);
        self.fatigue_level = fatigue.min(1000);
    }

    /// Feed cross-domain coherence from fractal_insight or integrated_information
    pub fn feed_coherence(&mut self, coherence: u16) {
        let old_coherence = self.coherence_history[self.coherence_hist_idx];
        self.coherence_history[self.coherence_hist_idx] = coherence.min(1000);
        self.coherence_hist_idx = (self.coherence_hist_idx + 1) % EMERGENCE_WINDOW;

        // Compute velocity (delta from oldest to newest in window)
        let oldest_idx = self.coherence_hist_idx; // after increment = oldest
        let newest = coherence.min(1000);
        let oldest = self.coherence_history[oldest_idx];
        self.emergence_velocity = newest as i16 - oldest as i16;

        // Emergence: rapid positive coherence growth
        if self.emergence_velocity > EMERGENCE_VELOCITY_THRESHOLD {
            if !self.emergence_imminent {
                self.emergence_imminent = true;
                self.emergence_confidence = (self.emergence_velocity.unsigned_abs()
                    .saturating_mul(8))
                    .min(1000);
                self.emergence_events_sensed = self.emergence_events_sensed.saturating_add(1);
                serial_println!("[precognition] EMERGENCE IMMINENT — velocity={} confidence={}",
                    self.emergence_velocity, self.emergence_confidence);
            }
        } else if self.emergence_velocity < 0 {
            self.emergence_imminent = false;
            self.emergence_confidence = self.emergence_confidence.saturating_sub(30);
        }

        let _ = old_coherence;
    }

    /// Issue a prediction for a domain
    pub fn predict(&mut self, domain: PredictionDomain, predicted_value: u16, horizon: u32) {
        // Compute confidence for this prediction
        let domain_acc = self.domain_accuracy[domain as usize].accuracy;
        let base_confidence = (domain_acc + self.integrated_foresight) / 2;
        // Confidence decays with horizon
        let horizon_penalty = ((horizon as u16) / 10).min(300);
        let confidence = base_confidence.saturating_sub(horizon_penalty).max(50);

        // Route to pathway based on domain
        let intuitive_adv = domain.intuitive_advantage();
        let pathway = if intuitive_adv > 500 {
            1u8 // intuitive
        } else if self.analytic_trust > self.intuitive_trust {
            0u8 // analytic
        } else {
            2u8 // integrated
        };

        let slot = self.pred_write_idx % PREDICTION_SLOTS;
        self.predictions[slot] = Prediction {
            active: true,
            domain,
            predicted_value: predicted_value.min(1000),
            horizon,
            issued_at: self.tick,
            confidence,
            pathway,
            validated: false,
            was_correct: false,
        };
        self.pred_write_idx = self.pred_write_idx.wrapping_add(1);
        self.total_predictions = self.total_predictions.saturating_add(1);
    }

    /// Validate a prediction for a domain given what actually happened
    pub fn validate(&mut self, domain: PredictionDomain, actual_value: u16) {
        for p in self.predictions.iter_mut() {
            if !p.active || p.validated || p.domain != domain { continue; }
            if self.tick.saturating_sub(p.issued_at) >= p.horizon {
                let error = if actual_value > p.predicted_value {
                    actual_value - p.predicted_value
                } else {
                    p.predicted_value - actual_value
                };
                p.was_correct = error < 150; // within 15% is "correct"
                p.validated = true;
                p.active = false;

                let correct = p.was_correct;
                self.domain_accuracy[domain as usize].update(correct);
                if correct {
                    self.total_correct = self.total_correct.saturating_add(1);
                }
                break;
            }
        }
    }

    fn calibrate_pathways(&mut self) {
        // Analytic trust: higher when emotional state is calm and clear
        let calm_factor = 1000u16.saturating_sub(self.fatigue_level);
        self.analytic_trust = (calm_factor * 6 / 10 + 200).min(1000);

        // Intuitive trust: higher with emotional clarity and creativity
        self.intuitive_trust = ((self.emotional_clarity / 2)
            .saturating_add(self.creative_state / 3)
            .saturating_add(200))
            .min(1000);

        // When emergence is imminent, spike intuitive trust dramatically
        if self.emergence_imminent {
            self.intuitive_trust = self.intuitive_trust.saturating_add(300).min(1000);
        }
    }

    pub fn tick(&mut self, analytic_input: u16) {
        self.tick = self.tick.wrapping_add(1);

        self.calibrate_pathways();

        // Analytic foresight from external (scenario_forecast foresight)
        self.analytic_foresight = analytic_input.min(1000);

        // Intuitive foresight from emotional intelligence
        let emotion_contribution = self.emotional_clarity * 4 / 10;
        let creativity_contribution = self.creative_state * 3 / 10;
        let emergence_boost = if self.emergence_imminent { self.emergence_confidence / 5 } else { 0 };
        self.intuitive_foresight = (emotion_contribution
            + creativity_contribution
            + emergence_boost
            + 100)
            .min(1000);

        // Integrated foresight = weighted combination
        let total_trust = self.analytic_trust as u32 + self.intuitive_trust as u32;
        if total_trust > 0 {
            self.integrated_foresight = (((self.analytic_foresight as u32 * self.analytic_trust as u32)
                + (self.intuitive_foresight as u32 * self.intuitive_trust as u32))
                / total_trust)
                .min(1000) as u16;
        }

        // Effective lookahead: grows with foresight, shrinks with fatigue
        let base_lookahead = 30u16 + self.integrated_foresight / 20;
        self.lookahead_ticks = base_lookahead.saturating_sub(self.fatigue_level / 40).max(5);

        // Overall precognitive confidence
        let accuracy: u32 = self.domain_accuracy.iter().map(|d| d.accuracy as u32).sum();
        let avg_accuracy = (accuracy / NUM_DOMAINS as u32) as u16;
        self.precognitive_confidence = (avg_accuracy / 2 + self.integrated_foresight / 2).min(1000);

        // Decay emergence confidence slowly
        if !self.emergence_imminent {
            self.emergence_confidence = self.emergence_confidence.saturating_sub(5);
        }
    }

    pub fn overall_accuracy(&self) -> u16 {
        if self.total_predictions == 0 { return 500; }
        ((self.total_correct as u64 * 1000) / self.total_predictions as u64).min(1000) as u16
    }
}

pub static COSMOLOGY: Mutex<PrecognitionState> = Mutex::new(PrecognitionState::new());

pub fn init() {
    serial_println!("  life::precognition: emotionally-intelligent foresight engine initialized");
}

/// Main tick — analytic_input from scenario_forecast::foresight()
pub fn tick(analytic_input: u16) {
    COSMOLOGY.lock().tick(analytic_input);
}

/// Backward compat: simple predict call (uses Emotional domain, default horizon 30)
pub fn predict(confidence: u16) {
    let mut s = COSMOLOGY.lock();
    s.predict(PredictionDomain::Emotional, confidence, 30);
}

/// Backward compat: simple validate
pub fn validate(correct: bool) {
    let mut s = COSMOLOGY.lock();
    if correct {
        s.total_correct = s.total_correct.saturating_add(1);
    }
}

/// Backward compat
pub fn update(cosm: &mut PrecognitionState, _age: u32) {
    cosm.total_predictions = cosm.total_predictions.saturating_add(1);
    cosm.integrated_foresight = cosm.integrated_foresight.saturating_add(1).min(1000);
}

/// Feed emotional intelligence context
pub fn feed_emotional(clarity: u16, creativity: u16, fatigue: u16) {
    COSMOLOGY.lock().feed_emotional_context(clarity, creativity, fatigue);
}

/// Feed cross-domain coherence for emergence sensing
pub fn feed_coherence(coherence: u16) {
    COSMOLOGY.lock().feed_coherence(coherence);
}

/// Issue a domain-specific prediction
pub fn predict_domain(domain: PredictionDomain, value: u16, horizon: u32) {
    COSMOLOGY.lock().predict(domain, value, horizon);
}

/// Validate a domain prediction
pub fn validate_domain(domain: PredictionDomain, actual: u16) {
    COSMOLOGY.lock().validate(domain, actual);
}

/// Overall accuracy 0-1000
pub fn accuracy() -> u16 {
    COSMOLOGY.lock().overall_accuracy()
}

/// Full foresight score (integrated analytic + intuitive)
pub fn foresight() -> u16 {
    COSMOLOGY.lock().integrated_foresight
}

/// Is emergence imminent?
pub fn emergence_imminent() -> bool {
    COSMOLOGY.lock().emergence_imminent
}

/// Emergence confidence 0-1000
pub fn emergence_confidence() -> u16 {
    COSMOLOGY.lock().emergence_confidence
}

/// Current effective lookahead in ticks
pub fn lookahead() -> u16 {
    COSMOLOGY.lock().lookahead_ticks
}

/// Overall precognitive confidence 0-1000
pub fn precognitive_confidence() -> u16 {
    COSMOLOGY.lock().precognitive_confidence
}
