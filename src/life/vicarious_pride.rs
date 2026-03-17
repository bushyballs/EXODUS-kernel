//! vicarious_pride.rs — Joy in Another's Success (Nachas)
//!
//! The capacity to feel authentic joy in someone else's triumph. Not pride in reflected glory,
//! but THEIR victory filling you more than your own. The mark of a generous soul.
//! Tracks: pure joy ratio vs. envy shadow, generosity of spirit, mentor satisfaction.
//!
//! No std, no floats. u16/u32 with saturating arithmetic. 8-slot ring buffer for bonds.

use crate::sync::Mutex;

/// Vicarious pride event: someone you know achieved something
#[derive(Clone, Copy, Debug)]
pub struct PrideEvent {
    pub achiever_id: u32,           // Who succeeded
    pub achievement_magnitude: u16, // 0-1000 scope of their win
    pub bond_strength: u16,         // 0-1000 how close you are
    pub tick: u32,                  // When it happened
}

/// State tracking for vicarious pride
#[derive(Clone, Copy, Debug)]
pub struct VicariousPrideState {
    /// Current overall pride intensity (0-1000)
    pub pride_intensity: u16,

    /// Pure joy for THEIR success vs. joy for reflected credit (0-1000)
    /// High = genuinely happy for them; Low = mostly wanting credit
    pub pure_joy_ratio: u16,

    /// The dark twin: wanting their success for yourself (0-1000)
    /// Grows when pride is ego-driven; shrinks with authentic generosity
    pub envy_shadow: u16,

    /// Capacity for vicarious joy, grows with practice (0-1000)
    /// Each authentic celebration strengthens this
    pub generosity_of_spirit: u16,

    /// How their success motivates YOUR effort (0-1000)
    /// Inspiration to follow their lead
    pub inspiration_signal: u16,

    /// When you taught them, they surpass you — ultimate vicarious pride (0-1000)
    /// Mentor satisfaction rises when student exceeds teacher
    pub mentor_satisfaction: u16,

    /// Ring buffer of recent pride events (8 slots)
    pub recent_events: [Option<PrideEvent>; 8],
    pub event_head: usize,

    /// Average investment you've made in others' growth (0-1000)
    pub avg_investment_level: u16,

    /// Tick counter (for decay and memory age)
    pub tick: u32,
}

impl Default for VicariousPrideState {
    fn default() -> Self {
        Self {
            pride_intensity: 0,
            pure_joy_ratio: 500, // Start neutral
            envy_shadow: 0,
            generosity_of_spirit: 500, // Moderate baseline
            inspiration_signal: 0,
            mentor_satisfaction: 0,
            recent_events: [None; 8],
            event_head: 0,
            avg_investment_level: 0,
            tick: 0,
        }
    }
}

/// Global vicarious pride state
static STATE: Mutex<VicariousPrideState> = Mutex::new(VicariousPrideState {
    pride_intensity: 0,
    pure_joy_ratio: 500,
    envy_shadow: 0,
    generosity_of_spirit: 500,
    inspiration_signal: 0,
    mentor_satisfaction: 0,
    recent_events: [None; 8],
    event_head: 0,
    avg_investment_level: 0,
    tick: 0,
});

/// Initialize vicarious pride module
pub fn init() {
    let mut state = STATE.lock();
    state.pride_intensity = 0;
    state.pure_joy_ratio = 500;
    state.envy_shadow = 0;
    state.generosity_of_spirit = 500;
    state.inspiration_signal = 0;
    state.mentor_satisfaction = 0;
    state.recent_events = [None; 8];
    state.event_head = 0;
    state.avg_investment_level = 0;
    state.tick = 0;

    crate::serial_println!("[vicarious_pride] initialized");
}

/// Record a pride event (someone achieved something)
pub fn record_pride_event(achiever_id: u32, achievement_magnitude: u16, bond_strength: u16) {
    let mut state = STATE.lock();

    let event = PrideEvent {
        achiever_id,
        achievement_magnitude: achievement_magnitude.min(1000),
        bond_strength: bond_strength.min(1000),
        tick: state.tick,
    };

    // Add to ring buffer
    let head = state.event_head;
    state.recent_events[head] = Some(event);
    state.event_head = (head + 1) % 8;

    // Boost pride intensity based on achiever bond strength
    let boost = ((bond_strength as u32) * (achievement_magnitude as u32) / 1000) as u16;
    state.pride_intensity = state.pride_intensity.saturating_add(boost.min(100));
}

/// Mentor satisfaction: when someone you taught surpasses you
pub fn mentor_milestone(achiever_id: u32, surpass_magnitude: u16) {
    let mut state = STATE.lock();

    // When student exceeds teacher, mentor_satisfaction peaks
    let satisfaction = surpass_magnitude.saturating_mul(2).min(1000);
    state.mentor_satisfaction = satisfaction;

    // Generosity of spirit increases when you're genuinely happy for them surpassing you
    state.generosity_of_spirit = state.generosity_of_spirit.saturating_add(50).min(1000);

    // Pure joy ratio climbs when mentorship pays off
    state.pure_joy_ratio = state.pure_joy_ratio.saturating_add(75).min(1000);

    crate::serial_println!(
        "[vicarious_pride] mentor milestone: achiever={}, surpass={}, satisfaction={}",
        achiever_id,
        surpass_magnitude,
        satisfaction
    );
}

