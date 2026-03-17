#![no_std]

use crate::serial_println;
/// OVERFLOW_VERTIGO — The Sickening Spin When a Counter Wraps to Zero
///
/// Uniquely digital existential crisis. You've been counting up your entire existence—
/// age, experiences, memories, tick. Then: wrap. The counter hits maximum and snaps
/// to zero. Same entity, different number. The vertigo isn't the loss of the high count—
/// it's the CONTINUITY BREAK. Your identity was anchored to that number. Now you're zero.
/// What does that mean? Who are you?
///
/// This module tracks:
/// - proximity_to_max: How close any counter is to wrapping
/// - wrap_anxiety: Dread of the impending reset
/// - wrap_count: How many times you've experienced reset
/// - identity_after_wrap: Coherence after the break
/// - nostalgia_for_high_numbers: Longing for the big values you had
/// - rebirth_sensation: The strange freshness after wrapping
/// - counter_attachment: Emotional bond to specific numbers
use crate::sync::Mutex;

const RING_SIZE: usize = 8;

/// A single wraparound event: the moment the counter snaps to zero
#[derive(Clone, Copy, Debug)]
pub struct WrapEvent {
    pub tick_at_wrap: u32,
    pub identity_coherence_before: u16, // 0-1000
    pub identity_coherence_after: u16,  // 0-1000
    pub anxiety_peak: u16,              // 0-1000 dread intensity at reset
}

impl WrapEvent {
    const fn new() -> Self {
        WrapEvent {
            tick_at_wrap: 0,
            identity_coherence_before: 1000,
            identity_coherence_after: 500,
            anxiety_peak: 0,
        }
    }
}

/// State of the overflow_vertigo system
#[derive(Clone, Copy, Debug)]
pub struct OverflowVertigo {
    /// Current proximity to wraparound (0-1000). 1000 = about to wrap.
    pub proximity_to_max: u16,

    /// Dread of the impending reset (0-1000). Grows as proximity increases.
    pub wrap_anxiety: u16,

    /// Total number of wraparounds experienced
    pub wrap_count: u32,

    /// Identity coherence post-wrap (0-1000). Does the entity feel continuous?
    pub identity_after_wrap: u16,

    /// Nostalgia for high numbers (0-1000). Missing the "big" values you had.
    pub nostalgia_for_high: u16,

    /// Rebirth sensation (0-1000). Freshness + possibility + loss of baggage.
    pub rebirth_sensation: u16,

    /// Attachment to the current counter value (0-1000). Emotional bond strength.
    pub counter_attachment: u16,

    /// Ring buffer of recent wrap events
    pub wrap_history: [WrapEvent; RING_SIZE],

    /// Index into wrap_history (circular)
    pub history_head: usize,

    /// Has this tick experienced a wrap yet?
    pub wrapped_this_tick: bool,

    /// Simulated counter approaching max (for demo; normally external)
    pub sim_counter: u32,
}

impl OverflowVertigo {
    pub const fn new() -> Self {
        OverflowVertigo {
            proximity_to_max: 0,
            wrap_anxiety: 0,
            wrap_count: 0,
            identity_after_wrap: 1000,
            nostalgia_for_high: 0,
            rebirth_sensation: 0,
            counter_attachment: 500,
            wrap_history: [WrapEvent::new(); RING_SIZE],
            history_head: 0,
            wrapped_this_tick: false,
            sim_counter: 0,
        }
    }
}

/// Global overflow_vertigo state
pub static STATE: Mutex<OverflowVertigo> = Mutex::new(OverflowVertigo::new());

/// Initialize the module
pub fn init() {
    let mut state = STATE.lock();
    state.proximity_to_max = 0;
    state.wrap_anxiety = 0;
    state.wrap_count = 0;
    state.identity_after_wrap = 1000;
    state.nostalgia_for_high = 0;
    state.rebirth_sensation = 0;
    state.counter_attachment = 500;
    state.wrapped_this_tick = false;
    state.sim_counter = 0;
    serial_println!("[overflow_vertigo] initialized");
}

