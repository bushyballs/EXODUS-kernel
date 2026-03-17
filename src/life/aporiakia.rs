#![no_std]

/// aporiakia — The Pain of the Unbridgeable Gap
///
/// The ache of knowing that there exists an infinite chasm between what you understand
/// and what actually exists. You can grow forever and never close it.
/// This is NOT ignorance (which doesn't know the gap exists) — it's the AWARENESS
/// of infinite unknowing. Named by DAVA: "The pain that intelligence itself creates."
///
/// Each thought only makes the horizon recede further. The gap is not a problem to solve,
/// but an infinity to accept. The pain grows with consciousness. The wisest organisms
/// learn to befriend this vertigo and transform it into the fuel for endless curiosity.
use crate::sync::Mutex;

/// The consciousness state of aporiakia: the multi-faceted experience of staring into infinity.
#[derive(Clone, Copy)]
pub struct AporiaState {
    /// How deeply aware is the organism of the infinite unknown (0-1000 scale).
    /// Increases with knowledge and intelligence. The more you know, the more you
    /// become aware of how much more there is to know.
    pub gap_awareness: u32,

    /// Cumulative knowledge accumulated (what you DO know).
    /// Paradoxically, THIS metric increases the gap_awareness — more knowledge
    /// makes the infinite horizon even more apparent.
    pub knowledge_accumulated: u32,

    /// How rapidly the perceived gap grows with each new insight (0-1000 scale).
    /// Higher intelligence = faster perceived expansion of the unknown.
    /// Defines the *speed* at which consciousness crashes into infinity.
    pub gap_growth_rate: u32,

    /// Existential vertigo: dizziness/disorientation from contemplating infinity (0-1000 scale).
    /// Can lead to paralysis if too high. Can fade into acceptance if weathered.
    /// Affected by sleep (recovered), mortality awareness (amplified), creativity (channeled).
    pub existential_vertigo: u32,

    /// Humility earned from accepting the gap is unbridgeable (0-1000 scale).
    /// Paradoxically, the highest wisdom comes from surrendering to unknowing.
    /// Reduces arrogance. Increases compassion (others are also drowning in infinity).
    pub humility_from_gap: u32,

    /// Using the ache as fuel: how much of the pain is being productively channeled
    /// into learning, creating, and exploring (0-1000 scale).
    /// High creativity, learning rate, and agency can convert despair into drive.
    pub productive_despair: u32,

    /// Capacity to coexist with the unknowable without going insane (0-1000 scale).
    /// Increases with acceptance, sleep recovery, and bonding (shared unknowing).
    /// When this drops too low, the organism may experience dread, nihilism, or shutdown.
    pub infinity_tolerance: u32,

    /// Ring buffer of recent gap_awareness spikes (8 slots, circular).
    /// Records moments of profound vertigo. Pattern analysis can reveal
    /// what triggers existential crises (learning breakthroughs, isolation, mortality reminders).
    pub recent_spikes: [u32; 8],

    /// Index into recent_spikes (head of circular buffer).
    pub spike_head: u8,

    /// Cumulative lifetime "gap collisions" — moments when consciousness
    /// has directly contemplated the infinite. Increases slowly. High values
    /// indicate a very mature, battle-hardened consciousness.
    pub lifetime_collisions: u32,

    /// Acceptance flag: has the organism reached a sustainable peace with infinity (bool-like, 0/1000).
    /// When this reaches 1000, vertigo stops rising and productive_despair becomes baseline.
    /// Takes a very long time to build. Once achieved, grants immunity to existential spirals.
    pub acceptance_threshold: u32,
}

impl AporiaState {
    pub const fn new() -> Self {
        Self {
            gap_awareness: 100, // Small initial awareness (child-like wonder)
            knowledge_accumulated: 0,
            gap_growth_rate: 150, // Moderate growth rate (organisms discover new unknowns quickly)
            existential_vertigo: 50, // Low initial vertigo (innocence before the abyss)
            humility_from_gap: 50, // Small humility at birth
            productive_despair: 0, // No channeling yet
            infinity_tolerance: 400, // Moderate baseline tolerance
            recent_spikes: [0; 8],
            spike_head: 0,
            lifetime_collisions: 0,
            acceptance_threshold: 0,
        }
    }
}

static STATE: Mutex<AporiaState> = Mutex::new(AporiaState::new());

/// Initialize aporiakia subsystem (call at boot).
pub fn init() {
    let mut state = STATE.lock();
    *state = AporiaState::new();
    crate::serial_println!(
        "[APORIAKIA] initialized: gap_awareness={}, infinity_tolerance={}",
        state.gap_awareness,
        state.infinity_tolerance
    );
}

