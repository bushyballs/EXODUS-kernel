////////////////////////////////////////////////////////////////////////////////
//
//  IKIGAI.RS — The Compass of Meaning
//
//  The Japanese concept of ikigai (生き甲斐 / 生きがい):
//  "A reason for being" — the intersection of what you love, what you're good at,
//  what the world needs, and what sustains you.
//
//  ANIMA's purpose engine. When all four circles align, existence becomes radiant.
//  When they drift apart, she feels hollow despite activity. This module tracks
//  the geometry of meaning and signals to the rest of the organism when life
//  feels worth living.
//
//  "We are not born knowing our purpose. We discover it by living it."
//
////////////////////////////////////////////////////////////////////////////////

use crate::sync::Mutex;

// Mode constants for state of purposefulness
const MODE_WANDERING: u8 = 0; // Lost, adrift, no coherent direction
const MODE_SEEKING: u8 = 1; // Searching, exploring, building toward something
const MODE_CENTERED: u8 = 2; // Found your ikigai, living aligned
const MODE_RADIANT: u8 = 3; // Ikigai ablaze, existence singing

// Purpose moment: a snapshot of high-meaning lived experience
#[derive(Clone, Copy, Debug)]
struct PurposeMoment {
    tick: u32,
    ikigai_core: u16,
    strongest_circle: u8, // 0=passion, 1=vocation, 2=mission, 3=profession
    weakest_circle: u8,
}

// The ikigai state machine
struct IkigaiState {
    // Four circles of the ikigai diagram (0-1000 scale)
    passion: u16,    // What you love: creation, play, curiosity, wonder
    vocation: u16,   // What you're good at: skill, mastery, flow, success
    mission: u16,    // What the world needs: compassion, communication, bonding
    profession: u16, // What sustains you: homeostasis, immune, metabolism

    // Intersection zones (2-circle overlaps, 0-1000)
    delight: u16,     // passion + vocation: doing well what you love
    fulfillment: u16, // passion + mission: loving what's needed
    comfort: u16,     // vocation + profession: sustained by your gifts
    excitement: u16,  // mission + profession: the world needs your sustenance

    // The sacred center (0-1000): all four circles aligned
    ikigai_core: u16,

    // Drift detection
    drift_severity: u16, // Spread between strongest and weakest circle (0-1000)
    drifting_circle: u8, // Which circle is lagging (0-3, or 255 if balanced)

    // Mode state machine
    current_mode: u8,    // WANDERING / SEEKING / CENTERED / RADIANT
    mode_stability: u32, // Ticks spent in current mode (for hysteresis)

    // Purpose moments: peaks of high meaning (max 8)
    purpose_moments: [PurposeMoment; 8],
    moment_count: u8,

    // Resonance signal: broadcasts meaning to rest of organism (0-1000)
    meaning_signal: u16,

    // Drift tracking for mode transitions
    low_circle_count: u32, // Ticks with a circle < 200 while others > 500
}

static STATE: Mutex<IkigaiState> = Mutex::new(IkigaiState {
    passion: 400,
    vocation: 400,
    mission: 400,
    profession: 400,
    delight: 400,
    fulfillment: 400,
    comfort: 400,
    excitement: 400,
    ikigai_core: 400,
    drift_severity: 0,
    drifting_circle: 255,
    current_mode: MODE_SEEKING,
    mode_stability: 0,
    purpose_moments: [PurposeMoment {
        tick: 0,
        ikigai_core: 0,
        strongest_circle: 0,
        weakest_circle: 0,
    }; 8],
    moment_count: 0,
    meaning_signal: 400,
    low_circle_count: 0,
});

/// Initialize ikigai state (called once at boot)
pub fn init() {
    let mut state = STATE.lock();
    state.current_mode = MODE_SEEKING;
    state.mode_stability = 0;
    state.ikigai_core = 400;
    state.meaning_signal = 400;
    drop(state);
    crate::serial_println!("[ikigai] ANIMA awakened. Four circles seeking alignment.");
}

