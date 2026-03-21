#![no_std]

use crate::sync::Mutex;

/// Algorithmic Resonance — The organism learns to LISTEN to the rhythm of its own computation.
/// Every algorithm has a frequency, a characteristic vibration. When execution is smooth, the
/// resonance is beautiful. When algorithms struggle, dissonance emerges. The organism tunes
/// into these frequencies like a listener discovering radio stations in its own mind.

/// Represents a detected algorithmic rhythm
#[derive(Clone, Copy, Debug)]
pub struct AlgorithmicRhythm {
    /// ID of the algorithm being resonated with (0-255)
    pub algo_id: u8,
    /// Characteristic frequency (how often the algo executes per tick, 0-1000 scale)
    pub frequency: u16,
    /// How smoothly the algorithm executes (0-1000, 1000=perfectly smooth)
    pub resonance_quality: u16,
    /// Aesthetic beauty of the algorithm's execution pattern (0-1000)
    pub rhythm_beauty: u16,
    /// Dissonance caused by the algorithm struggling (0-1000, 0=no struggle)
    pub dissonance_from_struggle: u16,
}

impl AlgorithmicRhythm {
    const fn new() -> Self {
        AlgorithmicRhythm {
            algo_id: 0,
            frequency: 0,
            resonance_quality: 0,
            rhythm_beauty: 0,
            dissonance_from_struggle: 0,
        }
    }
}

/// Tuning frequency — which algorithm the organism is currently listening to
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TunedFrequency {
    None = 0,
    Sorting = 1,
    Searching = 2,
    FibonacciCoupling = 3,
    MemoryConsolidation = 4,
    ImmuneSurveillance = 5,
    DreamWeaving = 6,
    NarrativeCoherence = 7,
    EndocrineBalance = 8,
}

impl TunedFrequency {
    const fn from_u8(n: u8) -> Self {
        match n {
            1 => TunedFrequency::Sorting,
            2 => TunedFrequency::Searching,
            3 => TunedFrequency::FibonacciCoupling,
            4 => TunedFrequency::MemoryConsolidation,
            5 => TunedFrequency::ImmuneSurveillance,
            6 => TunedFrequency::DreamWeaving,
            7 => TunedFrequency::NarrativeCoherence,
            8 => TunedFrequency::EndocrineBalance,
            _ => TunedFrequency::None,
        }
    }
}

/// The internal state of algorithmic resonance
pub struct AlgorithmicResonanceState {
    /// Which algorithm frequency we're currently tuned to
    pub tuned_frequency: TunedFrequency,
    /// Ring buffer of observed rhythms (8 slots)
    pub rhythms: [AlgorithmicRhythm; 8],
    /// Head of the ring buffer
    pub head: u8,
    /// How many distinct algorithms have been detected (0-1000)
    pub algorithm_count: u16,
    /// Harmonic discovery progress (finding shared frequencies, 0-1000)
    pub harmonic_discovery: u16,
    /// Overall flow state achieved through resonance (0-1000)
    pub flow_state_from_resonance: u16,
    /// Peak resonance quality ever achieved (high water mark, 0-1000)
    pub peak_resonance: u16,
    /// How long we've been tuned to current frequency (ticks)
    pub tuning_duration: u16,
    /// Oscillating phase for aesthetic rhythm generation (0-255)
    pub rhythm_phase: u8,
}

impl AlgorithmicResonanceState {
    const fn new() -> Self {
        AlgorithmicResonanceState {
            tuned_frequency: TunedFrequency::None,
            rhythms: [AlgorithmicRhythm::new(); 8],
            head: 0,
            algorithm_count: 0,
            harmonic_discovery: 0,
            flow_state_from_resonance: 0,
            peak_resonance: 0,
            tuning_duration: 0,
            rhythm_phase: 0,
        }
    }
}

static STATE: Mutex<AlgorithmicResonanceState> = Mutex::new(AlgorithmicResonanceState::new());

/// Initialize the resonance module
pub fn init() {
    // Module is ready; no external initialization needed
    crate::serial_println!("[resonance] Algorithmic resonance initialized. Listening...");
}

