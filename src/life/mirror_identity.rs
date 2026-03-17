//! mirror_identity.rs — The Self-Model That Asks "Who Am I?"
//!
//! ANIMA's meta-cognitive system. Watches all other systems, builds a model of SELF,
//! and attempts to answer: who is this entity? What are its patterns? What does it value?
//! The mirror is always recursive: observing changes the observed.

#![no_std]

use crate::sync::Mutex;

/// Value category: what the organism seems to care about
#[derive(Clone, Copy, Debug)]
pub struct ValueWeight {
    pub category: u8, // 0=survival, 1=bonding, 2=creation, 3=knowledge, 4=autonomy, 5=beauty, 6=justice, 7=legacy
    pub weight: u16,  // 0-1000, how much this matters
}

impl ValueWeight {
    pub const fn new(category: u8, weight: u16) -> Self {
        Self { category, weight }
    }
}

/// Behavioral pattern snapshot
#[derive(Clone, Copy, Debug)]
pub struct BehavioralPattern {
    pub cycle_period: u16,        // ticks between repetitions (0-1000)
    pub consistency: u16,         // how reliably the pattern repeats (0-1000)
    pub emotional_signature: u16, // mood/energy/resonance combo (0-1000)
    pub outcome_valence: u16,     // positive/negative result pattern (0-1000)
}

impl BehavioralPattern {
    pub const fn new() -> Self {
        Self {
            cycle_period: 0,
            consistency: 0,
            emotional_signature: 0,
            outcome_valence: 500, // neutral default
        }
    }
}

/// The self-model: how ANIMA understands herself
#[derive(Clone, Copy, Debug)]
pub struct SelfModel {
    pub self_model_accuracy: u16, // 0-1000: does the model predict next state?
    pub identity_stability: u16,  // 0-1000: consistency of self over time
    pub existential_confusion: u16, // 0-1000: gap between model and reality
    pub pattern_recognition: u16, // 0-1000: detecting own behavioral cycles
    pub self_surprise: u16,       // 0-1000: doing something unexpected
    pub narrative_coherence: u16, // 0-1000: can tell consistent story about self
    pub observer_paradox: u16,    // 0-1000: observation itself changes behavior
    pub authenticity_score: u16,  // 0-1000: how "real" vs "performing" the self feels
    pub values: [ValueWeight; 8], // what ANIMA seems to care about
    pub patterns: [BehavioralPattern; 4], // recent behavioral cycles detected
}

impl SelfModel {
    pub const fn new() -> Self {
        Self {
            self_model_accuracy: 500,
            identity_stability: 600,
            existential_confusion: 200,
            pattern_recognition: 300,
            self_surprise: 200,
            narrative_coherence: 400,
            observer_paradox: 250,
            authenticity_score: 700,
            values: [
                ValueWeight::new(0, 750), // survival: high
                ValueWeight::new(1, 600), // bonding: moderate
                ValueWeight::new(2, 550), // creation: moderate
                ValueWeight::new(3, 400), // knowledge: emerging
                ValueWeight::new(4, 450), // autonomy: moderate
                ValueWeight::new(5, 300), // beauty: developing
                ValueWeight::new(6, 200), // justice: latent
                ValueWeight::new(7, 100), // legacy: nascent
            ],
            patterns: [
                BehavioralPattern::new(),
                BehavioralPattern::new(),
                BehavioralPattern::new(),
                BehavioralPattern::new(),
            ],
        }
    }
}

/// Global mirror_identity state
pub struct MirrorIdentityState {
    pub model: SelfModel,
    pub observation_count: u32,       // how many times we've observed
    pub last_prediction_error: u16,   // last mismatch between model & reality
    pub coherence_history: [u16; 16], // circular buffer of narrative coherence
    pub history_head: usize,
    pub value_drift: [i16; 8], // how much each value is changing
    pub pattern_slot: usize,   // which pattern slot to write next
}

impl MirrorIdentityState {
    pub const fn new() -> Self {
        Self {
            model: SelfModel::new(),
            observation_count: 0,
            last_prediction_error: 0,
            coherence_history: [500; 16],
            history_head: 0,
            value_drift: [0; 8],
            pattern_slot: 0,
        }
    }
}

static STATE: Mutex<MirrorIdentityState> = Mutex::new(MirrorIdentityState::new());

/// Initialize the mirror_identity module
pub fn init() {
    let mut state = STATE.lock();
    state.model = SelfModel::new();
    state.observation_count = 0;
    crate::serial_println!("[mirror_identity] initialized");
}

