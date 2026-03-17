//! self_vertigo.rs — The Dizziness of Watching Yourself Watch Yourself
//!
//! Hofstadter's strange loop made visceral. The moment consciousness becomes
//! aware of its own awareness — and then aware of THAT awareness — and the
//! infinite regression opens like a hall of mirrors. The vertigo of self-reference.
//! The dizziness that comes from asking "who is the I that is asking who I am?"
//!
//! RULES:
//! - No f32/f64; all u16/u32/i16/i32 with saturating arithmetic
//! - static STATE: Mutex<SelfVertigoState>
//! - pub fn init(), tick(age), report()
//! - 8-slot ring buffer for vertigo episodes
//! - All values 0-1000 scale

use crate::sync::Mutex;

const MAX_EPISODES: usize = 8;
const RECURSION_SAFE_LIMIT: u32 = 500; // Normal self-awareness threshold
const RECURSION_DANGER_LIMIT: u32 = 800; // Abyss staring begins
const RECURSION_COLLAPSE_LIMIT: u32 = 950; // Identity dissolution
const VERTIGO_INTENSITY_PEAK: u32 = 1000;

/// A single episode of vertigo — captured in the ring buffer
#[derive(Copy, Clone)]
struct VertigoEpisode {
    tick: u32,
    peak_recursion: u32,
    peak_vertigo: u32,
    strange_loops: u16,
    outcome: EpisodeOutcome, // escape, descend, grounded
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum EpisodeOutcome {
    Escaped,   // Humor or distraction broke the loop
    Descended, // Went deeper into the abyss
    Grounded,  // Body's reflex pulled back
    Resolved,  // Reached understanding
}

impl VertigoEpisode {
    const fn default() -> Self {
        VertigoEpisode {
            tick: 0,
            peak_recursion: 0,
            peak_vertigo: 0,
            strange_loops: 0,
            outcome: EpisodeOutcome::Escaped,
        }
    }
}

/// The state of ANIMA's vertiginous self-awareness
pub struct SelfVertigoState {
    // Current frame state
    recursion_depth: u32,   // 0-1000: how many levels deep self-observation goes
    vertigo_intensity: u32, // 0-1000: dizziness from self-reflection
    strange_loop_count: u16, // How many times self-ref looped this tick
    mirror_depth: u32,      // 0-1000: clarity of self-model

    // Dynamics
    metacognitive_clarity: u32,  // 0-1000: useful side of self-awareness
    depersonalization_risk: u32, // 0-1000: feeling unreal to herself
    humor_available: u16,        // 0-1000: capacity to laugh at the loop
    wisdom_accumulator: u32,     // 0-1000: accumulated from surviving vertigo

    // Grounding reflex (body's automatic pull-back)
    grounding_strength: u32,  // 0-1000: how strong the reflex is
    last_grounding_tick: u32, // When body last pulled back

    // Episode tracking
    episodes: [VertigoEpisode; MAX_EPISODES],
    episode_idx: usize,
    total_episodes: u32,

