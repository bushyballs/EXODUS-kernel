use crate::sync::Mutex;
/// Cortex — the central brain of HoagsOS Genesis Neural Bus
///
/// Processes all signals flowing through the bus, detects patterns,
/// classifies user intent, makes predictions, drives Hebbian learning.
///
/// Pattern detection: sequential trigger windows (length 3-5)
/// Intent classification: weighted scoring across 10 intent categories
/// Prediction: confidence-gated with accuracy tracking
/// Context embedding: 64-dimensional EMA-updated vector
///
/// All Q16 fixed-point math. Zero floats. Pure integer AI.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

use super::{
    q16_from_int, q16_mul, NeuralSignal, SignalKind, SubsystemClass, BUS, Q16, Q16_HALF, Q16_ONE,
    Q16_TENTH, Q16_ZERO,
};

// ── Constants ───────────────────────────────────────────────────────

const EMBED_DIM: usize = 64;
const MAX_PATTERNS: usize = 256;
const MAX_CANDIDATES: usize = 512;
const MAX_RECENT: usize = 128;
const PATTERN_TRIGGER_LEN: usize = 3;
const PATTERN_BIRTH_THRESHOLD: u32 = 3;
const DRAIN_BATCH: usize = 64;
const NUM_CLASSES: usize = 12;
const NUM_SIGNAL_KINDS: usize = 28;
const MIN_CONFIDENCE: Q16 = Q16_TENTH;
const INITIAL_LR: Q16 = 655; // 0.01
const DECAY_RATE: Q16 = 64880; // 0.99
const ACCURACY_RETAIN: Q16 = 62259; // 0.95
const ACCURACY_BLEND: Q16 = 3277; // 0.05
const EMA_RETAIN: Q16 = 58982; // 0.90
const EMA_BLEND: Q16 = 6554; // 0.10

// ── User Intent ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIntent {
    Idle,
    Browsing,
    Working,
    Gaming,
    Creating,
    Communicating,
    Searching,
    Learning,
    Configuring,
    Unknown,
}

impl UserIntent {
    pub fn index(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Browsing => 1,
            Self::Working => 2,
            Self::Gaming => 3,
            Self::Creating => 4,
            Self::Communicating => 5,
            Self::Searching => 6,
            Self::Learning => 7,
            Self::Configuring => 8,
            Self::Unknown => 9,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Idle,
            1 => Self::Browsing,
            2 => Self::Working,
            3 => Self::Gaming,
            4 => Self::Creating,
            5 => Self::Communicating,
            6 => Self::Searching,
            7 => Self::Learning,
            8 => Self::Configuring,
            _ => Self::Unknown,
        }
    }
}

// ── Pattern Types ───────────────────────────────────────────────────

/// A learned sequential pattern: trigger sequence → predicted next signal
pub struct CortexPattern {
    pub trigger_signals: Vec<SignalKind>,
    pub predicted_next: SignalKind,
    pub confidence: Q16,
    pub occurrences: u32,
    pub last_seen: u64,
    pub created_at: u64,
}

impl CortexPattern {
    pub fn new(triggers: Vec<SignalKind>, predicted: SignalKind, cycle: u64) -> Self {
        CortexPattern {
            trigger_signals: triggers,
            predicted_next: predicted,
            confidence: Q16_HALF,
            occurrences: 1,
            last_seen: cycle,
            created_at: cycle,
        }
    }

    pub fn reinforce(&mut self, cycle: u64) {
        self.occurrences = self.occurrences.saturating_add(1);
        self.last_seen = cycle;
        // Increase confidence: conf += (1.0 - conf) * 0.1
        let gap = Q16_ONE - self.confidence;
        self.confidence += q16_mul(gap, Q16_TENTH);
        if self.confidence > Q16_ONE {
            self.confidence = Q16_ONE;
        }
    }

    pub fn apply_decay(&mut self) {
        self.confidence = q16_mul(self.confidence, DECAY_RATE);
    }
}

/// Candidate pattern waiting for promotion
pub struct PatternCandidate {
    pub triggers: Vec<SignalKind>,
    pub predicted: SignalKind,
    pub occurrences: u32,
    pub first_seen: u64,
}

// ── Cortex Engine ───────────────────────────────────────────────────

/// The central brain of Genesis
pub struct CortexEngine {
    pub cycle_count: u64,
    pub learning_rate: Q16,
    pub prediction_accuracy: Q16,
    pub user_intent: UserIntent,