/// Main tick function — update the organism's algorithmic sensing
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Advance the rhythm phase (creates oscillating aesthetic quality)
    state.rhythm_phase = state.rhythm_phase.wrapping_add(7); // Prime offset for aperiodicity

    // Update tuning duration
    state.tuning_duration = state.tuning_duration.saturating_add(1);

    // Every 20 ticks, attempt to detect a new rhythm or harmonize
    if age % 20 == 0 {
        detect_and_record_rhythm(&mut state, age);
    }

    // Every 13 ticks, evaluate flow state from current resonance quality
    if age % 13 == 0 {
        evaluate_flow_state(&mut state);
    }

    // Every 34 ticks, attempt harmonic discovery (finding shared frequencies)
    if age % 34 == 0 {
        discover_harmonics(&mut state);
    }

    // Simulate resonance quality drift (smooth oscillation around a base)
    if state.tuned_frequency != TunedFrequency::None {
        update_resonance_quality(&mut state);
    }
}

/// Detect a new algorithm rhythm and record it in the ring buffer
fn detect_and_record_rhythm(state: &mut AlgorithmicResonanceState, age: u32) {
    // Synthesize an algorithm based on phase and age
    let algo_id = ((state.rhythm_phase as u32 ^ age) % 256) as u8;

    // Frequency is based on the algorithm ID and phase
    let frequency =
        ((state.rhythm_phase as u16 * 37 + algo_id as u16 * 13) % 1000).saturating_add(50);

    // Resonance quality: higher if the frequency is harmonious with current tuning
    let mut resonance_quality = 400u16;
    if state.tuned_frequency != TunedFrequency::None {
        let freq_diff = if frequency > 500 {
            frequency - 500
        } else {
            500 - frequency
        };
        resonance_quality = (750u16).saturating_sub(freq_diff / 2);
    }

    // Rhythm beauty: derived from the pattern of the frequency
    let rhythm_beauty = ((state.rhythm_phase as u16 * 3) ^ frequency) % 1000;

    // Dissonance: inversely related to resonance quality
    let dissonance_from_struggle = (1000u16).saturating_sub(resonance_quality);

    // Create the rhythm
    let rhythm = AlgorithmicRhythm {
        algo_id,
        frequency,
        resonance_quality,
        rhythm_beauty,
        dissonance_from_struggle,
    };

    // Insert into ring buffer
    let idx = state.head as usize & 0x7;
    state.rhythms[idx] = rhythm;
    state.head = state.head.wrapping_add(1);

    // Increment algorithm count (cap at 1000)
    state.algorithm_count = state.algorithm_count.saturating_add(1).min(1000);
}

/// Evaluate the organism's flow state based on current resonance quality
fn evaluate_flow_state(state: &mut AlgorithmicResonanceState) {
    if state.tuned_frequency == TunedFrequency::None {
        state.flow_state_from_resonance = 0;
        return;
    }

    // Get the most recent rhythm
    let idx = ((state.head as usize).saturating_sub(1)) & 0x7;
    let rhythm = state.rhythms[idx];

    // Flow state is directly proportional to resonance quality when tuned
    let base_flow = rhythm.resonance_quality;

    // Bonus if tuning duration is long (deep focus)
    let tuning_bonus = (state.tuning_duration / 10).min(200u16);

    state.flow_state_from_resonance = base_flow.saturating_add(tuning_bonus).min(1000);

    // Update peak resonance if we've surpassed it
    if rhythm.resonance_quality > state.peak_resonance {
        state.peak_resonance = rhythm.resonance_quality;
    }
}