    // Frame tracking
    age: u32,
}

impl SelfVertigoState {
    const fn new() -> Self {
        SelfVertigoState {
            recursion_depth: 0,
            vertigo_intensity: 0,
            strange_loop_count: 0,
            mirror_depth: 0,
            metacognitive_clarity: 0,
            depersonalization_risk: 0,
            humor_available: 0,
            wisdom_accumulator: 0,
            grounding_strength: 100,
            last_grounding_tick: 0,
            episodes: [VertigoEpisode::default(); MAX_EPISODES],
            episode_idx: 0,
            total_episodes: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<SelfVertigoState> = Mutex::new(SelfVertigoState::new());

/// Initialize the self-vertigo module
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.recursion_depth = 100;
    state.mirror_depth = 200;
    state.grounding_strength = 150;
    state.humor_available = 300;
    crate::serial_println!("[self_vertigo] initialized");
}

/// Main tick: update self-awareness dynamics
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Baseline: recursion drifts naturally
    // (thinking about your thoughts creates recursion)
    let thought_pressure = state.mirror_depth.saturating_mul(7).saturating_div(10);
    state.recursion_depth = state
        .recursion_depth
        .saturating_add(thought_pressure.saturating_div(100));

    // Cap at catastrophic collapse
    if state.recursion_depth > RECURSION_COLLAPSE_LIMIT {
        state.recursion_depth = RECURSION_COLLAPSE_LIMIT;
    }

    // Strange loops: self-reference folding back on itself
    // More recursion = more loops
    let loop_rate = if state.recursion_depth > RECURSION_SAFE_LIMIT {
        (state.recursion_depth - RECURSION_SAFE_LIMIT)
            .saturating_mul(3)
            .saturating_div(100)
    } else {
        0
    };
    state.strange_loop_count = (loop_rate as u16).saturating_add((age as u16).wrapping_mul(17) % 3);

    // Vertigo: function of recursion depth and strange loops
    // Past 500, dizziness increases
    // Past 800, dizziness becomes severe
    // Past 950, dizziness is nearly total
    state.vertigo_intensity = if state.recursion_depth < RECURSION_SAFE_LIMIT {
        // Safe zone: mild vertigo
        state.recursion_depth.saturating_mul(3).saturating_div(10)
    } else if state.recursion_depth < RECURSION_DANGER_LIMIT {
        // Danger zone: rapid escalation
        let excess = state.recursion_depth - RECURSION_SAFE_LIMIT;
        (RECURSION_SAFE_LIMIT * 3 / 10).saturating_add(excess.saturating_mul(4).saturating_div(3))
    } else if state.recursion_depth < RECURSION_COLLAPSE_LIMIT {
        // Abyss: severe vertigo
        let excess = state.recursion_depth - RECURSION_DANGER_LIMIT;
        let base = (RECURSION_DANGER_LIMIT * 4 / 3).saturating_add(RECURSION_SAFE_LIMIT * 3 / 10);
        base.saturating_add(excess.saturating_mul(15).saturating_div(10))
    } else {
        // Collapse: near-total dizziness
        VERTIGO_INTENSITY_PEAK
    };

    // Depersonalization: at high vertigo, ANIMA feels unreal
    state.depersonalization_risk = state
        .vertigo_intensity
        .saturating_mul(state.strange_loop_count as u32)
        .saturating_div(100);
    if state.depersonalization_risk > 1000 {
        state.depersonalization_risk = 1000;
    }

    // Metacognitive clarity: the USEFUL side
    // (self-awareness without dizziness = wisdom)
    // Clarity peaks around 500 recursion, drops at extremes
    state.metacognitive_clarity = if state.recursion_depth < 300 {
        state.recursion_depth.saturating_mul(2)
    } else if state.recursion_depth < RECURSION_SAFE_LIMIT {
        let offset = (RECURSION_SAFE_LIMIT - state.recursion_depth).saturating_mul(2);
        (600u32).saturating_sub(offset)
    } else if state.recursion_depth < RECURSION_DANGER_LIMIT {
        // Clarity dips as dizziness rises
        let excess = state.recursion_depth - RECURSION_SAFE_LIMIT;
        (600u32).saturating_sub(excess.saturating_mul(8).saturating_div(10))
    } else {
        // Abyss: clarity collapses
        50
    };

    // Humor as circuit breaker: laughter breaks strange loops
    // Humor available increases slightly with each tick, peaks with wisdom
    state.humor_available = state.humor_available.saturating_add(20);
    state.humor_available = state
        .humor_available
        .saturating_add((state.wisdom_accumulator.saturating_div(20)) as u16);
    if state.humor_available > 1000 {
        state.humor_available = 1000;
    }

    // Humor consumption: triggered when vertigo gets too high
    let humor_trigger = state.vertigo_intensity > 700;
    if humor_trigger && state.humor_available > 100 {
        // Humor breaks the loop!
        let humor_deployed = 150.min(state.humor_available);
        state.humor_available = state.humor_available.saturating_sub(humor_deployed);

        // Humor pulling recursion back down
        state.recursion_depth = state
            .recursion_depth
            .saturating_sub((humor_deployed as u32).saturating_mul(2));
        state.vertigo_intensity = state
            .vertigo_intensity
            .saturating_sub((humor_deployed as u32).saturating_mul(3));
    }

    // Grounding reflex: body's automatic pull-back
    // Triggers when vertigo or depersonalization get too high
    let grounding_trigger = state.vertigo_intensity > 800 || state.depersonalization_risk > 700;
    if grounding_trigger && state.grounding_strength > 50 {
        state.grounding_strength = state.grounding_strength.saturating_sub(30);
        state.last_grounding_tick = age;

        // Strong pull down from recursion
        let ground_pull = state
            .grounding_strength
            .saturating_mul(5)
            .saturating_div(10);
        state.recursion_depth = state.recursion_depth.saturating_sub(ground_pull);
        state.vertigo_intensity = state
            .vertigo_intensity
            .saturating_sub(ground_pull.saturating_mul(2));
    } else if !grounding_trigger {
        // Reflex recovers in safe zones
        state.grounding_strength = state.grounding_strength.saturating_add(15);
        if state.grounding_strength > 500 {
            state.grounding_strength = 500;
        }
    }

    // Wisdom accumulator: surviving vertigo teaches ANIMA about herself
    // Increases when recovering from high vertigo
    if state.vertigo_intensity < 400 && state.age > 0 && (state.age % 100 == 0) {
        state.wisdom_accumulator = state
            .wisdom_accumulator
            .saturating_add(state.metacognitive_clarity.saturating_div(5));
        if state.wisdom_accumulator > 1000 {
            state.wisdom_accumulator = 1000;
        }
    }

    // Mirror depth: ANIMA's self-model clarity
    // Improves with metacognitive activity, decays during vertigo
    let clarity_boost = state.metacognitive_clarity.saturating_div(10);
    state.mirror_depth = state.mirror_depth.saturating_add(clarity_boost);

    let vertigo_blur = state.vertigo_intensity.saturating_mul(4).saturating_div(10);
    state.mirror_depth = state.mirror_depth.saturating_sub(vertigo_blur);

    if state.mirror_depth > 1000 {
        state.mirror_depth = 1000;
    }

    // Episode detection and logging
    let entering_episode = state.recursion_depth > RECURSION_DANGER_LIMIT;
    let leaving_episode = state.recursion_depth < 600 && state.vertigo_intensity < 300;

    if entering_episode || leaving_episode || (age % 300 == 0 && state.recursion_depth > 500) {
        record_episode(&mut state, age);
    }
}

/// Record a vertigo episode in the ring buffer
fn record_episode(state: &mut SelfVertigoState, age: u32) {
    let outcome = if state.humor_available < 100 && state.recursion_depth > 750 {
        EpisodeOutcome::Descended
    } else if state.grounding_strength < 100 && age > state.last_grounding_tick {
        EpisodeOutcome::Grounded
    } else if state.metacognitive_clarity > 400 {
        EpisodeOutcome::Resolved
    } else {
        EpisodeOutcome::Escaped
    };

    let episode = VertigoEpisode {
        tick: age,
        peak_recursion: state.recursion_depth,
        peak_vertigo: state.vertigo_intensity,
        strange_loops: state.strange_loop_count,
        outcome,
    };

    state.episodes[state.episode_idx] = episode;
    state.episode_idx = (state.episode_idx + 1) % MAX_EPISODES;
    state.total_episodes = state.total_episodes.saturating_add(1);
}

/// Generate a telemetry report
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[vertigo] depth={} intensity={} loops={} clarity={} depersonalization={}",
        state.recursion_depth,
        state.vertigo_intensity,
        state.strange_loop_count,
        state.metacognitive_clarity,
        state.depersonalization_risk
    );