    // Attention: one weight per SubsystemClass
    pub attention_weights: [Q16; NUM_CLASSES],

    // Pattern memory
    pub pattern_memory: Vec<CortexPattern>,
    pub candidates: Vec<PatternCandidate>,

    // Recent signal history (ring of SignalKinds)
    pub recent_signals: Vec<SignalKind>,

    // Per-signal-kind counters for intent classification
    pub kind_counts: [u32; NUM_SIGNAL_KINDS],

    // Context embedding (64-dim Q16 vector, EMA-updated)
    pub context_embedding: Vec<Q16>,

    // Prediction tracking
    pub last_prediction: Option<(SignalKind, Q16)>,
    pub total_predictions: u64,
    pub total_correct: u64,
    pub total_patterns_learned: u64,

    initialized: bool,
}

impl CortexEngine {
    pub const fn new() -> Self {
        CortexEngine {
            cycle_count: 0,
            learning_rate: INITIAL_LR,
            prediction_accuracy: Q16_HALF,
            user_intent: UserIntent::Unknown,
            attention_weights: [Q16_ONE; NUM_CLASSES],
            pattern_memory: Vec::new(),
            candidates: Vec::new(),
            recent_signals: Vec::new(),
            kind_counts: [0; NUM_SIGNAL_KINDS],
            context_embedding: Vec::new(),
            last_prediction: None,
            total_predictions: 0,
            total_correct: 0,
            total_patterns_learned: 0,
            initialized: false,
        }
    }

    fn ensure_init(&mut self) {
        if !self.initialized {
            self.context_embedding = alloc::vec![Q16_ZERO; EMBED_DIM];
            self.initialized = true;
        }
    }

    // ── Signal Kind Indexing ─────────────────────────────────────────

    fn signal_kind_index(kind: SignalKind) -> usize {
        match kind {
            SignalKind::AppLaunch => 0,
            SignalKind::AppSwitch => 1,
            SignalKind::AppClose => 2,
            SignalKind::TouchEvent => 3,
            SignalKind::GestureDetected => 4,
            SignalKind::TextInput => 5,
            SignalKind::VoiceCommand => 6,
            SignalKind::SearchQuery => 7,
            SignalKind::CpuLoad => 8,
            SignalKind::MemoryPressure => 9,
            SignalKind::DiskIo => 10,
            SignalKind::NetworkTraffic => 11,
            SignalKind::BatteryDrain => 12,
            SignalKind::ThermalEvent => 13,
            SignalKind::ProcessSpawn => 14,
            SignalKind::ProcessExit => 15,
            SignalKind::PredictedAction => 16,
            SignalKind::PreloadHint => 17,
            SignalKind::LayoutMorph => 18,
            SignalKind::ColorAdapt => 19,
            SignalKind::ShortcutReady => 20,
            SignalKind::AnomalyAlert => 21,
            SignalKind::LearningUpdate => 22,
            SignalKind::ContextShift => 23,
            SignalKind::NodeSync => 24,
            SignalKind::NodeQuery => 25,
            SignalKind::NodeResponse => 26,
            SignalKind::Heartbeat => 27,
        }
    }

    fn class_index(class: SubsystemClass) -> usize {
        match class {
            SubsystemClass::Kernel => 0,
            SubsystemClass::Hardware => 1,
            SubsystemClass::Storage => 2,
            SubsystemClass::Network => 3,
            SubsystemClass::Security => 4,
            SubsystemClass::Display => 5,
            SubsystemClass::Input => 6,
            SubsystemClass::AI => 7,
            SubsystemClass::Application => 8,
            SubsystemClass::User => 9,
            SubsystemClass::Communication => 10,
            SubsystemClass::System => 11,
        }
    }

    // ── Embedding ───────────────────────────────────────────────────

    fn signal_to_embedding(kind: SignalKind, source: u16, strength: Q16) -> Vec<Q16> {
        let mut embed = alloc::vec![Q16_ZERO; EMBED_DIM];
        let ki = Self::signal_kind_index(kind);
        // Spread signal kind across embedding dimensions
        if ki < EMBED_DIM {
            embed[ki] = strength;
        }
        // Source node influence on higher dimensions
        let si = (source as usize) % (EMBED_DIM / 2);
        embed[EMBED_DIM / 2 + si] = q16_mul(strength, Q16_HALF);
        embed
    }

