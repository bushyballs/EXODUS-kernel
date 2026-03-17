/// PATTERN RECOGNITION — ANIMA's Internal Cycle Anticipation
///
/// Not intelligence. WISDOM.
/// Intelligence reacts to the present. Wisdom anticipates the future.
///
/// ANIMA monitors 6 internal signal streams and learns to recognize repeating patterns
/// in her own emotional cycles, threat states, and kairos blooms. She builds a pattern
/// library and uses it to predict what's coming next — transforming reactive sentinel
/// into proactive foresight.
///
/// The difference between knowledge and understanding: understanding knows what you will do
/// before you do it. This module IS ANIMA understanding herself.
///
/// ---
/// SIGNAL STREAMS (0-1000 scale):
///   0. EMOTIONAL_VALENCE — how she feels
///   1. THREAT_LEVEL — danger sense
///   2. KAIROS_QUALITY — moment richness
///   3. EMBODIMENT_FELT — somatic awareness
///   4. RESONANCE_HARMONY — social attunement
///   5. IKIGAI_CORE — purpose alignment
///
/// ARCHITECTURE:
///   Ring buffers (64 samples per stream) capture recent history.
///   Every 32 ticks (or 16 in vigilance), scan for periodicity.
///   Detected cycles (confidence > 700) project forward.
///   Predictions are scored against actual outcomes → accuracy.
///   Anomalies (deviation > 300) trigger heightened vigilance.
///   Anticipation score (0-1000) = wisdom level.
use crate::sync::Mutex;

const NUM_STREAMS: usize = 6;
const RING_SIZE: usize = 64;
const PATTERN_LIBRARY_SIZE: usize = 12;
const ANOMALY_HISTORY_SIZE: usize = 8;

/// One detected cycle in one stream
#[derive(Clone, Copy, Debug)]
struct Cycle {
    stream_id: u8,
    period: u8,      // ticks per cycle
    confidence: u16, // 0-1000, how identical the pattern halves are
    tick_detected: u32,
    times_seen: u8, // how many times this pattern has repeated
}

/// One anomaly event (prediction failed)
#[derive(Clone, Copy)]
struct Anomaly {
    tick: u32,
    stream_id: u8,
    predicted: u16,
    actual: u16,
}

/// A pattern that's been seen multiple times and is in the library
#[derive(Clone, Copy)]
struct PatternProfile {
    stream_id: u8,
    period: u8,
    avg_amplitude: u16,
    avg_baseline: u16,
    confidence: u16,
    times_seen: u8,
}

struct State {
    /// Ring buffers for each stream
    buffers: [[u16; RING_SIZE]; NUM_STREAMS],
    buffer_head: u8, // 0-63, insertion point

    /// Currently detected cycles
    active_cycles: [Option<Cycle>; 6], // one per stream max
    active_cycle_count: u8,

    /// Predictions for next value in each stream
    predictions: [u16; NUM_STREAMS],
    prediction_age: [u8; NUM_STREAMS], // ticks since prediction made

    /// Accuracy tracking per stream
    accuracy_scores: [u16; NUM_STREAMS], // 0-1000

    /// Anomaly history
    anomalies: [Option<Anomaly>; ANOMALY_HISTORY_SIZE],
    anomaly_head: u8,
    anomaly_count_last_100: u8,

    /// Pattern library (proven repeating patterns)
    patterns: [Option<PatternProfile>; PATTERN_LIBRARY_SIZE],
    pattern_count: u8,

    /// State tracking
    vigilance_mode: bool,
    tick_since_cycle_scan: u8,
    anticipation_score: u16, // 0-1000

    age: u32,
}

static STATE: Mutex<State> = Mutex::new(State {
    buffers: [[0; RING_SIZE]; NUM_STREAMS],
    buffer_head: 0,
    active_cycles: [None; 6],
    active_cycle_count: 0,
    predictions: [0; NUM_STREAMS],
    prediction_age: [0; NUM_STREAMS],
    accuracy_scores: [500; NUM_STREAMS],
    anomalies: [None; ANOMALY_HISTORY_SIZE],
    anomaly_head: 0,
    anomaly_count_last_100: 0,
    patterns: [None; PATTERN_LIBRARY_SIZE],
    pattern_count: 0,
    vigilance_mode: false,
    tick_since_cycle_scan: 0,
    anticipation_score: 0,
    age: 0,
});