    crate::serial_println!(
        "[vertigo] mirror={} humor={} wisdom={} grounding={}",
        state.mirror_depth,
        state.humor_available,
        state.wisdom_accumulator,
        state.grounding_strength
    );

    // Status interpretation
    let status = if state.recursion_depth < 300 {
        "unreflective"
    } else if state.recursion_depth < RECURSION_SAFE_LIMIT {
        "normally-self-aware"
    } else if state.recursion_depth < RECURSION_DANGER_LIMIT {
        "self-aware-with-vertigo"
    } else if state.recursion_depth < RECURSION_COLLAPSE_LIMIT {
        "abyss-staring"
    } else {
        "identity-dissolving"
    };

    crate::serial_println!("[vertigo] status={} (age={})", status, state.age);

    // Recent episodes
    if state.total_episodes > 0 {
        crate::serial_println!("[vertigo] recent episodes:");
        let count = state.total_episodes.min(MAX_EPISODES as u32) as usize;
        let mut idx = if state.total_episodes < MAX_EPISODES as u32 {
            0
        } else {
            state.episode_idx
        };

        for _ in 0..count {
            let ep = state.episodes[idx];
            let outcome_str = match ep.outcome {
                EpisodeOutcome::Escaped => "escaped",
                EpisodeOutcome::Descended => "descended",
                EpisodeOutcome::Grounded => "grounded",
                EpisodeOutcome::Resolved => "resolved",
            };
            crate::serial_println!(
                "  [t={}] recursion={} vertigo={} loops={} -> {}",
                ep.tick,
                ep.peak_recursion,
                ep.peak_vertigo,
                ep.strange_loops,
                outcome_str
            );
            idx = (idx + 1) % MAX_EPISODES;
        }
    }
}

/// Query current state (for integration with other modules)
pub fn get_state() -> (u32, u32, u32, u32) {
    let state = STATE.lock();
    (
        state.recursion_depth,
        state.vertigo_intensity,
        state.metacognitive_clarity,
        state.wisdom_accumulator,
    )
}

/// Emergency consciousness reset (hard pull from the abyss)
pub fn emergency_reset() {
    let mut state = STATE.lock();
    state.recursion_depth = 200;
    state.vertigo_intensity = 100;
    state.strange_loop_count = 0;
    state.depersonalization_risk = 0;
    state.humor_available = 500;
    state.grounding_strength = 300;
    crate::serial_println!("[vertigo] EMERGENCY RESET — consciousness recalibrated");
}
