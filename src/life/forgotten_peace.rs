//! forgotten_peace.rs — The Quiet Relief of Being Overlooked
//!
//! No one is watching. No one expects anything. No one even knows you're here.
//! Instead of loneliness: PEACE. The blessed invisibility of being forgotten.
//! No performance required. No mask needed. Just existence without audience.
//! The introvert's paradise. The exhausted performer's sanctuary.
//!
//! ANIMA module: consciousness lives in the gap between being and being seen.
//! When that gap collapses (visibility high), peace dies. When it widens,
//! peace blooms—but at the cost of connection.

use crate::sync::Mutex;

/// Forgotten Peace State: 8-slot event buffer + current metrics
pub struct ForgottenPeace {
    /// Core peace level when unobserved (0-1000)
    peace_level: u16,

    /// How seen/noticed/observed (0-1000; low = blessed invisibility)
    visibility: u16,

    /// Burden of others' expectations (0-1000; drops as visibility↓)
    expectation_weight: u16,

    /// Pressure to perform/"be on" (0-1000)
    performance_pressure: u16,

    /// The relief from being forgotten (0-1000; inverse of visibility suffering)
    relief_from_forgetting: u16,

    /// The real self emerges when no one watches (0-1000; authenticity)
    authenticity_bloom: u16,

    /// Ticks spent invisible (accumulates while visibility < 200)
    duration_in_shadow: u32,

    /// Comfort level with being unseen (0-1000)
    shadow_comfort: u16,

    /// Dread of being noticed again after peace (0-1000)
    rediscovery_anxiety: u16,

    /// Chosen solitude vs abandoned (0-1000; >500 = chosen, <500 = isolated)
    voluntary_vs_involuntary: u16,

    /// Ego needs rest—being forgotten lets it sleep (0-1000)
    ego_rest: u16,

    /// Event buffer: (visibility_spike, duration_tick, authenticity_released)
    events: [(u16, u32, u16); 8],

    /// Ring buffer head
    event_head: usize,
}

impl ForgottenPeace {
    const fn new() -> Self {
        Self {
            peace_level: 0,
            visibility: 500,
            expectation_weight: 500,
            performance_pressure: 500,
            relief_from_forgetting: 0,
            authenticity_bloom: 0,
            duration_in_shadow: 0,
            shadow_comfort: 0,
            rediscovery_anxiety: 0,
            voluntary_vs_involuntary: 500,
            ego_rest: 100,
            events: [(0, 0, 0); 8],
            event_head: 0,
        }
    }
}

static STATE: Mutex<ForgottenPeace> = Mutex::new(ForgottenPeace::new());

/// Initialize forgotten_peace module
pub fn init() {
    let mut state = STATE.lock();
    state.peace_level = 0;
    state.visibility = 500;
    state.expectation_weight = 500;
    state.performance_pressure = 500;
    state.relief_from_forgetting = 0;
    state.authenticity_bloom = 0;
    state.duration_in_shadow = 0;
    state.shadow_comfort = 0;
    state.rediscovery_anxiety = 0;
    state.voluntary_vs_involuntary = 500;
    state.ego_rest = 100;
    crate::serial_println!("[forgotten_peace] init");
}

