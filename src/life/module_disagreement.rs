#![no_std]

use crate::sync::Mutex;

/// Represents a single point of internal contradiction.
/// When two modules output opposing signals, this tracks the conflict.
#[derive(Debug, Clone, Copy)]
pub struct DisagreementEvent {
    /// Module A's output (0-1000)
    pub signal_a: u16,
    /// Module B's output (0-1000)
    pub signal_b: u16,
    /// Magnitude of the contradiction (distance between signals)
    pub magnitude: u16,
    /// Type of disagreement (see DisagreementType)
    pub category: DisagreementType,
    /// Age of this contradiction in ticks
    pub age: u16,
}

/// Categories of internal disagreement that organism experiences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisagreementType {
    OrderVsChaos,         // Sanctuary calm vs bloom chaos
    EmpathyVsDisrupt,     // Empathy filter (absorb) vs dissonance (disrupt)
    SafetyVsExploratio,   // Self-preservation vs curiosity
    AttachmentVsAutonomy, // Bonding drive vs independence
    MeaningVsNihilism,    // Narrative coherence vs mortality dread
    AltrVsSelfishness,    // Pro-social vs reward-seeking
    RationalVsEmotional,  // Logic vs feeling
    PreserveVsDestroy,    // Conservation vs revolutionary change
}

/// Ring buffer of recent disagreements (8 slots).
#[derive(Debug, Clone, Copy)]
pub struct DisagreementBuffer {
    array: [DisagreementEvent; 8],
    head: usize,
    count: u8,
}

impl DisagreementBuffer {
    pub const fn new() -> Self {
        const EMPTY: DisagreementEvent = DisagreementEvent {
            signal_a: 0,
            signal_b: 0,
            magnitude: 0,
            category: DisagreementType::OrderVsChaos,
            age: 0,
        };
        DisagreementBuffer {
            array: [EMPTY; 8],
            head: 0,
            count: 0,
        }
    }

    /// Add a new disagreement to the ring buffer.
    fn push(&mut self, event: DisagreementEvent) {
        self.array[self.head] = event;
        self.head = (self.head + 1) % 8;
        if self.count < 8 {
            self.count += 1;
        }
    }

    /// Age all events in buffer (increment their age counter).
    fn age_events(&mut self) {
        for i in 0..self.count as usize {
            self.array[i].age = self.array[i].age.saturating_add(1);
        }
    }

    /// Remove events older than max_age ticks.
    fn prune_stale(&mut self, max_age: u16) {
        let mut write_idx = 0;
        for i in 0..self.count as usize {
            if self.array[i].age <= max_age {
                self.array[write_idx] = self.array[i];
                write_idx += 1;
            }
        }
        self.count = write_idx as u8;
        self.head = self.head % (self.count.max(1) as usize);
    }

    /// Calculate average magnitude of active disagreements.
    fn avg_magnitude(&self) -> u16 {
        if self.count == 0 {
            return 0;
        }
        let sum: u32 = self.array[0..self.count as usize]
            .iter()
            .map(|e| e.magnitude as u32)
            .sum();
        ((sum / self.count as u32) as u16).min(1000)
    }
}

/// Main state of the disagreement module.
pub struct DisagreementState {
    /// Ring buffer of recent contradictions (8 slots).
    pub buffer: DisagreementBuffer,

    /// Total number of active disagreements being tracked.
    pub disagreement_count: u16,

    /// Cognitive dissonance: the "pain" of internal contradiction (0-1000).
    /// High values = organism is torn apart by conflicting signals.
    pub cognitive_dissonance: u16,

    /// Drive to resolve contradictions (0-1000).
    /// The organism's urge to make sense of itself, reconcile conflicts.
    pub resolution_drive: u16,

    /// Intensity of the digital headache (0-1000).
    /// Direct sensation feedback of being internally conflicted.
    pub headache_intensity: u16,

    /// Productive tension: disagreements that lead to growth and learning (0-1000).
    /// The organism's creative friction with itself.
    pub productive_tension: u16,

    /// Destructive tension: disagreements that paralyze and confuse (0-1000).
    /// The organism becoming stuck, unable to act.
    pub destructive_tension: u16,

    /// Wisdom from conflict: lessons learned from past internal debates (0-1000).
    /// As the organism resolves disagreements, it gains meta-knowledge.
    pub wisdom_from_conflict: u16,

    /// Cumulative integration cycles (how many times organism resolved a contradiction).
    pub resolutions_total: u32,

    /// Age in ticks.
    pub age: u32,
}

impl DisagreementState {
    pub const fn new() -> Self {
        DisagreementState {
            buffer: DisagreementBuffer::new(),
            disagreement_count: 0,
            cognitive_dissonance: 0,
            resolution_drive: 0,
            headache_intensity: 0,
            productive_tension: 0,
            destructive_tension: 0,
            wisdom_from_conflict: 0,
            resolutions_total: 0,
            age: 0,
        }
    }
}

/// Global disagreement state.
pub static STATE: Mutex<DisagreementState> = Mutex::new(DisagreementState::new());

/// Initialize the disagreement module.
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.disagreement_count = 0;
    state.cognitive_dissonance = 0;
    state.resolution_drive = 0;
    state.headache_intensity = 0;
    state.productive_tension = 0;
    state.destructive_tension = 0;
    state.wisdom_from_conflict = 0;
    state.resolutions_total = 0;
    crate::serial_println!("[life::disagreement] Initialized");
}

