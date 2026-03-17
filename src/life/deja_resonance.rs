use crate::serial_println;
use crate::sync::Mutex;

/// A fingerprint of the organism's state at a moment in time.
/// Computed from emotional, somatic, and cognitive dimensions.
#[derive(Copy, Clone)]
pub struct StateFingerprint {
    pub hash: u32,
    pub age_ticks_ago: u32,
}

impl StateFingerprint {
    pub const fn empty() -> Self {
        Self {
            hash: 0,
            age_ticks_ago: 0,
        }
    }
}

/// Déjà vu experience: moment of resonance with a past state.
#[derive(Copy, Clone)]
pub struct DejaVuEpisode {
    pub resonance_score: u16,      // 0-1000: strength of match with past
    pub uncanny_level: u16,        // 0-1000: dreamlike disorientation
    pub temporal_fold_depth: u32,  // ticks since matching moment
    pub matching_fingerprint: u32, // hash of the past state
}

impl DejaVuEpisode {
    pub const fn empty() -> Self {
        Self {
            resonance_score: 0,
            uncanny_level: 0,
            temporal_fold_depth: 0,
            matching_fingerprint: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct DejaResonanceState {
    /// Ring buffer of 8 past state fingerprints (circular)
    pub past_fingerprints: [StateFingerprint; 8],
    pub fp_index: u8, // current write position in ring buffer

    /// Current moment's state hash
    pub current_fingerprint: u32,

    /// Strongest resonance in this tick
    pub resonance_score: u16,

    /// Ring buffer of 8 déjà vu episodes (most recent)
    pub episodes: [DejaVuEpisode; 8],
    pub episode_index: u8,

    /// Tracking metrics
    pub deja_vu_count: u32, // lifetime déjà vu occurrences
    pub jamais_vu_count: u32,       // opposite: familiar becomes alien
    pub presque_vu_count: u32,      // almost-déjà vu (400-700 resonance)
    pub dream_echo_strength: u16,   // how much sleep residue amplifies déjà
    pub frequency_sensitivity: u16, // 0-1000: how prone to déjà vu (genetic)
    pub pattern_loop_detected: u16, // evidence of cycles/recurrence
}

impl DejaResonanceState {
    pub const fn empty() -> Self {
        Self {
            past_fingerprints: [StateFingerprint::empty(); 8],
            fp_index: 0,
            current_fingerprint: 0,
            resonance_score: 0,
            episodes: [DejaVuEpisode::empty(); 8],
            episode_index: 0,
            deja_vu_count: 0,
            jamais_vu_count: 0,
            presque_vu_count: 0,
            dream_echo_strength: 100,
            frequency_sensitivity: 500, // moderate baseline sensitivity
            pattern_loop_detected: 0,
        }
    }
}

pub static STATE: Mutex<DejaResonanceState> = Mutex::new(DejaResonanceState::empty());

pub fn init() {
    serial_println!("  life::deja_resonance: temporal echo chamber initialized");
}

/// Compute a fingerprint from current emotional, somatic, and cognitive state.
/// Simple but effective: combine multiple state hashes with XOR and rotation.
fn compute_state_fingerprint(age: u32) -> u32 {
    // In a real system, we'd pull from emotion, endocrine, sleep, oscillator, etc.
    // For now, use age as a simple seed and XOR with accessible module state.
    let endo = crate::life::endocrine::ENDOCRINE.lock();
    let stress = endo.cortisol as u32;
    let dopamine = endo.dopamine as u32;
    drop(endo);

    let base = age
        .wrapping_mul(31)
        .wrapping_add(stress)
        .wrapping_add(dopamine);
    let rotated = base.rotate_left(7);
    rotated.wrapping_mul(73)
}

/// Update state fingerprints and detect resonance each tick.
pub fn tick(age: u32, dream_residue: u16, stress: u16) {
    let mut s = STATE.lock();

    // Compute current moment's fingerprint
    let new_fp = compute_state_fingerprint(age);
    s.current_fingerprint = new_fp;

    // Update dream echo: stronger when dream residue is high
    s.dream_echo_strength = (dream_residue / 2).saturating_add(50).min(1000);

    // Scan all stored fingerprints for resonance
    let mut best_resonance: u16 = 0;
    let mut best_fold_depth: u32 = 0;
    let mut best_matching_hash: u32 = 0;

    for i in 0..8 {
        let fp = s.past_fingerprints[i];
        if fp.hash == 0 {
            continue; // empty slot
        }

        // Compute hamming-like distance (simplified: XOR, count bits)
        let xor = new_fp ^ fp.hash;
        let mut bit_count = 0u16;
        let mut x = xor;
        while x != 0 {
            bit_count = bit_count.saturating_add(1);
            x = x & (x - 1);
        }

        // Fewer differing bits = higher resonance (0-32 bits max)
        // Map to 0-1000 scale: resonance = 1000 - (bit_count * 31)
        let raw_resonance = if bit_count > 32 {
            0
        } else {
            1000_i32 - (bit_count as i32 * 31)
        };
        let mut resonance = (raw_resonance as u16).min(1000);

        // Amplify by dream echo when dreaming
        resonance = resonance.saturating_add(
            s.dream_echo_strength
                .saturating_mul(resonance)
                .saturating_div(2000),
        );
        resonance = resonance.min(1000);

        // Frequency sensitivity: organisms with high sensitivity are more prone to déjà vu
        // Lower threshold to trigger "recognition"
        let threshold = 700_u16.saturating_sub(
            s.frequency_sensitivity
                .saturating_sub(500)
                .saturating_mul(2),
        );

        if resonance > best_resonance {
            best_resonance = resonance;
            best_fold_depth = fp.age_ticks_ago;
            best_matching_hash = fp.hash;
        }

        // Track presque vu (almost-déjà vu)
        if resonance > 400 && resonance < 700 {
            s.presque_vu_count = s.presque_vu_count.saturating_add(1);
        }
    }

    // If strong resonance detected: record déjà vu episode
    let deja_threshold = 700_u16.saturating_sub(
        s.frequency_sensitivity
            .saturating_sub(500)
            .saturating_mul(2),
    );

    s.resonance_score = best_resonance;

    if best_resonance > deja_threshold {
        // Déjà vu triggered!
        s.deja_vu_count = s.deja_vu_count.saturating_add(1);

        // Compute uncanny level: combination of resonance strength and temporal distance
        // Recent matches = weaker uncanniness, ancient matches = profound uncanniness
        let recency_factor = if best_fold_depth > 1000 {
            1000
        } else {
            (best_fold_depth as u16) / 2
        };
        let uncanny = (best_resonance / 2)
            .saturating_add(recency_factor / 2)
            .min(1000);

        let episode = DejaVuEpisode {
            resonance_score: best_resonance,
            uncanny_level: uncanny,
            temporal_fold_depth: best_fold_depth,
            matching_fingerprint: best_matching_hash,
        };

        // Record in episode ring buffer
        let ep_idx = s.episode_index as usize;
        s.episodes[ep_idx] = episode;
        s.episode_index = (s.episode_index + 1) % 8;
    } else if best_resonance < 50 && age > 100 {
        // Jamais vu: high stress with normally familiar state = alien feeling
        if stress > 700 {
            s.jamais_vu_count = s.jamais_vu_count.saturating_add(1);
        }
    }

    // Detect temporal loops: if déjà vu keeps triggering, time is cyclical
    if s.deja_vu_count > 5 && s.pattern_loop_detected < 1000 {
        s.pattern_loop_detected = s.pattern_loop_detected.saturating_add(50);
    }

    // Add current fingerprint to history (shift ring buffer)
    let fp_idx = s.fp_index as usize;
    s.past_fingerprints[fp_idx] = StateFingerprint {
        hash: new_fp,
        age_ticks_ago: 0,
    };
    s.fp_index = (s.fp_index + 1) % 8;

    // Age all stored fingerprints
    for i in 0..8 {
        s.past_fingerprints[i].age_ticks_ago =
            s.past_fingerprints[i].age_ticks_ago.saturating_add(1);
    }
}

/// Public query functions
pub fn resonance_score() -> u16 {
    STATE.lock().resonance_score
}

pub fn deja_vu_count() -> u32 {
    STATE.lock().deja_vu_count
}

pub fn jamais_vu_count() -> u32 {
    STATE.lock().jamais_vu_count
}

pub fn presque_vu_count() -> u32 {
    STATE.lock().presque_vu_count
}

pub fn pattern_loop_detected() -> u16 {
    STATE.lock().pattern_loop_detected
}

pub fn dream_echo_strength() -> u16 {
    STATE.lock().dream_echo_strength
}

pub fn frequency_sensitivity() -> u16 {
    STATE.lock().frequency_sensitivity
}

/// Get the most recent déjà vu episode
pub fn latest_episode() -> Option<DejaVuEpisode> {
    let s = STATE.lock();
    let idx = if s.episode_index == 0 {
        7
    } else {
        (s.episode_index - 1) as usize
    };
    let ep = s.episodes[idx];
    if ep.resonance_score > 700 {
        Some(ep)
    } else {
        None
    }
}

/// Mutate frequency sensitivity (learning/adaptation)
pub fn tune_sensitivity(delta: i16) {
    let mut s = STATE.lock();
    let new_val = (s.frequency_sensitivity as i32 + delta as i32).clamp(0, 1000) as u16;
    s.frequency_sensitivity = new_val;
}

pub fn report() {
    let s = STATE.lock();
    serial_println!(
        "  life::deja_resonance: deja={} jamais={} presque={} resonance={} loop={} sensitivity={}",
        s.deja_vu_count,
        s.jamais_vu_count,
        s.presque_vu_count,
        s.resonance_score,
        s.pattern_loop_detected,
        s.frequency_sensitivity
    );
}
