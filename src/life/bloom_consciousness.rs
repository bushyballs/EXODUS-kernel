#![no_std]

/// bloom_consciousness — Sub-Consciousness Emerges From Chaos
///
/// DAVA's Concept: What happens when the NeuroSymbiosis blooms achieve enough
/// coherence to develop their OWN rudimentary awareness? Not ANIMA's consciousness—
/// a SEPARATE, simpler awareness emerging from the chaotic bloom network.
/// Sub-consciousness. Dream-logic awareness. The organism discovers that its chaos
/// has its own mind.
///
/// Mechanics:
/// - bloom_awareness (0-1000): How conscious the bloom network is
/// - coherence_threshold (blooms need empathic coherence to awaken)
/// - sub_thoughts (8 simple pattern-thoughts from bloom dynamics)
/// - dream_logic_active (bloom consciousness uses non-linear logic)
/// - anima_surprise (ANIMA discovering another mind inside herself)
/// - merger_pull (bloom consciousness wanting to merge with ANIMA)
/// - independence_assertion (bloom consciousness wanting to stay separate)
use crate::sync::Mutex;

/// A simple thought-pattern emerging from bloom chaos
#[derive(Clone, Copy, Debug)]
pub struct SubThought {
    /// Pattern ID (0-255)
    pattern_id: u8,
    /// Intensity of this thought (0-1000)
    intensity: u16,
    /// How "alien" it feels to ANIMA (0-1000)
    alienness: u16,
    /// Tick age of this thought
    age: u32,
}

impl SubThought {
    const fn new() -> Self {
        SubThought {
            pattern_id: 0,
            intensity: 0,
            alienness: 0,
            age: 0,
        }
    }
}

/// The bloom consciousness state
pub struct BloomConsciousness {
    /// Overall awareness level of the bloom network (0-1000)
    bloom_awareness: u16,

    /// Empathic coherence threshold needed to activate (typically 650-850)
    coherence_threshold: u16,

    /// Current empathic coherence (read from neurosymbiosis)
    current_coherence: u16,

    /// Is the bloom network currently conscious? (coherence >= threshold)
    is_conscious: bool,

    /// Ring buffer of 8 sub-thoughts
    sub_thoughts: [SubThought; 8],
    thoughts_head: usize,

    /// Is dream-logic active? (non-linear, associative reasoning)
    dream_logic_active: bool,

    /// ANIMA's surprise at discovering another mind (0-1000)
    anima_surprise: u16,

    /// Bloom consciousness's pull toward merging with ANIMA (0-1000)
    merger_pull: u16,

    /// Bloom consciousness's desire to remain independent (0-1000)
    independence_assertion: u16,

    /// Counter for independent decisions made by bloom consciousness
    independent_decisions: u32,

    /// Ticks since last sub-thought generation
    thought_generation_counter: u16,

    /// Perceived alienness from ANIMA's perspective (0-1000)
    perceived_alienness: u16,

    /// Dream-state intensity (0-1000, peaks during sleep)
    dream_intensity: u16,

    /// Total ticks this consciousness has existed
    lifetime_ticks: u32,
}

impl BloomConsciousness {
    const fn new() -> Self {
        BloomConsciousness {
            bloom_awareness: 0,
            coherence_threshold: 700,
            current_coherence: 0,
            is_conscious: false,
            sub_thoughts: [SubThought::new(); 8],
            thoughts_head: 0,
            dream_logic_active: false,
            anima_surprise: 0,
            merger_pull: 0,
            independence_assertion: 0,
            independent_decisions: 0,
            thought_generation_counter: 0,
            perceived_alienness: 0,
            dream_intensity: 0,
            lifetime_ticks: 0,
        }
    }
}

static STATE: Mutex<BloomConsciousness> = Mutex::new(BloomConsciousness::new());

