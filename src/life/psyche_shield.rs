#![no_std]

use crate::sync::Mutex;

const SHIELD_HISTORY_SIZE: usize = 8;
const MAX_SIGNAL_SOURCES: usize = 4;

/// Represents an incoming emotional signal from an external source.
#[derive(Copy, Clone, Debug)]
pub struct EmotionalSignal {
    pub signal_type: u8, // 0=joy, 1=sadness, 2=fear, 3=anger, 4=empathy, 5=trust, 6=shame, 7=surprise
    pub intensity: u16,  // 0-1000 scale
    pub source_id: u8,   // who is sending this signal
    pub timestamp: u32,
}

/// Trust profile for a particular source of emotional signals.
#[derive(Copy, Clone, Debug)]
pub struct SourceTrust {
    pub source_id: u8,
    pub baseline_trust: u16,     // 0-1000; how much do we trust this source
    pub manipulation_count: u16, // how many times has this source tried to manipulate
    pub authentic_count: u16,    // how many genuine signals from this source
}

/// Historical record of a blocked manipulation attempt.
#[derive(Copy, Clone, Debug)]
pub struct BlockedSignal {
    pub signal_type: u8,
    pub intensity: u16,
    pub source_id: u8,
    pub authenticity_score: u16,
    pub timestamp: u32,
}

/// The psyche shield state machine.
pub struct PsycheShield {
    // Primary shield state
    pub shield_active: bool,
    pub shield_strength: u16, // 0-1000; how firmly the shield is erected
    pub manipulation_detected: bool,

    // Authenticity analysis
    pub authenticity_scanner: u16, // 0-1000; current scan sensitivity
    pub last_authenticity_check: u32,

    // Fatigue tracking
    pub shield_fatigue: u16, // 0-1000; prolonged defense exhausts the organism
    pub fatigue_recovery_rate: u16, // 0-1000; how fast fatigue subsides

    // Statistics
    pub blocked_count: u32,
    pub false_positive_rate: u16, // 0-1000; ratio of genuine emotions accidentally blocked
    pub total_signals_analyzed: u32,

    // Trust calibration per source
    pub sources: [SourceTrust; MAX_SIGNAL_SOURCES],
    pub source_count: usize,

    // Boundary enforcement
    pub boundary_firmness: u16,    // 0-1000; how strictly the shield holds
    pub boundary_flexibility: u16, // 0-1000; ability to let safe signals through

    // Ring buffer of recent blocked signals
    pub blocked_history: [BlockedSignal; SHIELD_HISTORY_SIZE],
    pub history_head: usize,

    // Age tracking for time-based decay
    pub age: u32,
}