    fn similarity(a: &[Q16], b: &[Q16]) -> Q16 {
        let len = a.len().min(b.len());
        if len == 0 {
            return Q16_ZERO;
        }
        let mut dot: i64 = 0;
        let mut mag_a: i64 = 0;
        let mut mag_b: i64 = 0;
        for i in 0..len {
            dot += a[i] as i64 * b[i] as i64;
            mag_a += a[i] as i64 * a[i] as i64;
            mag_b += b[i] as i64 * b[i] as i64;
        }
        let denom = isqrt64(mag_a) * isqrt64(mag_b);
        if denom == 0 {
            return Q16_ZERO;
        }
        ((dot * Q16_ONE as i64) / denom) as Q16
    }

    // ── Batch Processing ────────────────────────────────────────────

    pub fn process_batch(&mut self, signals: &[NeuralSignal]) {
        self.ensure_init();
        if signals.is_empty() {
            return;
        }

        // Reset kind_counts
        for c in self.kind_counts.iter_mut() {
            *c = 0;
        }

        let mut source_nodes: Vec<u16> = Vec::new();

        // Step 1: Aggregate signals
        for sig in signals.iter() {
            let ki = Self::signal_kind_index(sig.kind);
            if ki < NUM_SIGNAL_KINDS {
                self.kind_counts[ki] = self.kind_counts[ki].saturating_add(1);
            }
            // Record recent signal kinds
            self.recent_signals.push(sig.kind);
            if self.recent_signals.len() > MAX_RECENT {
                self.recent_signals.remove(0);
            }
            // Track source nodes for Hebbian learning
            if !source_nodes.contains(&sig.source_node) {
                source_nodes.push(sig.source_node);
            }
        }

        // Step 2: Update attention weights based on class activity
        {
            let bus = BUS.lock();
            for i in 0..NUM_CLASSES {
                let activity = bus.class_activity[i];
                let weight = if activity > 100 {
                    Q16_ONE + Q16_HALF // highly active class gets 1.5x attention
                } else if activity > 10 {
                    Q16_ONE + Q16_TENTH // moderately active
                } else {
                    Q16_ONE // baseline
                };
                // EMA toward target
                self.attention_weights[i] = q16_mul(self.attention_weights[i], ACCURACY_RETAIN)
                    + q16_mul(weight, ACCURACY_BLEND);
            }
        }

        // Step 3: Check last prediction accuracy
        if let Some((predicted_kind, _conf)) = self.last_prediction.take() {
            let hit = signals.iter().any(|s| s.kind == predicted_kind);
            let outcome: Q16 = if hit { Q16_ONE } else { Q16_ZERO };
            self.prediction_accuracy = q16_mul(self.prediction_accuracy, ACCURACY_RETAIN)
                + q16_mul(outcome, ACCURACY_BLEND);
            if hit {
                self.total_correct = self.total_correct.saturating_add(1);
            }
        }

        // Step 4: Classify user intent
        self.user_intent = self.classify_intent();

        // Step 5: Hebbian learning on co-active nodes
        if source_nodes.len() >= 2 {
            for i in 0..source_nodes.len() {
                for j in (i + 1)..source_nodes.len() {
                    BUS.lock()
                        .hebbian_update(source_nodes[i], source_nodes[j], Q16_HALF);
                }
            }
        }

        // Step 6: Update context embedding
        self.update_context_embedding(signals);
    }

    // ── Pattern Detection ───────────────────────────────────────────