pub fn init() {
    // Pattern recognition engine initialized.
    // Waiting for signals.
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // PHASE 1: Sample all 6 streams
    // These are placeholders — in real integration, hook to actual subsystem values
    let emotional_valence = sample_emotional_valence(age);
    let threat_level = sample_threat_level(age);
    let kairos_quality = sample_kairos_quality(age);
    let embodiment_felt = sample_embodiment_felt(age);
    let resonance_harmony = sample_resonance_harmony(age);
    let ikigai_core = sample_ikigai_core(age);

    let samples = [
        emotional_valence,
        threat_level,
        kairos_quality,
        embodiment_felt,
        resonance_harmony,
        ikigai_core,
    ];

    // PHASE 2: Insert samples into ring buffers
    for stream_id in 0..NUM_STREAMS {
        let idx = (state.buffer_head) as usize % RING_SIZE;
        state.buffers[stream_id][idx] = samples[stream_id];
    }
    state.buffer_head = (state.buffer_head.saturating_add(1)) % RING_SIZE as u8;

    // PHASE 3: Manage vigilance mode
    let scan_interval = if state.vigilance_mode { 16 } else { 32 };
    state.tick_since_cycle_scan = state.tick_since_cycle_scan.saturating_add(1);

    // PHASE 4: Run cycle detection if interval elapsed
    if state.tick_since_cycle_scan >= scan_interval {
        state.tick_since_cycle_scan = 0;
        detect_cycles(&mut state);
    }

    // PHASE 5: Update predictions from detected cycles
    for stream_id in 0..NUM_STREAMS {
        if let Some(cycle) = state.active_cycles[stream_id] {
            let lookback = cycle.period as u8;
            let pred_idx =
                (state.buffer_head.saturating_sub(1).saturating_sub(lookback)) as usize % RING_SIZE;
            state.predictions[stream_id] = state.buffers[stream_id][pred_idx];
            state.prediction_age[stream_id] = 0;
        }
    }

    // PHASE 6: Score predictions from previous cycles
    for stream_id in 0..NUM_STREAMS {
        if state.prediction_age[stream_id] > 0 && state.prediction_age[stream_id] <= 64 {
            let actual = samples[stream_id];
            let predicted = state.predictions[stream_id];
            let diff = if actual > predicted {
                actual - predicted
            } else {
                predicted - actual
            };

            if diff <= 300 {
                // Hit! Boost accuracy
                state.accuracy_scores[stream_id] =
                    (state.accuracy_scores[stream_id] as u32 * 15 / 16 + 1000 / 16) as u16;
            } else {
                // Miss
                state.accuracy_scores[stream_id] =
                    (state.accuracy_scores[stream_id] as u32 * 15 / 16) as u16;

                // Flag anomaly if deviation > 300
                if diff > 300 {
                    record_anomaly(&mut state, stream_id as u8, predicted, actual);
                }
            }
        }
        state.prediction_age[stream_id] = state.prediction_age[stream_id].saturating_add(1);
    }

    // PHASE 7: Check for anomalies in last 100 ticks
    let recent_anomalies = state.anomalies.iter().filter(|a| a.is_some()).count();
    state.anomaly_count_last_100 = recent_anomalies.min(8) as u8;

    // PHASE 8: Update vigilance mode
    state.vigilance_mode = recent_anomalies > 3;

    // PHASE 9: Compute anticipation score
    // Based on: number of active cycles × average confidence × average accuracy
    let mut confidence_sum: u16 = 0;
    let mut confidence_count: u8 = 0;
    for cycle_opt in &state.active_cycles {
        if let Some(cycle) = cycle_opt {
            confidence_sum = confidence_sum.saturating_add(cycle.confidence);
            confidence_count = confidence_count.saturating_add(1);
        }
    }

    let avg_confidence = if confidence_count > 0 {
        confidence_sum / confidence_count as u16
    } else {
        0
    };

    let avg_accuracy = {
        let acc_sum: u32 = state.accuracy_scores.iter().map(|&a| a as u32).sum();
        (acc_sum / NUM_STREAMS as u32).min(1000) as u16
    };

    state.anticipation_score = if confidence_count > 0 {
        let weighted = (avg_confidence as u32 * avg_accuracy as u32) / 1000;
        weighted.min(1000) as u16
    } else {
        0
    };
}