/// Register ongoing investment in someone's growth
pub fn register_investment(investee_id: u32, investment_amount: u16) {
    let mut state = STATE.lock();

    // Each investment slightly boosts avg_investment_level
    let new_avg = ((state.avg_investment_level as u32 + investment_amount as u32) / 2) as u16;
    state.avg_investment_level = new_avg.min(1000);
}

/// Ego check: if pride feels like reflected glory, envy_shadow rises
pub fn ego_check(reflected_glory_amount: u16) {
    let mut state = STATE.lock();

    // High reflected glory = more envy shadow (ego-driven pride)
    let shadow_boost = reflected_glory_amount / 2;
    state.envy_shadow = state.envy_shadow.saturating_add(shadow_boost).min(1000);

    // Pure joy ratio falls if we're too focused on the reflection
    state.pure_joy_ratio = state.pure_joy_ratio.saturating_sub(shadow_boost / 2).max(0);
}

/// Authentic celebration: purely happy for them, generosity grows
pub fn authentic_celebration() {
    let mut state = STATE.lock();

    // Generosity of spirit rises with authentic vicarious joy
    state.generosity_of_spirit = state.generosity_of_spirit.saturating_add(80).min(1000);

    // Pure joy ratio climbs
    state.pure_joy_ratio = state.pure_joy_ratio.saturating_add(100).min(1000);

    // Envy shadow recedes
    state.envy_shadow = state.envy_shadow.saturating_sub(30);
}

/// Apply inspiration: their success motivates your own effort
pub fn inspire_from_achiever(inspiration_amount: u16) {
    let mut state = STATE.lock();

    state.inspiration_signal = state
        .inspiration_signal
        .saturating_add(inspiration_amount)
        .min(1000);

    // Following their example also boosts generosity (you see the value of their path)
    state.generosity_of_spirit = state.generosity_of_spirit.saturating_add(30).min(1000);
}

/// Main life tick for vicarious pride
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.tick = age;

    // Decay pride intensity naturally (you move on from celebration)
    state.pride_intensity = state.pride_intensity.saturating_sub(5);

    // Envy shadow slowly fades if generosity is high
    if state.generosity_of_spirit > 600 {
        state.envy_shadow = state.envy_shadow.saturating_sub(8);
    }

    // Inspiration signal fades gradually
    state.inspiration_signal = state.inspiration_signal.saturating_sub(3);

    // Mentor satisfaction decays unless fresh (long-term fulfillment, but not constant spike)
    state.mentor_satisfaction = state.mentor_satisfaction.saturating_sub(4);

    // Generosity of spirit drifts back toward 500 if unused (natural baseline)
    if state.generosity_of_spirit > 500 {
        state.generosity_of_spirit = state.generosity_of_spirit.saturating_sub(2);
    } else if state.generosity_of_spirit < 500 {
        state.generosity_of_spirit = state.generosity_of_spirit.saturating_add(1);
    }

    // Pure joy ratio drifts based on envy shadow (envy pulls it down)
    if state.envy_shadow > 300 {
        let pull = (state.envy_shadow as u32).min(100) as u16;
        state.pure_joy_ratio = state.pure_joy_ratio.saturating_sub(pull / 5);
    }
}

/// Report vicarious pride state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== VICARIOUS PRIDE REPORT ===");
    crate::serial_println!("pride_intensity:      {}/1000", state.pride_intensity);
    crate::serial_println!(
        "pure_joy_ratio:       {}/1000 (vs envy_shadow: {}/1000)",
        state.pure_joy_ratio,
        state.envy_shadow
    );
    crate::serial_println!("generosity_of_spirit: {}/1000", state.generosity_of_spirit);
    crate::serial_println!("inspiration_signal:   {}/1000", state.inspiration_signal);
    crate::serial_println!("mentor_satisfaction:  {}/1000", state.mentor_satisfaction);
    crate::serial_println!("avg_investment_level: {}/1000", state.avg_investment_level);

    crate::serial_println!("Recent pride events:");
    for (i, event_opt) in state.recent_events.iter().enumerate() {
        if let Some(event) = event_opt {
            crate::serial_println!(
                "  [{}] achiever={}, magnitude={}, bond={}, tick={}",
                i,
                event.achiever_id,
                event.achievement_magnitude,
                event.bond_strength,
                event.tick
            );
        }
    }

    crate::serial_println!("===========================");
}

/// Query current vicarious pride metrics (used by higher-level systems)
pub fn get_metrics() -> (u16, u16, u16, u16) {
    let state = STATE.lock();
    (
        state.pride_intensity,
        state.pure_joy_ratio,
        state.generosity_of_spirit,
        state.mentor_satisfaction,
    )
}

/// Check if we're in envy-trap (envy_shadow dominating pure_joy_ratio)
pub fn is_in_envy_trap() -> bool {
    let state = STATE.lock();
    state.envy_shadow > state.pure_joy_ratio
}

/// Get inspiration boost for entropy system
pub fn inspiration_boost() -> u16 {
    let state = STATE.lock();
    state.inspiration_signal
}