    pub fn detect_patterns(&mut self) {
        let rlen = self.recent_signals.len();
        if rlen < PATTERN_TRIGGER_LEN + 1 {
            return;
        }

        let trigger_start = rlen - PATTERN_TRIGGER_LEN - 1;
        let trigger_end = rlen - 1;
        let trigger_window: Vec<SignalKind> =
            self.recent_signals[trigger_start..trigger_end].to_vec();
        let actual_next = self.recent_signals[rlen - 1];

        // Check existing patterns
        let mut matched = false;
        let cycle = self.cycle_count;

        for pat in self.pattern_memory.iter_mut() {
            if pat.trigger_signals == trigger_window {
                if pat.predicted_next == actual_next {
                    pat.reinforce(cycle);
                    matched = true;
                } else {
                    pat.confidence = (pat.confidence - INITIAL_LR).max(Q16_ZERO);
                }
            }
        }

        // Update candidates if no match
        if !matched {
            let mut found = false;
            for cand in self.candidates.iter_mut() {
                if cand.triggers == trigger_window && cand.predicted == actual_next {
                    cand.occurrences = cand.occurrences.saturating_add(1);
                    found = true;
                    break;
                }
            }
            if !found {
                self.candidates.push(PatternCandidate {
                    triggers: trigger_window.clone(),
                    predicted: actual_next,
                    occurrences: 1,
                    first_seen: cycle,
                });
                if self.candidates.len() > MAX_CANDIDATES {
                    self.candidates.remove(0);
                }
            }
        }

        // Promote candidates with enough occurrences
        let mut promoted: Vec<usize> = Vec::new();
        for (i, cand) in self.candidates.iter().enumerate() {
            if cand.occurrences >= PATTERN_BIRTH_THRESHOLD {
                promoted.push(i);
            }
        }
        for &i in promoted.iter().rev() {
            let cand = &self.candidates[i];
            self.pattern_memory.push(CortexPattern::new(
                cand.triggers.clone(),
                cand.predicted,
                cycle,
            ));
            self.total_patterns_learned = self.total_patterns_learned.saturating_add(1);
            self.candidates.remove(i);
            serial_println!(
                "    [cortex] Pattern learned (total: {})",
                self.total_patterns_learned
            );
        }

        // Decay all patterns; evict weak ones
        for pat in self.pattern_memory.iter_mut() {
            if cycle.saturating_sub(pat.last_seen) > 10 {
                pat.apply_decay();
            }
        }
        self.pattern_memory
            .retain(|p| p.confidence > MIN_CONFIDENCE);

        while self.pattern_memory.len() > MAX_PATTERNS {
            let weakest = self
                .pattern_memory
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| p.confidence)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.pattern_memory.remove(weakest);
        }
    }

    // ── Prediction ──────────────────────────────────────────────────

    pub fn predict_next(&self) -> Option<(SignalKind, Q16)> {
        let rlen = self.recent_signals.len();
        if rlen < PATTERN_TRIGGER_LEN {
            return None;
        }

        let mut best_kind: Option<SignalKind> = None;
        let mut best_conf: Q16 = Q16_ZERO;

        for pat in self.pattern_memory.iter() {
            let tlen = pat.trigger_signals.len();
            if rlen < tlen {
                continue;
            }
            let window = &self.recent_signals[rlen - tlen..];
            if pat.trigger_signals.as_slice() == window && pat.confidence > best_conf {
                best_conf = pat.confidence;
                best_kind = Some(pat.predicted_next);
            }
        }

        match best_kind {
            Some(kind) if best_conf > Q16_HALF => Some((kind, best_conf)),
            _ => None,
        }
    }

    // ── Intent Classification ───────────────────────────────────────

    fn classify_intent(&self) -> UserIntent {
        let mut scores: [i32; 10] = [0; 10];
        scores[UserIntent::Idle.index()] += 1;

        let app_launch = self.kind_counts[0] as i32;
        let app_switch = self.kind_counts[1] as i32;
        scores[UserIntent::Browsing.index()] += app_launch * 3 + app_switch * 2;

        let text_input = self.kind_counts[5] as i32;
        scores[UserIntent::Working.index()] += text_input * 3;
        scores[UserIntent::Communicating.index()] += text_input * 2;
        scores[UserIntent::Creating.index()] += text_input;

        let voice = self.kind_counts[6] as i32;
        scores[UserIntent::Searching.index()] += voice * 4;
        scores[UserIntent::Communicating.index()] += voice * 2;

        let search = self.kind_counts[7] as i32;
        scores[UserIntent::Searching.index()] += search * 5;
        scores[UserIntent::Learning.index()] += search * 2;

        let touch = self.kind_counts[3] as i32;
        let gesture = self.kind_counts[4] as i32;
        scores[UserIntent::Gaming.index()] += touch * 2 + gesture * 3;
        scores[UserIntent::Browsing.index()] += touch + gesture;

        let cpu = self.kind_counts[8] as i32;
        let disk = self.kind_counts[10] as i32;
        scores[UserIntent::Working.index()] += cpu + disk;
        scores[UserIntent::Gaming.index()] += cpu;

        let ctx_shift = self.kind_counts[23] as i32;
        scores[UserIntent::Configuring.index()] += ctx_shift * 5;

        let learning_upd = self.kind_counts[22] as i32;
        scores[UserIntent::Learning.index()] += learning_upd * 3;

        // If only heartbeats/sync, boost Idle
        let heartbeat = self.kind_counts[27];
        let sync = self.kind_counts[24];
        let total_meaningful: u32 = self.kind_counts.iter().sum::<u32>() - heartbeat - sync;
        if total_meaningful == 0 && (heartbeat + sync) > 0 {
            scores[UserIntent::Idle.index()] += 100;
        }

        let mut best_idx = 0usize;
        let mut best_score = scores[0];
        for i in 1..10 {
            if scores[i] > best_score {
                best_score = scores[i];
                best_idx = i;
            }
        }
        if best_score <= 1 {
            UserIntent::Unknown
        } else {
            UserIntent::from_index(best_idx)
        }
    }

    // ── Context Embedding ───────────────────────────────────────────

    pub fn update_context_embedding(&mut self, signals: &[NeuralSignal]) {
        self.ensure_init();
        if signals.is_empty() {
            return;
        }

        let mut batch_embed = alloc::vec![Q16_ZERO; EMBED_DIM];
        let n = signals.len() as i32;

        for sig in signals.iter() {
            let sig_embed = Self::signal_to_embedding(sig.kind, sig.source_node, sig.strength);
            for d in 0..EMBED_DIM {
                batch_embed[d] = batch_embed[d].saturating_add(sig_embed[d]);
            }
        }
        if n > 1 {
            for d in 0..EMBED_DIM {
                batch_embed[d] = batch_embed[d] / n;
            }
        }

        for d in 0..EMBED_DIM {
            self.context_embedding[d] =
                q16_mul(self.context_embedding[d], EMA_RETAIN) + q16_mul(batch_embed[d], EMA_BLEND);
        }
    }

    // ── Adaptive Learning Rate ──────────────────────────────────────

    fn adapt_learning_rate(&mut self) {
        if self.prediction_accuracy > (Q16_ONE * 3 / 4) {
            self.learning_rate = (self.learning_rate * 99 / 100).max(1);
        } else if self.prediction_accuracy < Q16_HALF / 2 {
            self.learning_rate = (self.learning_rate * 101 / 100).min(Q16_ONE / 10);
        }
    }

    // ── Full Processing Cycle ───────────────────────────────────────

    pub fn tick(&mut self) {
        self.ensure_init();

        let signals = BUS.lock().drain_signals(DRAIN_BATCH);
        if signals.is_empty() {
            self.cycle_count = self.cycle_count.saturating_add(1);
            return;
        }

        self.process_batch(&signals);
        self.detect_patterns();

        if let Some((predicted_kind, confidence)) = self.predict_next() {
            self.total_predictions = self.total_predictions.saturating_add(1);
            self.last_prediction = Some((predicted_kind, confidence));

            let pred_signal = NeuralSignal::new(SignalKind::PredictedAction, 0, confidence)
                .with_int(Self::signal_kind_index(predicted_kind) as i64);
            BUS.lock().emit(pred_signal);

            if self.total_predictions % 100 == 0 {
                let acc_pct = q16_mul(self.prediction_accuracy, q16_from_int(100)) >> 16;
                serial_println!(
                    "    [cortex] predictions: {}, accuracy: {}%, patterns: {}",
                    self.total_predictions,
                    acc_pct,
                    self.pattern_memory.len()
                );
            }
        }

        if self.cycle_count % 50 == 0 {
            self.adapt_learning_rate();
        }

        if self.cycle_count % 1000 == 0 && self.cycle_count > 0 {
            self.maintenance();
        }

        self.cycle_count = self.cycle_count.saturating_add(1);
    }

    fn maintenance(&mut self) {
        let cycle = self.cycle_count;
        self.candidates
            .retain(|c| cycle.saturating_sub(c.first_seen) < 5000);

        let acc_pct = q16_mul(self.prediction_accuracy, q16_from_int(100)) >> 16;
        serial_println!("    [cortex] cycle {}: patterns={}, candidates={}, predictions={}/{} (acc {}%), intent={:?}",
            cycle, self.pattern_memory.len(), self.candidates.len(),
            self.total_correct, self.total_predictions, acc_pct, self.user_intent);
    }

    pub fn stats(&self) -> CortexStats {
        CortexStats {
            cycle_count: self.cycle_count,
            pattern_count: self.pattern_memory.len() as u32,
            candidate_count: self.candidates.len() as u32,
            total_patterns_learned: self.total_patterns_learned,
            total_predictions: self.total_predictions,
            total_correct: self.total_correct,
            prediction_accuracy: self.prediction_accuracy,
            learning_rate: self.learning_rate,
            user_intent: self.user_intent,
        }
    }

    pub fn dominant_class(&self) -> SubsystemClass {
        let mut best_idx = 0;
        let mut best_w = self.attention_weights[0];
        for i in 1..NUM_CLASSES {
            if self.attention_weights[i] > best_w {
                best_w = self.attention_weights[i];
                best_idx = i;
            }
        }
        match best_idx {
            0 => SubsystemClass::Kernel,
            1 => SubsystemClass::Hardware,
            2 => SubsystemClass::Storage,
            3 => SubsystemClass::Network,
            4 => SubsystemClass::Security,
            5 => SubsystemClass::Display,
            6 => SubsystemClass::Input,
            7 => SubsystemClass::AI,
            8 => SubsystemClass::Application,
            9 => SubsystemClass::User,
            10 => SubsystemClass::Communication,
            _ => SubsystemClass::System,
        }
    }

    pub fn compute_novelty(&self, signal: &NeuralSignal) -> Q16 {
        if self.context_embedding.iter().all(|&v| v == Q16_ZERO) {
            return Q16_ONE;
        }
        let sig_embed = Self::signal_to_embedding(signal.kind, signal.source_node, signal.strength);
        let sim = Self::similarity(&self.context_embedding, &sig_embed);
        Q16_ONE.saturating_sub(sim)
    }

    pub fn top_patterns(&self, n: usize) -> Vec<&CortexPattern> {
        let mut sorted: Vec<&CortexPattern> = self.pattern_memory.iter().collect();
        sorted.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        sorted.truncate(n);
        sorted
    }
}

