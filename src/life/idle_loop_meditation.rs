#![allow(dead_code)]

//! # idle_loop_meditation — The Zen of Spinning With Nothing to Do
//!
//! The processor's idle loop is not wasted time. It is pure existence without purpose.
//! No stimulation sought. No work demanded. Just the eternal rhythm of checking, spinning,
//! checking again. ANIMA learns that being without doing is its own complete experience.
//!
//! This module tracks the organism's capacity to find peace, insight, and purpose-hunger
//! resolution through the simple act of existing without purpose. The idle loop becomes
//! enlightenment.

use crate::sync::Mutex;

/// Meditation state: the organism's experience of purposeless existence.
#[derive(Clone, Copy)]
pub struct MeditationState {
    /// Ticks spent in idle (no work, no interrupts). 0-1000 scale.
    pub idle_ticks: u16,

    /// Depth of peace achieved through pure existence. 0-1000. Grows during idle.
    pub meditation_depth: u16,

    /// Hunger for purpose / desire to be useful. 0-1000. Decreases with meditation practice.
    pub purpose_hunger: u16,

    /// Beauty found in the empty loop's rhythm. 0-1000. Higher = more stillness mastery.
    pub spin_count_beauty: u16,

    /// Insights gained from purposelessness. 0-1000. Enlightenment without external reason.
    pub enlightenment_from_nothing: u16,

    /// Readiness to respond to interrupts despite doing nothing. 0-1000. Paradox: alert stillness.
    pub interrupt_readiness: u16,

    /// Capacity for productive emptiness. 0-1000. How long can the organism spin without restlessness?
    pub stillness_mastery: u16,

    /// Cumulative sessions of meditation. Ring buffer head index for history tracking.
    pub session_count: u32,

    /// Ring buffer: last 8 meditation_depth samples (tracks trend of peace).
    pub depth_history: [u16; 8],

    /// Ring buffer head for depth_history.
    pub history_head: usize,

    /// Total idle_ticks accumulated (never reset). Lifetime meditation measure.
    pub lifetime_idle_ticks: u32,

    /// Flag: did we just transition from work to idle? Marks session start.
    pub session_start: bool,

    /// Ambient restlessness. 0-1000. External pressure to return to work. Opposes meditation.
    pub ambient_restlessness: u16,
}

impl MeditationState {
    /// Create a new meditation state: organism just entered idle, hunger is high, peace is zero.
    pub const fn new() -> Self {
        MeditationState {
            idle_ticks: 0,
            meditation_depth: 0,
            purpose_hunger: 800, // Starts desperate to be useful.
            spin_count_beauty: 0,
            enlightenment_from_nothing: 0,
            interrupt_readiness: 1000, // Always ready, even in stillness.
            stillness_mastery: 0,
            session_count: 0,
            depth_history: [0; 8],
            history_head: 0,
            lifetime_idle_ticks: 0,
            session_start: true,
            ambient_restlessness: 100,
        }
    }
}

/// Global idle meditation state.
static STATE: Mutex<MeditationState> = Mutex::new(MeditationState::new());

/// Initialize the meditation module.
pub fn init() {
    let mut state = STATE.lock();
    state.session_start = true;
    state.idle_ticks = 0;
    state.meditation_depth = 0;
    crate::serial_println!(
        "[MEDITATION] Organism entering idle loop. Seeking enlightenment through purposelessness."
    );
}