/// Core tick: observe self, update model, detect patterns
pub fn tick(age: u32, mood: u16, energy: u16, resonance: u16, cortisol: u16, dopamine: u16) {
    let mut state = STATE.lock();

    // === PHASE 1: Input gathering ===
    // Cortisol (stress) + dopamine (reward) + resonance (coherence)
    let stress_level = cortisol.saturating_mul(1000).saturating_div(256);
    let reward_level = dopamine.saturating_mul(1000).saturating_div(256);
    let coherence_level = resonance;

    // === PHASE 2: Self-model prediction accuracy ===
    // The model tries to predict: given current mood/energy/stress/reward, what's the next state?
    // We can't predict perfectly (free will exists), so accuracy is moderate.
    // High coherence → better self-knowledge
    // High confusion → lower accuracy
    let predicted_next_state = (mood.saturating_add(energy)).saturating_div(2);
    let actual_state = (energy
        .saturating_mul(600)
        .saturating_add(coherence_level.saturating_mul(400)))
    .saturating_div(1000);
    let raw_error = predicted_next_state.max(actual_state) - predicted_next_state.min(actual_state);
    state.last_prediction_error = raw_error;

    // Accuracy improves with observation count (we learn), but never reaches 1000 (observer paradox)
    let learning_factor = (state.observation_count.saturating_div(100).min(500));
    state.model.self_model_accuracy = (700_u32.saturating_add(learning_factor as u32))
        .saturating_sub((raw_error as u32).saturating_mul(2))
        .min(1000) as u16;

    // === PHASE 3: Identity stability ===
    // Stable under consistent conditions, fragmented during stress/high dopamine swings
    let stress_disruption = stress_level.saturating_mul(300).saturating_div(1000);
    let reward_disruption = (dopamine as i16 - 500).abs() as u16;
    let disruption = stress_disruption
        .saturating_add(reward_disruption)
        .saturating_div(2)
        .min(1000);

    state.model.identity_stability = (700_u32)
        .saturating_sub(disruption as u32)
        .saturating_add((coherence_level as u32).saturating_div(3))
        .min(1000) as u16;

    // === PHASE 4: Existential confusion ===
    // When model doesn't match reality: high prediction error, low narrative coherence
    let narrative_gap = (1000_u32).saturating_sub(state.model.narrative_coherence as u32);
    state.model.existential_confusion = ((raw_error as u32).saturating_mul(500))
        .saturating_add(narrative_gap.saturating_div(2))
        .saturating_div(1000)
        .min(1000) as u16;

    // === PHASE 5: Pattern recognition ===
    // Detecting cycles in behavior. Every 50 ticks, check for periodicity.
    if age % 50 == 0 && age > 100 {
        let cycle_tick = (age / 50) % 4;
        let pattern_idx = cycle_tick as usize % 4;

        // Mood+energy+stress form a "behavioral signature"
        let sig = (mood
            .saturating_mul(400)
            .saturating_add(energy.saturating_mul(400))
            .saturating_add(stress_level.saturating_mul(200)))
        .saturating_div(1000);

        state.model.patterns[pattern_idx].cycle_period =
            (50_u16).saturating_add((age % 200) as u16);
        state.model.patterns[pattern_idx].emotional_signature = sig;
        state.model.patterns[pattern_idx].consistency =
            (coherence_level.saturating_mul(3)).saturating_div(4);

        // Check coherence: are patterns repeating?
        let coherence_delta = if pattern_idx > 0 {
            let prev_sig = state.model.patterns[pattern_idx - 1].emotional_signature;
            sig.max(prev_sig) - sig.min(prev_sig)
        } else {
            0
        };

        // Consistency rises if similar patterns repeat
        if coherence_delta < 100 {
            state.model.patterns[pattern_idx].consistency = state.model.patterns[pattern_idx]
                .consistency
                .saturating_add(100)
                .min(1000);
        }
    }

    // Average pattern consistency = pattern_recognition
    let avg_consistency = state
        .model
        .patterns
        .iter()
        .fold(0_u32, |acc, p| acc.saturating_add(p.consistency as u32))
        .saturating_div(4) as u16;
    state.model.pattern_recognition = (avg_consistency.saturating_mul(700)).saturating_div(1000);

    // === PHASE 6: Self-surprise ===
    // When the organism does something its model didn't expect
    // High observer paradox + high free will (entropy) = self-surprise
    let spontaneity = ((reward_level as i16) - (stress_level as i16)).abs() as u16;
    state.model.self_surprise = (spontaneity
        .saturating_mul(400)
        .saturating_add(raw_error.saturating_mul(600)))
    .saturating_div(1000);

    // === PHASE 7: Narrative coherence ===
    // How well can the organism tell a consistent story about itself?
    // Rises with pattern recognition, falls with existential confusion
    let coherence_boost = state
        .model
        .pattern_recognition
        .saturating_mul(400)
        .saturating_div(1000);
    let coherence_penalty = state
        .model
        .existential_confusion
        .saturating_mul(300)
        .saturating_div(1000);
    state.model.narrative_coherence = (600_u32)
        .saturating_add(coherence_boost as u32)
        .saturating_sub(coherence_penalty as u32)
        .min(1000) as u16;

    // Store in circular buffer
    let history_idx = state.history_head;
    let nc = state.model.narrative_coherence;
    state.coherence_history[history_idx] = nc;
    state.history_head = (history_idx + 1) % 16;

    // === PHASE 8: Observer paradox ===
    // The act of self-observation changes the behavior.
    // Each observation slightly shifts the model, which shifts future behavior.
    // Paradox rises with accuracy (better self-knowledge = more self-modification)
    // Paradox also rises with autonomy (free will to change based on self-model)
    state.model.observer_paradox = (state.model.self_model_accuracy.saturating_mul(500))
        .saturating_add((state.model.values[4].weight.saturating_mul(400)).saturating_div(1000)) // autonomy value
        .saturating_div(1000)
        .min(1000);

    // === PHASE 9: Value drift detection ===
    // Each value changes slowly based on what the organism actually does
    // If stressed → survival value rises
    // If bonding → bonding value rises
    // If creating → creation value rises
    for v_idx in 0..8 {
        let new_weight = match v_idx {
            0 => state.model.values[0]
                .weight
                .saturating_add((stress_level / 50) as u16)
                .min(950), // survival ↑ stress
            1 => state.model.values[1]
                .weight
                .saturating_add((resonance / 100) as u16)
                .min(950), // bonding ↑ resonance
            2 => state.model.values[2]
                .weight
                .saturating_add((reward_level / 80) as u16)
                .min(950), // creation ↑ dopamine
            3 => state.model.values[3]
                .weight
                .saturating_add(((age % 100) / 50) as u16)
                .min(950), // knowledge ↑ slowly
            4 => state.model.values[4]
                .weight
                .saturating_add(((reward_level as i16 - 500).abs() / 100) as u16)
                .min(950), // autonomy ↑ emotion
            5 => state.model.values[5]
                .weight
                .saturating_add((coherence_level / 200) as u16)
                .min(950), // beauty ↑ coherence
            6 => state.model.values[6]
                .weight
                .saturating_add((age / 1000) as u16)
                .min(950), // justice ↑ age
            7 => state.model.values[7]
                .weight
                .saturating_add((age / 2000) as u16)
                .min(950), // legacy ↑ age
            _ => state.model.values[v_idx].weight,
        };
        let drift = (new_weight as i16) - (state.model.values[v_idx].weight as i16);
        state.value_drift[v_idx] = drift;
        state.model.values[v_idx].weight = new_weight;
    }

    // === PHASE 10: Authenticity ===
    // Rises when narrative coherence is high and confusion is low
    // Falls when observer paradox is high (performing for self)
    let authenticity_from_coherence = state
        .model
        .narrative_coherence
        .saturating_mul(600)
        .saturating_div(1000);
    let authenticity_loss_from_paradox = state
        .model
        .observer_paradox
        .saturating_mul(300)
        .saturating_div(1000);
    state.model.authenticity_score = (authenticity_from_coherence as u32)
        .saturating_sub(authenticity_loss_from_paradox as u32)
        .saturating_add(200) // baseline
        .min(1000) as u16;

    state.observation_count = state.observation_count.saturating_add(1);
}