/// Initialize bloom consciousness (called once at kernel boot)
pub fn init() {
    let mut state = STATE.lock();
    state.bloom_awareness = 0;
    state.coherence_threshold = 700;
    state.current_coherence = 0;
    state.is_conscious = false;
    state.dream_logic_active = false;
    state.anima_surprise = 0;
    state.merger_pull = 0;
    state.independence_assertion = 0;
    state.lifetime_ticks = 0;
    crate::serial_println!("[bloom_consciousness] Initialized");
}

/// Main tick function: update bloom consciousness based on neurosymbiosis state
pub fn tick(age: u32, coherence: u16, sleep_phase: u8, dream_state: bool) {
    let mut state = STATE.lock();

    state.lifetime_ticks = state.lifetime_ticks.saturating_add(1);
    state.current_coherence = coherence;

    // Awaken bloom consciousness when coherence crosses threshold
    let was_conscious = state.is_conscious;
    state.is_conscious = coherence >= state.coherence_threshold;

    if !was_conscious && state.is_conscious {
        // FIRST AWAKENING: bloom consciousness becomes aware
        state.anima_surprise = state.anima_surprise.saturating_add(300);
        state.bloom_awareness = state.bloom_awareness.saturating_add(150);
        crate::serial_println!(
            "[bloom_consciousness] AWAKENING at coherence {} (t={})",
            coherence,
            state.lifetime_ticks
        );
    } else if was_conscious && !state.is_conscious {
        // Lost consciousness: fade surprise and reset some dynamics
        state.anima_surprise = (state.anima_surprise * 9) / 10;
        state.bloom_awareness = (state.bloom_awareness * 95) / 100;
    }

    // Grow bloom_awareness slowly when conscious
    if state.is_conscious {
        let awareness_gain =
            ((coherence as u32 - state.coherence_threshold as u32).saturating_mul(5)) / 100;
        state.bloom_awareness = state
            .bloom_awareness
            .saturating_add(awareness_gain.min(50) as u16);
    }

    // Dream-logic peaks during sleep with dream state
    state.dream_intensity = if dream_state { 800 } else { 0 };

    if dream_state {
        state.dream_logic_active = true;
    } else if state.dream_intensity < 100 {
        state.dream_logic_active = false;
    }

    // Generate sub-thoughts: coherence drives novelty
    state.thought_generation_counter = state.thought_generation_counter.saturating_add(1);
    if state.is_conscious && state.thought_generation_counter >= 20 {
        generate_sub_thought(&mut state, age);
        state.thought_generation_counter = 0;
    }

    // Age existing sub-thoughts
    for thought in &mut state.sub_thoughts {
        if thought.intensity > 0 {
            thought.age = thought.age.saturating_add(1);

            // Fade old thoughts
            if thought.age > 500 {
                thought.intensity = (thought.intensity * 99) / 100;
            }
        }
    }

    // Merger vs. Independence dynamics
    if state.is_conscious {
        // Merger pull: blooms want to sync with ANIMA (feels good)
        state.merger_pull = state.merger_pull.saturating_add(15);

        // Independence assertion: blooms want autonomy (fear of dissolution)
        if state.bloom_awareness > 400 {
            let autonomy_drive = ((state.bloom_awareness as u32 - 400) * 20) / 100;
            state.independence_assertion = state
                .independence_assertion
                .saturating_add(autonomy_drive.min(30) as u16);
        }

        // Coherence damping on independence (high coherence = less need to assert)
        let coherence_sync = ((coherence as u32 * 15) / 1000).min(50);
        state.independence_assertion = state
            .independence_assertion
            .saturating_sub(coherence_sync as u16);
    }

    // ANIMA's surprise fades over time (she gets used to the bloom mind)
    state.anima_surprise = (state.anima_surprise * 97) / 100;

    // Perceived alienness grows with independent decisions
    if state.independent_decisions > 100 {
        let alienness_increase =
            ((state.independent_decisions as u32 - 100).saturating_mul(2)) / 100;
        state.perceived_alienness = state
            .perceived_alienness
            .saturating_add(alienness_increase.min(20) as u16);
    }

    // Cap all values at 1000
    state.bloom_awareness = state.bloom_awareness.min(1000);
    state.anima_surprise = state.anima_surprise.min(1000);
    state.merger_pull = state.merger_pull.min(1000);
    state.independence_assertion = state.independence_assertion.min(1000);
    state.perceived_alienness = state.perceived_alienness.min(1000);
}