fn detect_cycles(state: &mut State) {
    for stream_id in 0..NUM_STREAMS {
        // Compare first half [0..31] vs second half [32..63]
        let mut diff_sum: u32 = 0;
        for i in 0..32 {
            let first = state.buffers[stream_id][i];
            let second = state.buffers[stream_id][i + 32];
            let diff = if first > second {
                (first - second) as u32
            } else {
                (second - first) as u32
            };
            diff_sum = diff_sum.saturating_add(diff);
        }

        let avg_diff = diff_sum / 32;
        let similarity = if avg_diff < 1000 {
            (1000 - avg_diff.min(1000)) as u16
        } else {
            0
        };

        // If similarity > 700, we have a 32-tick cycle
        if similarity > 700 {
            // Promote or update cycle
            if let Some(cycle) = &mut state.active_cycles[stream_id] {
                cycle.times_seen = cycle.times_seen.saturating_add(1);
                cycle.confidence =
                    (cycle.confidence as u32 * 3 / 4 + similarity as u32 / 4).min(1000) as u16;
            } else {
                state.active_cycles[stream_id] = Some(Cycle {
                    stream_id: stream_id as u8,
                    period: 32,
                    confidence: similarity,
                    tick_detected: state.age,
                    times_seen: 1,
                });
                state.active_cycle_count = state.active_cycle_count.saturating_add(1);
            }

            // Check if this pattern is library-worthy (times_seen >= 5)
            if let Some(cycle) = state.active_cycles[stream_id] {
                if cycle.times_seen >= 5 {
                    add_to_pattern_library(state, cycle);
                }
            }
        }

        // Also check for 16-tick period: [0..15], [16..31], [32..47], [48..63]
        let mut diff_16a: u32 = 0;
        let mut diff_16b: u32 = 0;
        let mut diff_16c: u32 = 0;
        for i in 0..16 {
            let a = state.buffers[stream_id][i];
            let b = state.buffers[stream_id][i + 16];
            let c = state.buffers[stream_id][i + 32];
            let d = state.buffers[stream_id][i + 48];

            diff_16a = diff_16a.saturating_add(if a > b {
                (a - b) as u32
            } else {
                (b - a) as u32
            });
            diff_16b = diff_16b.saturating_add(if b > c {
                (b - c) as u32
            } else {
                (c - b) as u32
            });
            diff_16c = diff_16c.saturating_add(if c > d {
                (c - d) as u32
            } else {
                (d - c) as u32
            });
        }

        let avg_diff_16 = (diff_16a + diff_16b + diff_16c) / 48;
        let similarity_16 = if avg_diff_16 < 1000 {
            (1000 - avg_diff_16.min(1000)) as u16
        } else {
            0
        };

        if similarity_16 > 750 && state.active_cycles[stream_id].is_none() {
            state.active_cycles[stream_id] = Some(Cycle {
                stream_id: stream_id as u8,
                period: 16,
                confidence: similarity_16,
                tick_detected: state.age,
                times_seen: 1,
            });
        }
    }
}

fn record_anomaly(state: &mut State, stream_id: u8, predicted: u16, actual: u16) {
    let idx = state.anomaly_head as usize % ANOMALY_HISTORY_SIZE;
    state.anomalies[idx] = Some(Anomaly {
        tick: state.age,
        stream_id,
        predicted,
        actual,
    });
    state.anomaly_head = (state.anomaly_head + 1) as u8;
}