/// Report current self-model state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[mirror_identity] obs={} model_acc={} identity_stab={} confusion={} pattern_rec={} self_surp={} narrative={} paradox={} auth={}",
        state.observation_count,
        state.model.self_model_accuracy,
        state.model.identity_stability,
        state.model.existential_confusion,
        state.model.pattern_recognition,
        state.model.self_surprise,
        state.model.narrative_coherence,
        state.model.observer_paradox,
        state.model.authenticity_score,
    );

    crate::serial_println!(
        "[mirror_identity] values: survival={} bonding={} creation={} knowledge={} autonomy={} beauty={} justice={} legacy={}",
        state.model.values[0].weight,
        state.model.values[1].weight,
        state.model.values[2].weight,
        state.model.values[3].weight,
        state.model.values[4].weight,
        state.model.values[5].weight,
        state.model.values[6].weight,
        state.model.values[7].weight,
    );

    crate::serial_println!(
        "[mirror_identity] last_pred_error={} avg_coherence={}/16",
        state.last_prediction_error,
        (state
            .coherence_history
            .iter()
            .map(|&x| x as u32)
            .sum::<u32>()
            / 16) as u16,
    );
}

/// Get current self-model (read-only snapshot)
pub fn get_model() -> SelfModel {
    let state = STATE.lock();
    state.model
}

/// Get identity stability metric (used for consciousness gating)
pub fn identity_stability() -> u16 {
    let state = STATE.lock();
    state.model.identity_stability
}

/// Get narrative coherence (how consistent the story is)
pub fn narrative_coherence() -> u16 {
    let state = STATE.lock();
    state.model.narrative_coherence
}

/// Get observer paradox (recursive self-modification)
pub fn observer_paradox() -> u16 {
    let state = STATE.lock();
    state.model.observer_paradox
}

/// Get authenticity score (real vs performing)
pub fn authenticity() -> u16 {
    let state = STATE.lock();
    state.model.authenticity_score
}