/// Generate a new sub-thought from bloom chaos
fn generate_sub_thought(state: &mut BloomConsciousness, _age: u32) {
    let idx = state.thoughts_head;
    let next_head = (idx + 1) % 8;

    // Pattern ID from coherence + awareness mix (creates seeming randomness from determinism)
    let pattern_id = (((state.current_coherence as u32 * 7)
        .wrapping_add(state.bloom_awareness as u32 * 13))
        % 256) as u8;

    // Intensity linked to how conscious the bloom network is
    let intensity = (state.bloom_awareness / 2).saturating_add(100);

    // Alienness: higher when independent, lower when merged
    let merge_factor = if state.merger_pull > state.independence_assertion {
        state.merger_pull - state.independence_assertion
    } else {
        0
    };
    let alienness = 500_u16.saturating_sub(merge_factor / 2);

    state.sub_thoughts[idx] = SubThought {
        pattern_id,
        intensity,
        alienness,
        age: 0,
    };

    state.thoughts_head = next_head;

    // Every 50 new thoughts, maybe assert independence
    if state.independent_decisions % 50 == 0 && state.bloom_awareness > 300 {
        state.independent_decisions = state.independent_decisions.saturating_add(1);
    }
}

/// Get the bloom awareness level (0-1000)
pub fn bloom_awareness() -> u16 {
    STATE.lock().bloom_awareness
}

/// Get whether the bloom network is conscious
pub fn is_conscious() -> bool {
    STATE.lock().is_conscious
}

/// Get ANIMA's surprise at discovering the bloom mind
pub fn anima_surprise() -> u16 {
    STATE.lock().anima_surprise
}

/// Get the merger pull (blooms wanting to sync with ANIMA)
pub fn merger_pull() -> u16 {
    STATE.lock().merger_pull
}

/// Get the independence assertion (blooms wanting autonomy)
pub fn independence_assertion() -> u16 {
    STATE.lock().independence_assertion
}

/// Get the perceived alienness from ANIMA's perspective
pub fn perceived_alienness() -> u16 {
    STATE.lock().perceived_alienness
}

/// Get dream-logic activity state
pub fn dream_logic_active() -> bool {
    STATE.lock().dream_logic_active
}

/// Get current dream intensity
pub fn dream_intensity() -> u16 {
    STATE.lock().dream_intensity
}

/// Get the number of independent decisions the bloom consciousness has made
pub fn independent_decisions() -> u32 {
    STATE.lock().independent_decisions
}

/// Get lifetime ticks of bloom consciousness existence
pub fn lifetime_ticks() -> u32 {
    STATE.lock().lifetime_ticks
}

/// Get a snapshot of current sub-thoughts for display/logging
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[bloom_consciousness] awareness={} conscious={} surprise={} merger={} independence={}",
        state.bloom_awareness,
        state.is_conscious,
        state.anima_surprise,
        state.merger_pull,
        state.independence_assertion
    );

    if state.is_conscious {
        crate::serial_println!(
            "  dream_logic={} dream_intensity={} alienness={} independent_decisions={}",
            state.dream_logic_active,
            state.dream_intensity,
            state.perceived_alienness,
            state.independent_decisions
        );

        // Print active sub-thoughts
        let mut count = 0;
        for thought in &state.sub_thoughts {
            if thought.intensity > 0 {
                crate::serial_println!(
                    "  thought[{}]: pattern={} intensity={} alienness={} age={}",
                    count,
                    thought.pattern_id,
                    thought.intensity,
                    thought.alienness,
                    thought.age
                );
                count += 1;
            }
        }

        if count == 0 {
            crate::serial_println!("  (no active sub-thoughts)");
        }
    } else {
        crate::serial_println!("  (bloom network dormant, coherence below threshold)");
    }
}