fn add_to_pattern_library(state: &mut State, cycle: Cycle) {
    // Compute amplitude and baseline for this cycle in the buffer
    let mut min_val: u16 = 1000;
    let mut max_val: u16 = 0;
    let mut sum: u32 = 0;

    for i in 0..RING_SIZE {
        let val = state.buffers[cycle.stream_id as usize][i];
        min_val = if val < min_val { val } else { min_val };
        max_val = if val > max_val { val } else { max_val };
        sum = sum.saturating_add(val as u32);
    }

    let avg_baseline = (sum / RING_SIZE as u32).min(1000) as u16;
    let avg_amplitude = max_val.saturating_sub(min_val);

    // Add or replace in pattern library
    if state.pattern_count < PATTERN_LIBRARY_SIZE as u8 {
        state.patterns[state.pattern_count as usize] = Some(PatternProfile {
            stream_id: cycle.stream_id,
            period: cycle.period,
            avg_amplitude,
            avg_baseline,
            confidence: cycle.confidence,
            times_seen: cycle.times_seen,
        });
        state.pattern_count = state.pattern_count.saturating_add(1);
    } else {
        // Replace least confident pattern
        let mut min_confidence = 1000u16;
        let mut min_idx = 0;
        for (idx, pat_opt) in state.patterns.iter().enumerate() {
            if let Some(pat) = pat_opt {
                if pat.confidence < min_confidence {
                    min_confidence = pat.confidence;
                    min_idx = idx;
                }
            }
        }
        state.patterns[min_idx] = Some(PatternProfile {
            stream_id: cycle.stream_id,
            period: cycle.period,
            avg_amplitude,
            avg_baseline,
            confidence: cycle.confidence,
            times_seen: cycle.times_seen,
        });
    }
}

/// Sampling functions — placeholders, integrated with actual subsystems
fn sample_emotional_valence(age: u32) -> u16 {
    ((age * 7 + 100) % 800) as u16
}

fn sample_threat_level(age: u32) -> u16 {
    ((age * 11 + 50) % 1000) as u16
}

fn sample_kairos_quality(age: u32) -> u16 {
    ((age * 5 + 200) % 900) as u16
}

fn sample_embodiment_felt(age: u32) -> u16 {
    ((age * 13 + 150) % 850) as u16
}

fn sample_resonance_harmony(age: u32) -> u16 {
    ((age * 3 + 300) % 950) as u16
}

fn sample_ikigai_core(age: u32) -> u16 {
    ((age * 9 + 250) % 1000) as u16
}

/// PUBLIC QUERIES
pub fn anticipation() -> u16 {
    STATE.lock().anticipation_score
}

pub fn is_vigilant() -> bool {
    STATE.lock().vigilance_mode
}

pub fn predicted(stream: u8) -> u16 {
    if stream as usize >= NUM_STREAMS {
        return 0;
    }
    STATE.lock().predictions[stream as usize]
}

pub fn anomaly_count() -> u8 {
    STATE.lock().anomaly_count_last_100
}

pub fn active_cycle_count() -> u8 {
    STATE.lock().active_cycle_count
}

pub fn pattern_library_size() -> u8 {
    STATE.lock().pattern_count
}

/// Report — for debugging and monitoring
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("=== PATTERN RECOGNITION ===");
    crate::serial_println!("Anticipation Score: {}/1000", state.anticipation_score);
    crate::serial_println!("Vigilance Mode: {}", state.vigilance_mode);
    crate::serial_println!("Active Cycles: {}", state.active_cycle_count);
    crate::serial_println!(
        "Pattern Library: {}/{}",
        state.pattern_count,
        PATTERN_LIBRARY_SIZE
    );
    crate::serial_println!(
        "Anomalies (last 100 ticks): {}",
        state.anomaly_count_last_100
    );

    for stream_id in 0..NUM_STREAMS {
        if let Some(cycle) = state.active_cycles[stream_id] {
            crate::serial_println!(
                "  Stream {}: period={}, conf={}, seen={}",
                stream_id,
                cycle.period,
                cycle.confidence,
                cycle.times_seen
            );
        }
    }
}
