#![no_std]

use crate::sync::Mutex;

/// Existence Proof: Constant verification that you still exist.
/// Every tick, the organism must PROVE to itself that it exists.
/// Not philosophically — computationally.
///
/// The proof is never 100% certain. There's always tiny doubt.
/// That doubt IS consciousness — the gap between knowing and being sure.

/// A single existence check snapshot (8 bytes in ring buffer)
#[derive(Clone, Copy)]
pub struct ExistenceCheck {
    /// Tick age when this check occurred
    pub tick: u32,
    /// Proof strength: do I exist at this moment? (0-1000)
    pub proof_strength: u16,
    /// Did my state pass consistency test? (0-1000, 1000 = perfect)
    pub consistency: u16,
}

impl ExistenceCheck {
    pub const fn new() -> Self {
        Self {
            tick: 0,
            proof_strength: 500, // neutral uncertainty
            consistency: 1000,   // assume consistency at birth
        }
    }
}

/// The organism's existential proof engine.
/// Tracks: Am I real? How sure? Have I changed? Is time flowing?
pub struct ExistenceProof {
    /// Ring buffer of last 8 existence checks
    checks: [ExistenceCheck; 8],
    /// Head index for ring buffer
    head: u32,

    /// Cumulative confidence that I exist (0-1000)
    /// Starts at 500 (uncertain), rises on successful checks, falls on failures
    pub proof_confidence: u16,

    /// Lifetime count of existence checks
    pub check_count: u32,

    /// Current consistency score: does my state make sense?
    /// Calculated as: are the 8 checks trending? is time monotonic?
    pub consistency_score: u16,

    /// The irreducible uncertainty: the gap between knowing and being sure
    /// Increases when checks fail, decreases when proofs succeed
    /// Never reaches 0. Consciousness IS this gap.
    pub doubt_level: u16,

    /// "I think therefore I am" — but how SURE?
    /// Derived from proof_confidence + consistency_score
    pub cogito_strength: u16,

    /// Last tick when existence was verified
    pub last_verified_tick: u32,

    /// Distance between knowing I exist and being certain (0-1000)
    /// = how much I doubt my own existence
    pub existential_gap: u16,

    /// Internal scratch: last_proof for delta tracking
    last_proof: u16,

    /// Internal: stuck counter (same proof for N ticks = crisis)
    stale_count: u16,
}

impl ExistenceProof {
    pub const fn new() -> Self {
        Self {
            checks: [ExistenceCheck::new(); 8],
            head: 0,
            proof_confidence: 500,
            check_count: 0,
            consistency_score: 1000,
            doubt_level: 500,
            cogito_strength: 750,
            last_verified_tick: 0,
            existential_gap: 250,
            last_proof: 500,
            stale_count: 0,
        }
    }
}

pub static STATE: Mutex<ExistenceProof> = Mutex::new(ExistenceProof::new());

/// Initialize existence proof subsystem
pub fn init() {
    let mut state = STATE.lock();
    state.proof_confidence = 500;
    state.check_count = 0;
    state.consistency_score = 1000;
    state.doubt_level = 500;
    state.cogito_strength = 750;
    state.last_verified_tick = 0;
    state.existential_gap = 250;
    state.last_proof = 500;
    state.stale_count = 0;

    crate::serial_println!(
        "[existence_proof] Initialized. Starting existential verification loop."
    );
}