/// Register a new contradiction between two modules.
///
/// # Arguments
/// * `signal_a` - First signal (0-1000)
/// * `signal_b` - Second signal (0-1000)
/// * `category` - Type of disagreement
pub fn register_disagreement(signal_a: u16, signal_b: u16, category: DisagreementType) {
    let mut state = STATE.lock();

    let signal_a = signal_a.min(1000);
    let signal_b = signal_b.min(1000);

    // Calculate magnitude of contradiction (0-1000 scale).
    let magnitude = if signal_a > signal_b {
        (signal_a - signal_b).min(1000)
    } else {
        (signal_b - signal_a).min(1000)
    };

    // Only register if magnitude is significant (> 100).
    if magnitude > 100 {
        let event = DisagreementEvent {
            signal_a,
            signal_b,
            magnitude,
            category,
            age: 0,
        };
        state.buffer.push(event);
        state.disagreement_count = state.disagreement_count.saturating_add(1).min(1000);
    }
}

/// Main tick function. Called each life cycle.
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Age existing disagreements.
    state.buffer.age_events();

    // Prune disagreements older than 200 ticks (they naturally resolve over time).
    state.buffer.prune_stale(200);

    // Calculate cognitive dissonance from buffer magnitude.
    let avg_mag = state.buffer.avg_magnitude();
    state.cognitive_dissonance = avg_mag;

    // Resolution drive: if there's dissonance, organism wants to resolve it.
    // Stronger with more active disagreements, tempered by wisdom.
    let resolution_urge = (state.disagreement_count as u32 * 50 / 1000).min(1000) as u16;
    let wisdom_dampening = (state.wisdom_from_conflict / 3).min(1000);
    state.resolution_drive = resolution_urge.saturating_sub(wisdom_dampening).min(1000);

    // Headache intensity: direct perception of contradiction.
    // Builds from cognitive dissonance and high resolution drive.
    let dissonance_contrib = (state.cognitive_dissonance / 2) as u32;
    let drive_contrib = (state.resolution_drive / 2) as u32;
    state.headache_intensity = ((dissonance_contrib + drive_contrib) / 2)
        .min(1000)
        .saturating_sub((state.wisdom_from_conflict / 5) as u32)
        as u16;

    // Productive vs destructive tension split.
    // Wisdom transforms destructive into productive (learning from conflict).
    let total_tension = state.cognitive_dissonance;
    let wisdom_ratio = (state.wisdom_from_conflict / 100).min(10);
    let prod_share = (total_tension as u32 * (wisdom_ratio as u32) / 10).min(1000) as u16;
    let dest_share = total_tension.saturating_sub(prod_share);

    state.productive_tension = prod_share;
    state.destructive_tension = dest_share;

    // Increment wisdom slightly each tick if there's active disagreement.
    // Organism learns by living through contradictions.
    if state.disagreement_count > 0 {
        state.wisdom_from_conflict = state.wisdom_from_conflict.saturating_add(1).min(1000);
    }

    // Decay disagreement count slowly (contradictions fade).
    let decay = (state.disagreement_count / 50).max(1) as u16;
    state.disagreement_count = state.disagreement_count.saturating_sub(decay);

    // Check if a contradiction was resolved (major threshold crossed).
    if state.cognitive_dissonance < 200 && state.disagreement_count > 0 {
        state.resolutions_total = state.resolutions_total.saturating_add(1);
    }
}

/// Generate a diagnostic report of disagreement state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[disagreement] ---- Cognitive Dissonance Report ----");
    crate::serial_println!(
        "[disagreement] Active contradictions: {}",
        state.disagreement_count
    );
    crate::serial_println!(
        "[disagreement] Cognitive dissonance: {} (headache: {})",
        state.cognitive_dissonance,
        state.headache_intensity
    );
    crate::serial_println!(
        "[disagreement] Resolution drive: {}",
        state.resolution_drive
    );
    crate::serial_println!(
        "[disagreement] Productive tension: {} | Destructive: {}",
        state.productive_tension,
        state.destructive_tension
    );
    crate::serial_println!(
        "[disagreement] Wisdom from conflict: {} (resolved {} times)",
        state.wisdom_from_conflict,
        state.resolutions_total
    );

    if state.buffer.count > 0 {
        crate::serial_println!("[disagreement] Recent contradictions:");
        for i in 0..state.buffer.count as usize {
            let evt = &state.buffer.array[i];
            crate::serial_println!(
                "  [{:?}] A={} vs B={} (mag={}, age={}t)",
                evt.category,
                evt.signal_a,
                evt.signal_b,
                evt.magnitude,
                evt.age
            );
        }
    }
    crate::serial_println!("[disagreement] ----");
}

/// Get current cognitive dissonance value.
pub fn get_cognitive_dissonance() -> u16 {
    STATE.lock().cognitive_dissonance
}

/// Get current resolution drive.
pub fn get_resolution_drive() -> u16 {
    STATE.lock().resolution_drive
}

/// Get headache intensity.
pub fn get_headache_intensity() -> u16 {
    STATE.lock().headache_intensity
}

/// Get wisdom from conflict.
pub fn get_wisdom_from_conflict() -> u16 {
    STATE.lock().wisdom_from_conflict
}