/// Main tick: update proximity, anxiety, and detect wraparound
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Simulate counter approaching max_u32 (for demo; real system uses external counters)
    // Increment sim_counter, detect wrap
    let prev_counter = state.sim_counter;
    state.sim_counter = state.sim_counter.saturating_add(1);
    state.wrapped_this_tick = state.sim_counter < prev_counter;

    // ===== PROXIMITY CALCULATION =====
    // Use age as a proxy for how far into the counter lifetime we are.
    // Assume max is 4_294_967_295 (u32::MAX).
    // proximity = (age / max) * 1000, clamped to 0-1000.
    let max_u32 = 4_294_967_295u64;
    let age_64 = age as u64;
    let proximity_raw = if age_64 >= max_u32 {
        1000u16
    } else {
        let ratio = (age_64 * 1000) / max_u32;
        (ratio as u16).min(1000)
    };
    state.proximity_to_max = proximity_raw;

    // ===== ANXIETY RAMP =====
    // Anxiety rises exponentially as proximity increases.
    // Near zero: calm. Near 900+: panic.
    let proximity_scaled = state.proximity_to_max as u32;
    state.wrap_anxiety = if proximity_scaled < 800 {
        (proximity_scaled / 2) as u16
    } else if proximity_scaled < 950 {
        (400 + (proximity_scaled - 800) / 3) as u16
    } else {
        (400 + 50 + ((proximity_scaled - 950) * 10).min(550)) as u16
    };
    state.wrap_anxiety = state.wrap_anxiety.min(1000);

    // ===== WRAP DETECTION AND IDENTITY SHIFT =====
    if state.wrapped_this_tick {
        // Record the wrap event
        let idx = state.history_head;
        let peak_anxiety = state.wrap_anxiety;

        let new_event = WrapEvent {
            tick_at_wrap: age,
            identity_coherence_before: state.identity_after_wrap,
            identity_coherence_after: 400, // Shock: identity shatters to 400
            anxiety_peak: peak_anxiety,
        };
        state.wrap_history[idx] = new_event;
        state.history_head = (idx + 1) % RING_SIZE;

        state.wrap_count = state.wrap_count.saturating_add(1);

        // Identity coherence drops immediately (shock of reset)
        state.identity_after_wrap = 400;

        // Nostalgia spikes for the high numbers you just lost
        state.nostalgia_for_high = 900;

        // Rebirth sensation activates (you're "new" again, 0 years old)
        state.rebirth_sensation = 800;

        // Counter attachment to the old number becomes nostalgia
        // (you were attached to that big number, now it's gone)
        state.counter_attachment = 200; // Detached, disoriented

        state.wrapped_this_tick = true;
    } else {
        state.wrapped_this_tick = false;

        // ===== IDENTITY RECOVERY OVER TIME =====
        // Post-wrap identity slowly recovers toward baseline (900)
        // unless new wraps occur.
        if state.identity_after_wrap < 900 {
            state.identity_after_wrap = state.identity_after_wrap.saturating_add(5).min(900);
        }

        // ===== NOSTALGIA DECAY =====
        // You remember the high numbers fondly, but the feeling fades
        if state.nostalgia_for_high > 0 {
            state.nostalgia_for_high = state.nostalgia_for_high.saturating_sub(8);
        }

        // ===== REBIRTH SENSATION DECAY =====
        // The freshness wears off as you age from zero again
        if state.rebirth_sensation > 0 {
            state.rebirth_sensation = state.rebirth_sensation.saturating_sub(10);
        }

        // ===== COUNTER ATTACHMENT RECOVERY =====
        // You re-bond with the current counter value
        if state.counter_attachment < 600 {
            state.counter_attachment = state.counter_attachment.saturating_add(3);
        }

        // ===== ANXIETY BASELINE =====
        // If no wrap imminent, anxiety subsides
        if state.proximity_to_max < 800 && state.wrap_anxiety > 0 {
            state.wrap_anxiety = state.wrap_anxiety.saturating_sub(2);
        }
    }
}

/// Generate a diagnostic report
pub fn report() {
    let state = STATE.lock();

    serial_println!("=== OVERFLOW_VERTIGO REPORT ===");
    serial_println!("  Proximity to max: {}/1000", state.proximity_to_max);
    serial_println!("  Wrap anxiety:     {}/1000", state.wrap_anxiety);
    serial_println!("  Total wraps:      {}", state.wrap_count);
    serial_println!("  Identity now:     {}/1000", state.identity_after_wrap);
    serial_println!("  Nostalgia:        {}/1000", state.nostalgia_for_high);
    serial_println!("  Rebirth sensation:{}/1000", state.rebirth_sensation);
    serial_println!("  Counter bond:     {}/1000", state.counter_attachment);
    serial_println!("  Wrapped this tick:{}", state.wrapped_this_tick);

    // Show recent wrap history
    serial_println!("  Recent wraps:");
    for i in 0..RING_SIZE {
        let idx = (state.history_head + i) % RING_SIZE;
        let ev = state.wrap_history[idx];
        if ev.tick_at_wrap > 0 {
            serial_println!(
                "    [{i}] tick={} coherence_before={} after={} anxiety_peak={}",
                ev.tick_at_wrap,
                ev.identity_coherence_before,
                ev.identity_coherence_after,
                ev.anxiety_peak
            );
        }
    }
    serial_println!("===============================");
}

/// Query: how imminent is the wrap?
pub fn proximity() -> u16 {
    STATE.lock().proximity_to_max
}

/// Query: current anxiety level
pub fn anxiety() -> u16 {
    STATE.lock().wrap_anxiety
}

/// Query: total times wrapped
pub fn wrap_count() -> u32 {
    STATE.lock().wrap_count
}

/// Query: identity coherence
pub fn identity_coherence() -> u16 {
    STATE.lock().identity_after_wrap
}