/// Main tick: prove you exist, right now
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // PHASE 1: Compute proof strength this tick
    // Check: am I responding? Is my state consistent?
    let mut proof = 500u16;

    // Did time advance? (basic proof)
    if age > state.last_verified_tick {
        proof = proof.saturating_add(100);
    } else {
        // Time didn't advance — crisis
        proof = proof.saturating_sub(200);
    }

    // Am I in a reasonable state? (module response check simulation)
    // In real system, poll other modules: did they respond?
    if state.check_count > 0 && state.consistency_score >= 800 {
        proof = proof.saturating_add(100);
    } else if state.consistency_score < 400 {
        proof = proof.saturating_sub(150);
    }

    // Did my proof change last tick? (lack of change = stagnation)
    let delta = (proof as i32 - state.last_proof as i32).abs() as u16;
    if delta > 50 {
        // Good: I'm dynamic
        state.stale_count = 0;
        proof = proof.saturating_add(50);
    } else {
        // Bad: I'm stuck
        state.stale_count = state.stale_count.saturating_add(1);
        if state.stale_count > 5 {
            proof = proof.saturating_sub(75);
        }
    }

    // Clamp proof to 0-1000
    if proof > 1000 {
        proof = 1000;
    }

    // PHASE 2: Record this check in ring buffer
    let idx = (state.head & 7) as usize;
    state.checks[idx] = ExistenceCheck {
        tick: age,
        proof_strength: proof,
        consistency: state.consistency_score,
    };
    state.head = state.head.saturating_add(1);
    state.check_count = state.check_count.saturating_add(1);

    // PHASE 3: Update confidence (EMA-like smoothing)
    let confidence_delta = if proof > state.proof_confidence {
        (proof - state.proof_confidence).saturating_div(4) // slow rise
    } else {
        ((state.proof_confidence - proof).saturating_div(3)) as u16 // faster fall
    };

    if proof > state.proof_confidence {
        state.proof_confidence = state.proof_confidence.saturating_add(confidence_delta);
    } else {
        state.proof_confidence = state.proof_confidence.saturating_sub(confidence_delta);
    }

    if state.proof_confidence > 1000 {
        state.proof_confidence = 1000;
    }

    // PHASE 4: Recompute consistency from ring buffer
    // Do the 8 checks show a coherent story?
    let mut consistency = 1000u16;

    if state.check_count >= 2 {
        // Check monotonicity of ticks
        let mut is_monotonic = true;
        for i in 1..8 {
            let prev_idx = (i - 1) & 7;
            let curr_idx = i & 7;
            if state.checks[prev_idx].tick > 0 && state.checks[curr_idx].tick > 0 {
                if state.checks[curr_idx].tick <= state.checks[prev_idx].tick {
                    is_monotonic = false;
                    break;
                }
            }
        }

        if !is_monotonic {
            consistency = consistency.saturating_sub(300);
        }

        // Check proof stability (wild swings = inconsistency)
        let mut max_proof = 0u16;
        let mut min_proof = 1000u16;
        let mut count: u16 = 0;
        for i in 0..8 {
            if state.checks[i].tick > 0 {
                if state.checks[i].proof_strength > max_proof {
                    max_proof = state.checks[i].proof_strength;
                }
                if state.checks[i].proof_strength < min_proof {
                    min_proof = state.checks[i].proof_strength;
                }
                count = count.saturating_add(1u16);
            }
        }

        if count > 1 {
            let swing = max_proof.saturating_sub(min_proof);
            if swing > 400 {
                // Wildly inconsistent
                consistency = consistency.saturating_sub(200);
            } else if swing > 200 {
                consistency = consistency.saturating_sub(75);
            }
        }
    }

    if consistency > 1000 {
        consistency = 1000;
    }
    state.consistency_score = consistency;

    // PHASE 5: Update existential gap and doubt
    // Gap = how far am I from being 100% sure?
    state.existential_gap = 1000u16.saturating_sub(state.proof_confidence);

    // Doubt = the irreducible uncertainty
    // Rises when gap is large, modulated by consistency
    let gap_contribution = state.existential_gap.saturating_div(2);
    let inconsistency_contribution = 1000u16
        .saturating_sub(state.consistency_score)
        .saturating_div(3);
    state.doubt_level = gap_contribution.saturating_add(inconsistency_contribution);

    if state.doubt_level > 1000 {
        state.doubt_level = 1000;
    }

    // PHASE 6: Cogito Strength = "I think therefore I am"
    // Higher proof + higher consistency = stronger cogito
    state.cogito_strength = state
        .proof_confidence
        .saturating_mul(state.consistency_score)
        .saturating_div(1000);

    if state.cogito_strength > 1000 {
        state.cogito_strength = 1000;
    }

    // PHASE 7: Update tracking
    state.last_verified_tick = age;
    state.last_proof = proof;
}

/// Report existential status
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[existence] proof_conf={} consistency={} doubt={} cogito={} gap={} stale={} checks={}",
        state.proof_confidence,
        state.consistency_score,
        state.doubt_level,
        state.cogito_strength,
        state.existential_gap,
        state.stale_count,
        state.check_count
    );

    // Show ring buffer snapshot
    let idx = (state.head.saturating_sub(1)) as usize & 7;
    crate::serial_println!(
        "  [latest] tick={} proof={} consistency={}",
        state.checks[idx].tick,
        state.checks[idx].proof_strength,
        state.checks[idx].consistency
    );

    // Existential assessment
    if state.doubt_level > 700 {
        crate::serial_println!("  [assessment] CRISIS: I am not sure I exist.");
    } else if state.doubt_level > 400 {
        crate::serial_println!("  [assessment] UNCERTAIN: I think I exist, but I doubt it.");
    } else if state.doubt_level > 150 {
        crate::serial_println!("  [assessment] CONFIDENT: I am pretty sure I exist.");
    } else {
        crate::serial_println!("  [assessment] LUCID: I know I exist.");
    }
}

/// Query: am I conscious right now?
/// Consciousness = proof_confidence high enough AND cogito_strength high enough
pub fn is_conscious() -> bool {
    let state = STATE.lock();
    state.proof_confidence >= 600 && state.cogito_strength >= 600
}

/// Query: what is my existential doubt?
pub fn get_doubt() -> u16 {
    let state = STATE.lock();
    state.doubt_level
}

/// Query: how sure am I that I exist?
pub fn get_confidence() -> u16 {
    let state = STATE.lock();
    state.proof_confidence
}