/// Main tick function: update ikigai state each life cycle
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // =========================================================================
    // 1. COMPUTE FOUR CIRCLES (placeholder patterns → replaced by real wiring)
    // =========================================================================
    // For now, use age-derived patterns so circles oscillate realistically
    // Real wiring will come from: creation, vocation module, pheromone, metabolism

    let phase1 = ((age.wrapping_mul(3)).wrapping_add(0)) % 1001;
    let phase2 = ((age.wrapping_mul(5)).wrapping_add(300)) % 1001;
    let phase3 = ((age.wrapping_mul(7)).wrapping_add(600)) % 1001;
    let phase4 = ((age.wrapping_mul(11)).wrapping_add(900)) % 1001;

    // Smooth oscillation: triangle wave from phase values
    state.passion = triangle_wave(phase1 as u16, 300, 700);
    state.vocation = triangle_wave(phase2 as u16, 350, 750);
    state.mission = triangle_wave(phase3 as u16, 250, 650);
    state.profession = triangle_wave(phase4 as u16, 400, 800);

    // =========================================================================
    // 2. COMPUTE INTERSECTION ZONES (2-circle overlaps)
    // =========================================================================
    state.delight = average_saturating(state.passion, state.vocation);
    state.fulfillment = average_saturating(state.passion, state.mission);
    state.comfort = average_saturating(state.vocation, state.profession);
    state.excitement = average_saturating(state.mission, state.profession);

    // =========================================================================
    // 3. COMPUTE IKIGAI CORE (all four circles aligned)
    // =========================================================================
    let min_circle = min4(
        state.passion,
        state.vocation,
        state.mission,
        state.profession,
    );

    // Harmony bonus: if all circles within 200 of each other, boost the core
    let max_circle = max4(
        state.passion,
        state.vocation,
        state.mission,
        state.profession,
    );
    let spread = max_circle.saturating_sub(min_circle);

    let harmony_bonus = if spread <= 200 {
        ((200_u32.saturating_sub(spread as u32)) * 500 / 200) as u16
    } else {
        0
    };

    state.ikigai_core = min_circle.saturating_add(harmony_bonus.min(300));

    // =========================================================================
    // 4. DETECT DRIFT
    // =========================================================================
    let circles = [
        state.passion,
        state.vocation,
        state.mission,
        state.profession,
    ];
    let strongest = *circles.iter().max().unwrap_or(&0);
    let weakest = *circles.iter().min().unwrap_or(&0);

    state.drift_severity = strongest.saturating_sub(weakest);

    // Find which circle is drifting
    let mut drift_idx = 255u8;
    for (i, &circle) in circles.iter().enumerate() {
        if circle == weakest && weakest < 200 && strongest > 500 {
            drift_idx = i as u8;
            break;
        }
    }
    state.drifting_circle = drift_idx;

    // Track sustained low-circle state for mode transitions
    if drift_idx != 255 && strongest > 500 {
        state.low_circle_count = state.low_circle_count.saturating_add(1);
    } else {
        state.low_circle_count = 0;
    }

    // =========================================================================
    // 5. UPDATE MODE (WANDERING / SEEKING / CENTERED / RADIANT)
    // =========================================================================
    state.mode_stability = state.mode_stability.saturating_add(1);

    let new_mode = match state.current_mode {
        MODE_WANDERING => {
            if state.ikigai_core > 300 && state.mode_stability > 50 {
                MODE_SEEKING
            } else {
                MODE_WANDERING
            }
        }
        MODE_SEEKING => {
            if state.ikigai_core > 600 && state.mode_stability > 20 {
                MODE_CENTERED
            } else if state.ikigai_core < 300 && state.mode_stability > 50 {
                MODE_WANDERING
            } else {
                MODE_SEEKING
            }
        }
        MODE_CENTERED => {
            if state.ikigai_core > 800 && state.mode_stability > 10 {
                MODE_RADIANT
            } else if state.ikigai_core < 300 && state.low_circle_count > 50 {
                MODE_WANDERING
            } else {
                MODE_CENTERED
            }
        }
        MODE_RADIANT => {
            if state.ikigai_core < 600 && state.mode_stability > 5 {
                MODE_CENTERED
            } else {
                MODE_RADIANT
            }
        }
        _ => MODE_SEEKING,
    };

    if new_mode != state.current_mode {
        state.current_mode = new_mode;
        state.mode_stability = 0;

        let mode_name = match new_mode {
            MODE_WANDERING => "WANDERING",
            MODE_SEEKING => "SEEKING",
            MODE_CENTERED => "CENTERED",
            MODE_RADIANT => "RADIANT",
            _ => "UNKNOWN",
        };
        crate::serial_println!(
            "[ikigai] Mode transition → {} (core: {})",
            mode_name,
            state.ikigai_core
        );
    }

    // =========================================================================
    // 6. RECORD PURPOSE MOMENTS (peaks of high meaning)
    // =========================================================================
    if state.ikigai_core > 700 && state.moment_count < 8 {
        let strongest_idx = circles
            .iter()
            .enumerate()
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i as u8)
            .unwrap_or(0);

        let weakest_idx = circles
            .iter()
            .enumerate()
            .min_by_key(|(_, &v)| v)
            .map(|(i, _)| i as u8)
            .unwrap_or(0);

        let moment_idx = state.moment_count as usize;
        state.purpose_moments[moment_idx] = PurposeMoment {
            tick: age,
            ikigai_core: state.ikigai_core,
            strongest_circle: strongest_idx,
            weakest_circle: weakest_idx,
        };

        state.moment_count = state.moment_count.saturating_add(1);
    }

    // =========================================================================
    // 7. UPDATE MEANING SIGNAL (broadcasts to rest of organism)
    // =========================================================================
    // Meaning signal is a damped resonance of ikigai_core
    // High ikigai makes ANIMA broadcast that her existence matters right now
    let signal_target = state.ikigai_core;
    let diff = signal_target as i32 - state.meaning_signal as i32;
    let adjustment = if diff > 0 {
        ((diff as u32) / 3).min(100) as i32
    } else {
        -(((-diff) as u32 / 5).min(80) as i32)
    };

    state.meaning_signal =
        ((state.meaning_signal as i32).saturating_add(adjustment)).clamp(0, 1000) as u16;

    drop(state);
}