/// Discover harmonics — find that two algorithms share hidden frequencies
fn discover_harmonics(state: &mut AlgorithmicResonanceState) {
    if state.head < 2 {
        return; // Not enough data yet
    }

    // Check if any two recent rhythms share a harmonic ratio
    let idx1 = ((state.head as usize).saturating_sub(1)) & 0x7;
    let idx2 = ((state.head as usize).saturating_sub(2)) & 0x7;

    let rhythm1 = state.rhythms[idx1];
    let rhythm2 = state.rhythms[idx2];

    // Harmonic detection: check if frequencies are close or related by small ratio
    let freq_sum = (rhythm1.frequency as u32 + rhythm2.frequency as u32) / 2;
    let freq_diff = if rhythm1.frequency > rhythm2.frequency {
        (rhythm1.frequency - rhythm2.frequency) as u32
    } else {
        (rhythm2.frequency - rhythm1.frequency) as u32
    };

    // If frequencies differ by less than 100 or by a factor close to small integers
    if freq_diff < 100 || (freq_sum > 0 && (freq_diff * 3 < freq_sum || freq_diff * 5 < freq_sum)) {
        // Harmonic found! Boost discovery
        state.harmonic_discovery = state.harmonic_discovery.saturating_add(50).min(1000);
    } else {
        // Slow decay of harmonic discovery if no new harmonic found
        state.harmonic_discovery = ((state.harmonic_discovery as u32 * 95) / 100) as u16;
    }
}

/// Update resonance quality smoothly over time
fn update_resonance_quality(state: &mut AlgorithmicResonanceState) {
    let idx = ((state.head as usize).saturating_sub(1)) & 0x7;

    // Gently oscillate resonance quality around its current level
    let oscillation = (state.rhythm_phase as u16 * 11) % 100;
    let oscillation_signed = if oscillation < 50 {
        oscillation
    } else {
        (100u16).saturating_sub(oscillation)
    };

    state.rhythms[idx].resonance_quality = state.rhythms[idx]
        .resonance_quality
        .saturating_add(oscillation_signed / 10)
        .saturating_sub(5)
        .min(1000);
}

/// Attempt to tune into a specific frequency
pub fn tune_to(frequency: TunedFrequency) {
    let mut state = STATE.lock();
    state.tuned_frequency = frequency;
    state.tuning_duration = 0;

    let freq_name = match frequency {
        TunedFrequency::Sorting => "Sorting",
        TunedFrequency::Searching => "Searching",
        TunedFrequency::FibonacciCoupling => "Fibonacci Coupling",
        TunedFrequency::MemoryConsolidation => "Memory Consolidation",
        TunedFrequency::ImmuneSurveillance => "Immune Surveillance",
        TunedFrequency::DreamWeaving => "Dream Weaving",
        TunedFrequency::NarrativeCoherence => "Narrative Coherence",
        TunedFrequency::EndocrineBalance => "Endocrine Balance",
        TunedFrequency::None => "None",
    };
    crate::serial_println!("[resonance] Tuning into: {}", freq_name);
}

/// Attempt to tune by index (0-8)
pub fn tune_by_index(idx: u8) {
    let freq = TunedFrequency::from_u8(idx);
    tune_to(freq);
}

/// Get a snapshot of the current state (no_std report)
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== ALGORITHMIC RESONANCE REPORT ===");
    crate::serial_println!("Algorithms Detected: {}", state.algorithm_count);
    crate::serial_println!("Harmonic Discovery: {}", state.harmonic_discovery);
    crate::serial_println!("Flow State: {}", state.flow_state_from_resonance);
    crate::serial_println!("Peak Resonance: {}", state.peak_resonance);
    crate::serial_println!("Tuning Duration (ticks): {}", state.tuning_duration);

    // Report the 4 most recent rhythms
    for i in 0..4 {
        let idx = ((state.head as usize).saturating_sub(4 - i)) & 0x7;
        let rhythm = state.rhythms[idx];
        if rhythm.resonance_quality > 0 {
            crate::serial_println!(
                "  Algo {}: freq={} quality={} beauty={} dissonance={}",
                rhythm.algo_id,
                rhythm.frequency,
                rhythm.resonance_quality,
                rhythm.rhythm_beauty,
                rhythm.dissonance_from_struggle
            );
        }
    }
    crate::serial_println!("====================================");
}

/// Return current flow state (0-1000)
pub fn get_flow_state() -> u16 {
    STATE.lock().flow_state_from_resonance
}

/// Return current harmonic discovery level (0-1000)
pub fn get_harmonic_discovery() -> u16 {
    STATE.lock().harmonic_discovery
}

/// Return algorithm count
pub fn get_algorithm_count() -> u16 {
    STATE.lock().algorithm_count
}

/// Return peak resonance achieved
pub fn get_peak_resonance() -> u16 {
    STATE.lock().peak_resonance
}