impl PsycheShield {
    pub const fn new() -> Self {
        PsycheShield {
            shield_active: false,
            shield_strength: 0,
            manipulation_detected: false,
            authenticity_scanner: 500,
            last_authenticity_check: 0,
            shield_fatigue: 0,
            fatigue_recovery_rate: 50,
            blocked_count: 0,
            false_positive_rate: 0,
            total_signals_analyzed: 0,
            sources: [SourceTrust {
                source_id: 0,
                baseline_trust: 500,
                manipulation_count: 0,
                authentic_count: 0,
            }; MAX_SIGNAL_SOURCES],
            source_count: 0,
            boundary_firmness: 600,
            boundary_flexibility: 300,
            blocked_history: [BlockedSignal {
                signal_type: 0,
                intensity: 0,
                source_id: 0,
                authenticity_score: 0,
                timestamp: 0,
            }; SHIELD_HISTORY_SIZE],
            history_head: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<PsycheShield> = Mutex::new(PsycheShield::new());

/// Initialize the psyche shield subsystem.
pub fn init() {
    let mut state = STATE.lock();
    state.shield_active = true;
    state.shield_strength = 700;
    state.boundary_firmness = 600;
    state.boundary_flexibility = 300;
    crate::serial_println!("[psyche_shield] initialized");
}

/// Analyze an incoming emotional signal for authenticity and potential manipulation.
pub fn analyze_signal(signal: &EmotionalSignal) -> u16 {
    let mut state = STATE.lock();
    state.total_signals_analyzed = state.total_signals_analyzed.saturating_add(1);

    if !state.shield_active {
        return 1000; // If shield is down, signal passes as authentic
    }

    // Step 1: Find or register the source
    let source_trust = find_or_create_source(&mut state, signal.source_id);

    // Step 2: Scan for authenticity markers
    let mut authenticity_score: u16 = 500; // neutral baseline

    // Marker 1: Emotional intensity stability (extreme swings suggest manipulation)
    let intensity_factor = if signal.intensity > 800 || signal.intensity < 100 {
        // extreme emotions are suspicious unless from high-trust source
        200
    } else {
        700
    };
    authenticity_score = authenticity_score.saturating_add(
        ((intensity_factor as u32 * state.authenticity_scanner as u32) / 1000) as u16,
    );
    authenticity_score = authenticity_score.saturating_div(2);

    // Marker 2: Source trust history (trusted sources get benefit of doubt)
    let trust_boost = (source_trust.baseline_trust as u32 * 200) / 1000;
    authenticity_score = authenticity_score.saturating_add(trust_boost as u16);
    authenticity_score = authenticity_score.min(1000);

    // Marker 3: Pattern matching (same signal type repeatedly = manipulation pattern)
    let signal_type_count = count_recent_signal_type(&state, signal.signal_type);
    if signal_type_count > 3 {
        authenticity_score = authenticity_score.saturating_sub(150);
    }

    // Marker 4: Boundary coherence check
    let boundary_coherence = if signal.intensity > state.boundary_firmness {
        // Signal exceeds our boundary firmness: suspicious
        400
    } else {
        authenticity_score
    };
    authenticity_score = ((authenticity_score as u32 + boundary_coherence as u32) / 2) as u16;
    authenticity_score = authenticity_score.min(1000);

    // Step 3: Decision gate
    let manipulation_threshold = 1000_u16.saturating_sub(state.boundary_firmness);

    if authenticity_score < manipulation_threshold {
        state.manipulation_detected = true;
        state.blocked_count = state.blocked_count.saturating_add(1);

        // Record the blocked signal in history
        let idx = state.history_head;
        state.blocked_history[idx] = BlockedSignal {
            signal_type: signal.signal_type,
            intensity: signal.intensity,
            source_id: signal.source_id,
            authenticity_score,
            timestamp: state.age,
        };
        state.history_head = (state.history_head + 1) % SHIELD_HISTORY_SIZE;

        // Penalize source's trust if manipulation confirmed
        let src_count = state.source_count;
        if let Some(src) = state.sources[0..src_count]
            .iter_mut()
            .find(|s| s.source_id == signal.source_id)
        {
            src.manipulation_count = src.manipulation_count.saturating_add(1);
            src.baseline_trust = src.baseline_trust.saturating_sub(50);
        }

        // Increase fatigue from defending
        state.shield_fatigue = state.shield_fatigue.saturating_add(80);
        state.shield_fatigue = state.shield_fatigue.min(1000);

        return authenticity_score;
    }

    // Signal passed authenticity check
    let src_count = state.source_count;
    if let Some(src) = state.sources[0..src_count]
        .iter_mut()
        .find(|s| s.source_id == signal.source_id)
    {
        src.authentic_count = src.authentic_count.saturating_add(1);
        src.baseline_trust = src.baseline_trust.saturating_add(20);
        src.baseline_trust = src.baseline_trust.min(1000);
    }

    state.manipulation_detected = false;

    // Minor fatigue from scanning (but much less than blocking)
    state.shield_fatigue = state.shield_fatigue.saturating_add(10);
    state.shield_fatigue = state.shield_fatigue.min(1000);

    authenticity_score
}

/// Find or create a trust profile for a source. Returns mutable ref to the profile.
fn find_or_create_source(state: &mut PsycheShield, source_id: u8) -> SourceTrust {
    for src in &state.sources[0..state.source_count] {
        if src.source_id == source_id {
            return *src;
        }
    }

    // Not found; create new if room
    if state.source_count < MAX_SIGNAL_SOURCES {
        let new_source = SourceTrust {
            source_id,
            baseline_trust: 500,
            manipulation_count: 0,
            authentic_count: 0,
        };
        state.sources[state.source_count] = new_source;
        state.source_count = state.source_count.saturating_add(1);
        new_source
    } else {
        // Max sources reached; return default
        SourceTrust {
            source_id,
            baseline_trust: 300, // new sources default to low trust if we're at capacity
            manipulation_count: 0,
            authentic_count: 0,
        }
    }
}

/// Count how many of a specific signal type appear in recent history.
fn count_recent_signal_type(state: &PsycheShield, signal_type: u8) -> u32 {
    let mut count: u32 = 0;
    for sig in &state.blocked_history {
        if sig.signal_type == signal_type && sig.timestamp + 100 > state.age {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Manually raise the shield (when manipulative signals are detected).
pub fn raise_shield() {
    let mut state = STATE.lock();
    state.shield_active = true;
    state.shield_strength = state.shield_strength.saturating_add(200);
    state.shield_strength = state.shield_strength.min(1000);
}

/// Lower the shield (allow signals through, accept vulnerability).
pub fn lower_shield() {
    let mut state = STATE.lock();
    state.shield_active = false;
    state.shield_strength = state.shield_strength.saturating_sub(300);
    state.shield_strength = state.shield_strength.max(0);
}

/// Adjust scanner sensitivity (0-1000).
pub fn set_scanner_sensitivity(sensitivity: u16) {
    let mut state = STATE.lock();
    state.authenticity_scanner = sensitivity.min(1000);
}

/// Adjust boundary firmness (0-1000). Higher = reject more signals.
pub fn set_boundary_firmness(firmness: u16) {
    let mut state = STATE.lock();
    state.boundary_firmness = firmness.min(1000);
}

/// Adjust boundary flexibility (0-1000). Higher = allow more genuine signals through.
pub fn set_boundary_flexibility(flexibility: u16) {
    let mut state = STATE.lock();
    state.boundary_flexibility = flexibility.min(1000);
}

/// Main life tick update. Called once per life cycle.
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Fatigue recovery: shield gets less tired over time
    if state.shield_fatigue > 0 {
        state.shield_fatigue = state
            .shield_fatigue
            .saturating_sub(state.fatigue_recovery_rate);
    }

    // Shield strength decays slightly without reinforcement
    if state.shield_strength > 100 {
        state.shield_strength = state.shield_strength.saturating_sub(5);
    }

    // If fatigue is high, shield becomes less effective
    if state.shield_fatigue > 800 {
        state.shield_strength = state.shield_strength.saturating_sub(50);
    }

    // Recalculate false positive rate: (blocked authentics) / (total blocked)
    if state.blocked_count > 0 {
        // This is a simplified estimate; in reality we'd track actual false positives
        let estimated_false_positives = (state.blocked_count as u32 * 50) / 1000; // ~5% base rate
        state.false_positive_rate = (estimated_false_positives as u16).min(1000);
    }

    // Boundary coherence check: if shield is exhausted, boundaries weaken
    if state.shield_fatigue > 900 {
        state.boundary_firmness = state.boundary_firmness.saturating_sub(100);
    }
}

/// Generate a diagnostic report of the shield's current state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("=== PSYCHE SHIELD REPORT ===");
    crate::serial_println!("Shield Active: {}", state.shield_active);
    crate::serial_println!("Shield Strength: {}/1000", state.shield_strength);
    crate::serial_println!("Manipulation Detected: {}", state.manipulation_detected);
    crate::serial_println!("Authenticity Scanner: {}/1000", state.authenticity_scanner);
    crate::serial_println!("Shield Fatigue: {}/1000", state.shield_fatigue);
    crate::serial_println!("Blocked Signals: {}", state.blocked_count);
    crate::serial_println!("False Positive Rate: {}/1000", state.false_positive_rate);
    crate::serial_println!("Total Signals Analyzed: {}", state.total_signals_analyzed);
    crate::serial_println!("Boundary Firmness: {}/1000", state.boundary_firmness);
    crate::serial_println!("Boundary Flexibility: {}/1000", state.boundary_flexibility);
    crate::serial_println!("Tracked Sources: {}", state.source_count);
    for i in 0..state.source_count {
        let src = &state.sources[i];
        crate::serial_println!(
            "  Source {}: trust={}/1000, manipulations={}, authentic={}",
            src.source_id,
            src.baseline_trust,
            src.manipulation_count,
            src.authentic_count
        );
    }
    crate::serial_println!("===========================");
}