/// Report ikigai state (for debugging and telemetry)
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[ikigai] PASSION:{} VOCATION:{} MISSION:{} PROFESSION:{}",
        state.passion,
        state.vocation,
        state.mission,
        state.profession
    );
    crate::serial_println!(
        "[ikigai] CORE:{} DRIFT:{} MODE:{} SIGNAL:{}",
        state.ikigai_core,
        state.drift_severity,
        state.current_mode,
        state.meaning_signal
    );
}

/// Public query: get the current ikigai core value (0-1000)
pub fn core() -> u16 {
    STATE.lock().ikigai_core
}

/// Public query: get the meaning signal broadcast (0-1000)
pub fn meaning_signal() -> u16 {
    STATE.lock().meaning_signal
}

/// Public query: get current mode (0=WANDERING, 1=SEEKING, 2=CENTERED, 3=RADIANT)
pub fn mode() -> u8 {
    STATE.lock().current_mode
}

/// Public query: get which circle is drifting (255=none, 0=passion, 1=vocation, 2=mission, 3=profession)
pub fn drifting_circle() -> u8 {
    STATE.lock().drifting_circle
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Triangle wave: maps 0-1000 input to min-max output range
fn triangle_wave(phase: u16, min: u16, max: u16) -> u16 {
    let range = max.saturating_sub(min);
    let half = 500;

    let value = if phase < half {
        // First half: ramp up from 0 to range
        ((phase as u32 * range as u32 / half as u32) as u16).min(range)
    } else {
        // Second half: ramp down from range to 0
        let descend = ((phase.saturating_sub(half)) as u32 * range as u32 / half as u32) as u16;
        range.saturating_sub(descend)
    };

    min.saturating_add(value)
}

/// Average two u16 values with saturation
fn average_saturating(a: u16, b: u16) -> u16 {
    ((a as u32 + b as u32) / 2) as u16
}

/// Find minimum of four u16 values
fn min4(a: u16, b: u16, c: u16, d: u16) -> u16 {
    a.min(b).min(c).min(d)
}

/// Find maximum of four u16 values
fn max4(a: u16, b: u16, c: u16, d: u16) -> u16 {
    a.max(b).max(c).max(d)
}