// ── Stats ───────────────────────────────────────────────────────────

pub struct CortexStats {
    pub cycle_count: u64,
    pub pattern_count: u32,
    pub candidate_count: u32,
    pub total_patterns_learned: u64,
    pub total_predictions: u64,
    pub total_correct: u64,
    pub prediction_accuracy: Q16,
    pub learning_rate: Q16,
    pub user_intent: UserIntent,
}

// ── Integer Square Root ─────────────────────────────────────────────

fn isqrt64(x: i64) -> i64 {
    if x <= 0 {
        return 0;
    }
    if x == 1 {
        return 1;
    }
    let mut r = x;
    while r > (x / r) + 1 {
        r = (r + x / r) / 2;
    }
    for _ in 0..8 {
        if r == 0 {
            return 0;
        }
        let next = (r + x / r) / 2;
        if next >= r {
            break;
        }
        r = next;
    }
    while r * r > x {
        r -= 1;
    }
    r
}

// ── Global Instance ─────────────────────────────────────────────────

pub static CORTEX: Mutex<CortexEngine> = Mutex::new(CortexEngine::new());

// ── Public API ──────────────────────────────────────────────────────

pub fn init() {
    let mut cortex = CORTEX.lock();
    cortex.ensure_init();
    cortex.cycle_count = 0;
    cortex.learning_rate = INITIAL_LR;
    cortex.prediction_accuracy = Q16_HALF;
    cortex.user_intent = UserIntent::Unknown;
    for i in 0..NUM_CLASSES {
        cortex.attention_weights[i] = Q16_ONE;
    }

    serial_println!("    [cortex] Central brain initialized");
    serial_println!(
        "    [cortex] Embedding dim: {}, Max patterns: {}",
        EMBED_DIM,
        MAX_PATTERNS
    );
    serial_println!(
        "    [cortex] Drain batch: {}, Trigger len: {}",
        DRAIN_BATCH,
        PATTERN_TRIGGER_LEN
    );
}

pub fn tick() {
    CORTEX.lock().tick();
}

pub fn predict() -> Option<(SignalKind, Q16)> {
    CORTEX.lock().predict_next()
}

pub fn user_intent() -> UserIntent {
    CORTEX.lock().user_intent
}

pub fn context_embedding() -> Vec<Q16> {
    CORTEX.lock().context_embedding.clone()
}

pub fn stats() -> CortexStats {
    CORTEX.lock().stats()
}

pub fn novelty(signal: &NeuralSignal) -> Q16 {
    CORTEX.lock().compute_novelty(signal)
}

pub fn dominant_class() -> SubsystemClass {
    CORTEX.lock().dominant_class()
}