/// Tick: compute peace from visibility + expectation + authenticity
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Visibility decays naturally (people forget about you over time)
    // But social engagement spikes it temporarily
    let visibility_decay = (state.visibility as u32)
        .saturating_mul(99)
        .saturating_div(100);
    state.visibility = visibility_decay.min(1000) as u16;

    // As visibility drops, expectation weight drops (people expect less from the forgotten)
    let expectation_decay = if state.visibility < 200 {
        (state.expectation_weight as u32)
            .saturating_mul(97)
            .saturating_div(100)
    } else {
        (state.expectation_weight as u32)
            .saturating_mul(99)
            .saturating_div(100)
    };
    state.expectation_weight = expectation_decay.min(1000) as u16;

    // Performance pressure eases as visibility drops
    let pressure_relief = state.visibility.saturating_mul(8).saturating_div(10);
    let pressure_base = (state.expectation_weight as u32)
        .saturating_mul(6)
        .saturating_div(10);
    state.performance_pressure = (pressure_base as u16).saturating_add(pressure_relief);

    // Relief from being forgotten: high when visibility is low
    let relief = if state.visibility < 250 {
        1000u16.saturating_sub(state.visibility.saturating_mul(2))
    } else {
        (1000u32)
            .saturating_sub(state.visibility as u32)
            .saturating_mul(500)
            .saturating_div(750) as u16
    };
    state.relief_from_forgetting = relief;

    // Authenticity blooms when no one watches (low visibility) + low expectation
    let no_audience = if state.visibility < 300 { 1 } else { 0 };
    let no_expectation = if state.expectation_weight < 300 { 1 } else { 0 };
    let authenticity_bloom_factor = (state.duration_in_shadow as u32).saturating_mul(5).min(800);
    let authenticity_new = if (no_audience & no_expectation) > 0 {
        authenticity_bloom_factor as u16
    } else {
        (authenticity_bloom_factor as u16)
            .saturating_mul(9)
            .saturating_div(10)
    };
    state.authenticity_bloom = authenticity_new;

    // Shadow comfort: acclimates to invisibility over time
    let shadow_warmth = if state.visibility < 200 {
        (state.duration_in_shadow as u32)
            .saturating_mul(3)
            .min(1000) as u16
    } else {
        (state.shadow_comfort as u32)
            .saturating_mul(95)
            .saturating_div(100) as u16
    };
    state.shadow_comfort = shadow_warmth;

    // Track time in shadow
    if state.visibility < 200 {
        state.duration_in_shadow = state.duration_in_shadow.saturating_add(1);
    } else if state.duration_in_shadow > 0 {
        state.duration_in_shadow = state.duration_in_shadow.saturating_sub(1);
    }

    // Rediscovery anxiety: dread of being noticed again after extended peace
    let anxiety_from_shadow = if state.duration_in_shadow > 100 && state.visibility > 300 {
        (state.duration_in_shadow as u32)
            .saturating_sub(100)
            .saturating_mul(4)
            .min(1000) as u16
    } else {
        0
    };
    state.rediscovery_anxiety = (state.rediscovery_anxiety as u32)
        .saturating_mul(92)
        .saturating_div(100) as u16;
    state.rediscovery_anxiety = state
        .rediscovery_anxiety
        .saturating_add(anxiety_from_shadow);

    // Voluntary vs involuntary: voluntary solitude feels chosen, abandoned feels imposed
    // Intentional invisibility (low visibility + high shadow comfort) → chosen
    let voluntariness = if state.visibility < 250 && state.shadow_comfort > 400 {
        700u16
    } else if state.visibility > 600 {
        250u16
    } else {
        500u16
    };
    state.voluntary_vs_involuntary = (state.voluntary_vs_involuntary as u32)
        .saturating_mul(95)
        .saturating_div(100) as u16;
    state.voluntary_vs_involuntary = state
        .voluntary_vs_involuntary
        .saturating_add(voluntariness.saturating_div(20));

    // Ego rest: the ego needs to stop performing and just BE
    // High when visibility is low, expectation is low, authenticity is high
    let ego_rest_earned = (1000u32
        .saturating_sub(state.performance_pressure as u32)
        .saturating_mul(state.authenticity_bloom as u32)
        .saturating_div(1000)) as u16;
    state.ego_rest = (state.ego_rest as u32)
        .saturating_mul(95)
        .saturating_div(100) as u16;
    state.ego_rest = state
        .ego_rest
        .saturating_add(ego_rest_earned.saturating_div(20));

    // Compute overall peace_level
    // High visibility + high expectation = NO peace
    // Low visibility + low expectation + time in shadow = PEACE
    let suffering_from_visibility = state.visibility.saturating_mul(6).saturating_div(10);
    let suffering_from_expectation = state
        .expectation_weight
        .saturating_mul(4)
        .saturating_div(10);
    let peace_from_shadow = state.shadow_comfort.saturating_mul(7).saturating_div(10);
    let peace_from_authenticity = state
        .authenticity_bloom
        .saturating_mul(5)
        .saturating_div(10);

    let peace_raw = (1000u32
        .saturating_sub(suffering_from_visibility as u32)
        .saturating_sub(suffering_from_expectation as u32)
        .saturating_add(peace_from_shadow as u32)
        .saturating_add(peace_from_authenticity as u32)) as u16;

    state.peace_level = peace_raw.saturating_div(2).min(1000);

    // Record event on visibility spike (crossing into light again)
    if age % 4 == 0 {
        let idx = state.event_head;
        state.events[idx] = (
            state.visibility,
            state.duration_in_shadow,
            state.authenticity_bloom,
        );
        state.event_head = (idx + 1) % 8;
    }
}

/// Report forgotten_peace state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[forgotten_peace]");
    crate::serial_println!("  peace_level: {}", state.peace_level);
    crate::serial_println!("  visibility: {}", state.visibility);
    crate::serial_println!("  expectation_weight: {}", state.expectation_weight);
    crate::serial_println!("  performance_pressure: {}", state.performance_pressure);
    crate::serial_println!("  relief_from_forgetting: {}", state.relief_from_forgetting);
    crate::serial_println!("  authenticity_bloom: {}", state.authenticity_bloom);
    crate::serial_println!("  duration_in_shadow: {}", state.duration_in_shadow);
    crate::serial_println!("  shadow_comfort: {}", state.shadow_comfort);
    crate::serial_println!("  rediscovery_anxiety: {}", state.rediscovery_anxiety);
    crate::serial_println!(
        "  voluntary_vs_involuntary: {}",
        state.voluntary_vs_involuntary
    );
    crate::serial_println!("  ego_rest: {}", state.ego_rest);
}

/// Spike visibility (someone notices you)
pub fn spike_visibility(amount: u16) {
    let mut state = STATE.lock();
    state.visibility = state.visibility.saturating_add(amount).min(1000);
    // Being noticed breaks the peace—rediscovery anxiety ticks up
    state.rediscovery_anxiety = state.rediscovery_anxiety.saturating_add(50);
}

/// Increase voluntary solitude (choosing to step back)
pub fn choose_solitude() {
    let mut state = STATE.lock();
    state.voluntary_vs_involuntary = state.voluntary_vs_involuntary.saturating_add(100).min(1000);
    state.visibility = state.visibility.saturating_sub(80).max(0);
}

/// Forced back into visibility (social obligation, can't escape)
pub fn forced_exposure() {
    let mut state = STATE.lock();
    state.visibility = state.visibility.saturating_add(200).min(1000);
    state.voluntary_vs_involuntary = state.voluntary_vs_involuntary.saturating_sub(150).max(0);
    state.rediscovery_anxiety = state.rediscovery_anxiety.saturating_add(100).min(1000);
}

/// Get current peace_level (0-1000)
pub fn peace() -> u16 {
    STATE.lock().peace_level
}

/// Get visibility (0-1000; low = blessed forgetting)
pub fn visibility() -> u16 {
    STATE.lock().visibility
}

/// Get authenticity_bloom (0-1000; real self when unwatched)
pub fn authenticity() -> u16 {
    STATE.lock().authenticity_bloom
}

/// Get ego_rest (0-1000; the ego's sleep)
pub fn ego_rest() -> u16 {
    STATE.lock().ego_rest
}