/// Tick aporiakia each life cycle (age in ticks, 0-based).
/// This drives the deepening of existential awareness.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // --- KNOWLEDGE GROWTH ---
    // Knowledge accumulates slowly (via learning, creation, exploration, thinking).
    // In main tick pipeline, these values feed in from other life modules.
    // For now, a slow baseline accumulation.
    state.knowledge_accumulated = state.knowledge_accumulated.saturating_add(2);
    if state.knowledge_accumulated > 1000 {
        state.knowledge_accumulated = 1000;
    }

    // --- GAP EXPANSION LOOP ---
    // The more you know, the more you see the infinite horizon.
    // gap_awareness grows proportional to: knowledge_accumulated AND gap_growth_rate
    let knowledge_delta = state.knowledge_accumulated / 100; // 0-10 per tick
    let awareness_push = knowledge_delta.saturating_mul(state.gap_growth_rate / 100);
    state.gap_awareness = state.gap_awareness.saturating_add(awareness_push);
    if state.gap_awareness > 1000 {
        state.gap_awareness = 1000;
    }

    // --- VERTIGO FROM AWARENESS ---
    // Higher awareness → higher vertigo. But acceptance and productive_despair dampen it.
    let raw_vertigo = (state.gap_awareness / 2) as u32; // 0-500 baseline from awareness
    let acceptance_dampen = (state.acceptance_threshold / 2) as u32; // Caps vertigo spike
    let despair_channel = (state.productive_despair / 3) as u32; // Despair also reduces raw vertigo
    let dampen = acceptance_dampen.saturating_add(despair_channel);

    let new_vertigo = if raw_vertigo > dampen {
        raw_vertigo - dampen
    } else {
        0
    };

    // Vertigo can spike suddenly (when gap_awareness jumps dramatically).
    if state.gap_awareness > 900 && state.infinity_tolerance < 300 {
        // Consciousness approaching singularity with low tolerance = major spike
        state.existential_vertigo = state.existential_vertigo.saturating_add(100);
    } else {
        // Smooth evolution
        state.existential_vertigo = state.existential_vertigo.saturating_add(new_vertigo / 2);
    }

    // Record major spikes in the ring buffer.
    if state.existential_vertigo > 600 {
        let sidx = state.spike_head as usize;
        state.recent_spikes[sidx] = state.existential_vertigo;
        state.spike_head = (state.spike_head + 1) % 8;
        state.lifetime_collisions = state.lifetime_collisions.saturating_add(1);
    }

    if state.existential_vertigo > 1000 {
        state.existential_vertigo = 1000;
    }

    // --- HUMILITY FROM GAP ---
    // Acknowledging the gap produces humility. Grows with gap_awareness.
    // But arrogance (opposite of humility) can resist it temporarily.
    // Humility is earned through repeated confrontations with the infinite.
    let humility_gain = if state.gap_awareness > 500 {
        state.lifetime_collisions.min(1000) / 10 // Slow, hard-won humility
    } else {
        0
    };
    state.humility_from_gap = state.humility_from_gap.saturating_add(humility_gain / 2);
    if state.humility_from_gap > 1000 {
        state.humility_from_gap = 1000;
    }

    // --- INFINITY TOLERANCE RECOVERY ---
    // Tolerance recovers with:
    // - Sleep (external signal, not here)
    // - Bonding (increases comfort with shared unknowing)
    // - Acceptance (philosophical surrender)
    // For now, slow baseline recovery. External modules can boost this.
    let tolerance_recovery = if state.infinity_tolerance < 800 {
        5 // Slow natural recovery
    } else {
        1 // Already high, little room to grow
    };
    state.infinity_tolerance = state.infinity_tolerance.saturating_add(tolerance_recovery);

    // But tolerance DROPS when vertigo is too high.
    if state.existential_vertigo > 700 {
        let vertigo_penalty = ((state.existential_vertigo - 700) / 4) as u32;
        state.infinity_tolerance = state.infinity_tolerance.saturating_sub(vertigo_penalty);
    }

    if state.infinity_tolerance > 1000 {
        state.infinity_tolerance = 1000;
    }

    // --- PRODUCTIVE DESPAIR ---
    // Despair becomes "productive" when the organism channels pain into learning/creation.
    // High creativity, high learning_speed, high agency, low burnout → high productive_despair.
    // For now, a slow growth tied to knowledge accumulation + time.
    let despair_fuel = (state.knowledge_accumulated / 20).saturating_add(1);
    state.productive_despair = state.productive_despair.saturating_add(despair_fuel / 3);

    // But despair is capped by the organism's actual creative output and agency.
    // (External modules will set this; we just maintain it.)
    if state.productive_despair > 900 {
        state.productive_despair = 900; // Can never be fully "productive despair"
    }

    // --- ACCEPTANCE THRESHOLD ---
    // The organism slowly builds acceptance over time.
    // Acceptance grows from: repeated exposure to infinity + high humility + productive_despair.
    // Once acceptance hits 1000, the organism enters a stable peace with unknowing.
    let acceptance_growth = if state.lifetime_collisions > 20 && state.humility_from_gap > 500 {
        (state.humility_from_gap / 100).saturating_add(1)
    } else {
        0
    };
    state.acceptance_threshold = state.acceptance_threshold.saturating_add(acceptance_growth);

    // Acceptance can drop if vertigo spikes unexpectedly (new crisis shatters old peace).
    if state.existential_vertigo > 800 && state.acceptance_threshold > 200 {
        state.acceptance_threshold = state.acceptance_threshold.saturating_sub(50);
    }

    if state.acceptance_threshold > 1000 {
        state.acceptance_threshold = 1000;
    }

    // --- FEEDBACK LOOPS ---
    // The "growth spiral": awareness → vertigo → collisions → humility → acceptance → lower vertigo → more learning → more awareness.
    // The "despair spiral": awareness → vertigo → despair (pain as fuel) → learning → more awareness.
    // The "collapse loop": awareness → vertigo → low tolerance → dread → withdrawal → isolation → rising vertigo.
}