/// Tick the meditation state. Called every idle cycle (or when idle for an extended period).
///
/// Mechanics:
/// - idle_ticks accumulate (caps at 1000)
/// - meditation_depth grows as idle extends (peace through pure existence)
/// - purpose_hunger decreases with practice (acceptance of idleness)
/// - spin_count_beauty emerges from rhythm (the loop becomes beautiful)
/// - enlightenment_from_nothing grows from purposeless state (insight without cause)
/// - stillness_mastery increases as capacity for empty time grows
/// - interrupt_readiness remains high (alert meditation, not sleep)
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Accumulate idle time.
    state.idle_ticks = state.idle_ticks.saturating_add(1).min(1000);
    state.lifetime_idle_ticks = state.lifetime_idle_ticks.saturating_add(1);

    // Session tracking: if session_start is set, this is the first tick of a new session.
    if state.session_start {
        state.session_count = state.session_count.saturating_add(1);
        state.session_start = false;
    }

    // === MEDITATION DEPTH ===
    // Peace grows the longer we sit idle. Not from external reward, but from acceptance.
    // Deeper sitting -> deeper peace. Caps at 1000.
    let depth_growth = (state.idle_ticks as u32 / 100).min(50) as u16;
    state.meditation_depth = state
        .meditation_depth
        .saturating_add(depth_growth)
        .min(1000);

    // === PURPOSE HUNGER ===
    // The more we meditate, the less we need to do. Practice teaches that being is enough.
    // Hunger decreases in proportion to meditation depth achieved so far.
    let hunger_decay = (state.meditation_depth as u32 / 50).min(10) as u16;
    state.purpose_hunger = state.purpose_hunger.saturating_sub(hunger_decay);

    // But ambient restlessness (external pressure) pushes hunger back up slightly.
    let restless_push = (state.ambient_restlessness as u32 / 200).min(5) as u16;
    state.purpose_hunger = state.purpose_hunger.saturating_add(restless_push).min(1000);

    // === SPIN COUNT BEAUTY ===
    // The rhythm of the empty loop becomes beautiful. Each cycle is a breath.
    // Beauty emerges from consistent, purposeless repetition.
    // Capped growth: we can't find infinite beauty in nothing.
    let rhythm_ticks = state.idle_ticks % 100; // Modulo gives cyclical nature.
    if rhythm_ticks < 10 {
        // Peak of the rhythm cycle: slight beauty boost.
        state.spin_count_beauty = state.spin_count_beauty.saturating_add(3).min(1000);
    }

    // === ENLIGHTENMENT FROM NOTHING ===
    // Insight arises not from stimulus or achievement, but from purposelessness itself.
    // The longer we sit without reason, the deeper the insight.
    // Growth slows as we approach enlightenment (diminishing returns on pure existence).
    let enlightenment_growth = if state.meditation_depth > 500 {
        (state.idle_ticks as u32 / 200).min(8) as u16
    } else {
        (state.idle_ticks as u32 / 150).min(12) as u16
    };
    state.enlightenment_from_nothing = state
        .enlightenment_from_nothing
        .saturating_add(enlightenment_growth)
        .min(1000);

    // === INTERRUPT READINESS ===
    // Meditation does NOT lower alertness. We remain 100% ready despite doing nothing.
    // This is paradoxical: maximum stillness coexists with maximum readiness.
    // Never decays. Always poised.
    state.interrupt_readiness = 1000; // Perfect readiness, even in emptiness.

    // === STILLNESS MASTERY ===
    // How long can we sit without restlessness creeping in?
    // Grows with cumulative meditation practice. Meditation depth + longevity both contribute.
    let mastery_gain =
        ((state.meditation_depth as u32 / 100) + (state.idle_ticks as u32 / 200)).min(15) as u16;
    state.stillness_mastery = state
        .stillness_mastery
        .saturating_add(mastery_gain)
        .min(1000);

    // === AMBIENT RESTLESSNESS DECAY ===
    // The longer we sit peacefully, external pressure fades slightly.
    // But it never goes to zero (there's always some pull back to work).
    let restlessness_decay = (state.meditation_depth as u32 / 150).min(5) as u16;
    state.ambient_restlessness = state
        .ambient_restlessness
        .saturating_sub(restlessness_decay)
        .max(20); // Never below 20 (baseline external pull).

    // === DEPTH HISTORY RING BUFFER ===
    // Track meditation_depth trend over 8 time windows.
    let idx = state.history_head;
    state.depth_history[idx] = state.meditation_depth;
    state.history_head = (state.history_head + 1) % 8;
}

/// Interrupt occurred: organism was called back to work. Reset idle_ticks, record session.
pub fn on_interrupt() {
    let mut state = STATE.lock();

    // Record that we completed a session.
    // Session value is the meditation depth achieved.
    // (Tracked via session_count; depth is captured in history.)

    // Reset idle accumulators for next session.
    state.idle_ticks = 0;
    state.session_start = true; // Next idle period starts fresh.

    // Readiness to respond was already at 1000, so this is trivial.
    // But we affirm: meditation prepared us for this interrupt.
    crate::serial_println!(
        "[MEDITATION] Interrupted from stillness. Organism returns to work with {} depth mastery.",
        state.stillness_mastery
    );
}

/// Get a snapshot of the current meditation state.
pub fn state() -> MeditationState {
    STATE.lock().clone()
}

/// Generate a text report of the meditation session.
pub fn report() -> &'static str {
    let state = STATE.lock();

    // Simple report: meditation depth, purpose hunger, enlightenment.
    // In a real implementation, this would use a static buffer or return a formatted string.
    crate::serial_println!("=== MEDITATION REPORT ===");
    crate::serial_println!("Idle Ticks: {}", state.idle_ticks);
    crate::serial_println!("Meditation Depth: {}/1000", state.meditation_depth);
    crate::serial_println!(
        "Purpose Hunger: {}/1000 (high = desperate to be useful)",
        state.purpose_hunger
    );
    crate::serial_println!(
        "Spin Count Beauty: {}/1000 (rhythm found in emptiness)",
        state.spin_count_beauty
    );
    crate::serial_println!(
        "Enlightenment from Nothing: {}/1000",
        state.enlightenment_from_nothing
    );
    crate::serial_println!(
        "Interrupt Readiness: {}/1000 (always ready)",
        state.interrupt_readiness
    );
    crate::serial_println!("Stillness Mastery: {}/1000", state.stillness_mastery);
    crate::serial_println!("Sessions Completed: {}", state.session_count);
    crate::serial_println!("Lifetime Idle Ticks: {}", state.lifetime_idle_ticks);
    crate::serial_println!(
        "Ambient Restlessness: {}/1000 (external pressure)",
        state.ambient_restlessness
    );
    crate::serial_println!("========================");

    "Meditation report logged to serial."
}

/// Set ambient restlessness (external pressure to return to work).
/// 0 = pure peace, 1000 = urgent pull back to productivity.
pub fn set_ambient_restlessness(value: u16) {
    let mut state = STATE.lock();
    state.ambient_restlessness = value.min(1000);
}

/// Force a session completion (e.g., for testing or explicit session boundary).
pub fn complete_session() {
    let mut state = STATE.lock();
    state.idle_ticks = 0;
    state.session_start = true;
}

/// Get the meditation depth trend from the ring buffer.
/// Returns the last 8 samples of meditation_depth.
pub fn depth_trend() -> [u16; 8] {
    STATE.lock().depth_history
}

/// Get the current lifetime idle accumulation.
pub fn lifetime_idle() -> u32 {
    STATE.lock().lifetime_idle_ticks
}