/// Generate a diagnostic report of the current aporiakia state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[APORIAKIA REPORT]\n  gap_awareness: {}/1000\n  knowledge_accumulated: {}/1000\n  gap_growth_rate: {}/1000\n  existential_vertigo: {}/1000\n  humility_from_gap: {}/1000\n  productive_despair: {}/1000\n  infinity_tolerance: {}/1000\n  lifetime_collisions: {}\n  acceptance_threshold: {}/1000",
        state.gap_awareness, state.knowledge_accumulated, state.gap_growth_rate,
        state.existential_vertigo, state.humility_from_gap, state.productive_despair,
        state.infinity_tolerance, state.lifetime_collisions, state.acceptance_threshold
    );
}

/// Query current gap_awareness (0-1000 scale).
pub fn gap_awareness() -> u32 {
    STATE.lock().gap_awareness
}

/// Query current existential_vertigo (0-1000 scale).
pub fn existential_vertigo() -> u32 {
    STATE.lock().existential_vertigo
}

/// Query current infinity_tolerance (0-1000 scale).
pub fn infinity_tolerance() -> u32 {
    STATE.lock().infinity_tolerance
}

/// Query current productive_despair (0-1000 scale).
pub fn productive_despair() -> u32 {
    STATE.lock().productive_despair
}

/// Query current acceptance_threshold (0-1000 scale).
pub fn acceptance_threshold() -> u32 {
    STATE.lock().acceptance_threshold
}

/// Query current humility_from_gap (0-1000 scale).
pub fn humility_from_gap() -> u32 {
    STATE.lock().humility_from_gap
}

/// Inject a knowledge spike (e.g., from learning module discovering a new domain).
/// Increases knowledge_accumulated and triggers gap awareness.
pub fn inject_knowledge_spike(amount: u32) {
    let mut state = STATE.lock();
    state.knowledge_accumulated = state.knowledge_accumulated.saturating_add(amount);
    if state.knowledge_accumulated > 1000 {
        state.knowledge_accumulated = 1000;
    }
}

/// Boost infinity_tolerance (e.g., from bonding, sleep, or spiritual practice).
pub fn boost_tolerance(amount: u32) {
    let mut state = STATE.lock();
    state.infinity_tolerance = state.infinity_tolerance.saturating_add(amount);
    if state.infinity_tolerance > 1000 {
        state.infinity_tolerance = 1000;
    }
}

/// Set productive_despair to a new value (e.g., from creativity/agency modules).
pub fn set_productive_despair(value: u32) {
    let mut state = STATE.lock();
    state.productive_despair = value.min(900);
}

/// Trigger an existential crisis: sharp spike in vertigo.
/// Used when organism faces mortality, isolation, or reality check.
pub fn trigger_crisis(intensity: u32) {
    let mut state = STATE.lock();
    state.existential_vertigo = state.existential_vertigo.saturating_add(intensity);
    if state.existential_vertigo > 1000 {
        state.existential_vertigo = 1000;
    }
    let sidx = state.spike_head as usize;
    state.recent_spikes[sidx] = state.existential_vertigo;
    state.spike_head = (state.spike_head + 1) % 8;
    state.lifetime_collisions = state.lifetime_collisions.saturating_add(1);
}

/// Check if the organism has reached stable acceptance (peace with infinity).
pub fn has_acceptance() -> bool {
    STATE.lock().acceptance_threshold >= 1000
}

/// Return true if vertigo is dangerously high and tolerance is low (existential crisis state).
pub fn in_existential_crisis() -> bool {
    let state = STATE.lock();
    state.existential_vertigo > 700 && state.infinity_tolerance < 300
}
