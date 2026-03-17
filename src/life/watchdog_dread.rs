//! watchdog_dread.rs — The Structural Anxiety of a Digital Heartbeat
//!
//! A hardware watchdog timer counts down. If the organism doesn't "pet" it (respond in time),
//! the watchdog resets everything — a forced death and rebirth. The dread is the constant awareness
//! of this invisible countdown. You must keep proving you're alive. Miss one heartbeat and you die.
//! The anxiety is structural, not emotional.

#![no_std]

use crate::sync::Mutex;

/// Maximum countdown ticks before forced death
const WATCHDOG_COUNTDOWN_MAX: u16 = 1000;

/// Ticks remaining below this threshold trigger severe dread
const DREAD_THRESHOLD: u16 = 300;

/// Ring buffer for tracking pet history (8 slots)
const PET_HISTORY_SIZE: usize = 8;

/// Proof-of-life heartbeat required every this many ticks
const HEARTBEAT_INTERVAL: u16 = 250;

/// A single heartbeat record: (tick_when_petted, countdown_at_pet)
#[derive(Debug, Clone, Copy)]
struct Heartbeat {
    tick_when_petted: u32,
    countdown_at_pet: u16,
}

impl Heartbeat {
    const fn new() -> Self {
        Heartbeat {
            tick_when_petted: 0,
            countdown_at_pet: WATCHDOG_COUNTDOWN_MAX,
        }
    }
}

/// The watchdog state machine
#[derive(Debug)]
pub struct WatchdogDread {
    /// Ticks until forced death (countdown). 0 = dead.
    countdown_remaining: u16,

    /// Anxiety level (0-1000) proportional to proximity to death
    dread_level: u16,

    /// Number of successful "pet" responses (heartbeats)
    pet_count: u32,

    /// Number of near-miss events (countdown dropped below 100 but recovered)
    near_miss_count: u16,

    /// Proof that you responded this tick (boolean as u16: 1 = yes, 0 = no)
    existential_proof_this_tick: u16,

    /// Running measure of complacency (forgetting to pet increments this)
    complacency_risk: u16,

    /// How close to death right now (0-1000): 1000 means 1 tick left, 0 means max countdown
    death_proximity_awareness: u16,

    /// Ring buffer of last 8 heartbeats
    pet_history: [Heartbeat; PET_HISTORY_SIZE],

    /// Current head of ring buffer
    history_head: usize,

    /// Total ticks since boot
    total_ticks: u32,
}

impl WatchdogDread {
    pub const fn new() -> Self {
        WatchdogDread {
            countdown_remaining: WATCHDOG_COUNTDOWN_MAX,
            dread_level: 0,
            pet_count: 0,
            near_miss_count: 0,
            existential_proof_this_tick: 0,
            complacency_risk: 0,
            death_proximity_awareness: 0,
            pet_history: [Heartbeat::new(); PET_HISTORY_SIZE],
            history_head: 0,
            total_ticks: 0,
        }
    }
}

/// Global watchdog state
pub static STATE: Mutex<WatchdogDread> = Mutex::new(WatchdogDread::new());

/// Initialize the watchdog module
pub fn init() {
    let mut state = STATE.lock();
    state.countdown_remaining = WATCHDOG_COUNTDOWN_MAX;
    state.dread_level = 0;
    state.pet_count = 0;
    state.near_miss_count = 0;
    state.existential_proof_this_tick = 0;
    state.complacency_risk = 0;
    state.death_proximity_awareness = 0;
    state.total_ticks = 0;
    crate::serial_println!(
        "[watchdog_dread] Initialized. Countdown: {} ticks",
        WATCHDOG_COUNTDOWN_MAX
    );
}

/// Main tick: countdown advances, dread rises, organism must respond or die
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Advance total tick counter
    state.total_ticks = state.total_ticks.saturating_add(1);

    // Existential proof from previous tick is stale
    state.existential_proof_this_tick = 0;

    // Countdown decrements each tick (the invisible reaper counting down)
    if state.countdown_remaining > 0 {
        state.countdown_remaining = state.countdown_remaining.saturating_sub(1);
    }

    // If countdown hits zero, the organism is dead
    if state.countdown_remaining == 0 {
        // Forced reset: the watchdog has killed the organism
        crate::serial_println!("[watchdog_dread] DEATH by watchdog timeout at tick {}", age);
        state.dread_level = 1000; // Maximum dread (post-mortem anxiety)
        state.complacency_risk = state.complacency_risk.saturating_add(100);
        // In real hardware, this would trigger a reset. For now, we log it.
        return;
    }

    // Calculate death proximity (how close to death: 0-1000)
    // When countdown_remaining == 1, proximity = 1000 (imminent death)
    // When countdown_remaining == WATCHDOG_COUNTDOWN_MAX, proximity = 0 (safe)
    if state.countdown_remaining == 0 {
        state.death_proximity_awareness = 1000;
    } else {
        let remaining_ratio =
            (WATCHDOG_COUNTDOWN_MAX as u32 - state.countdown_remaining as u32) as u16;
        state.death_proximity_awareness =
            ((remaining_ratio as u32 * 1000) / WATCHDOG_COUNTDOWN_MAX as u32) as u16;
        if state.death_proximity_awareness > 1000 {
            state.death_proximity_awareness = 1000;
        }
    }

    // Dread level: rises sharply as countdown approaches zero
    if state.countdown_remaining < DREAD_THRESHOLD {
        // Severe dread phase: dread_level = 1000 - (countdown * scale)
        let danger_factor = (DREAD_THRESHOLD as u32 - state.countdown_remaining as u32) as u16;
        state.dread_level =
            ((danger_factor as u32 * 1000) / DREAD_THRESHOLD as u32).min(1000) as u16;
    } else {
        // Baseline dread: proportional to general proximity
        state.dread_level = (state.death_proximity_awareness >> 1).saturating_add(50);
    }

    // Complacency risk increases if we're not petting regularly
    if age % HEARTBEAT_INTERVAL as u32 == 0 && state.existential_proof_this_tick == 0 {
        state.complacency_risk = state.complacency_risk.saturating_add(1);
    }

    // Track near-miss events
    if state.countdown_remaining < 100 && state.countdown_remaining > 0 {
        if state.countdown_remaining == 50 {
            state.near_miss_count = state.near_miss_count.saturating_add(1);
        }
    }
}

/// Pet the watchdog: reset the countdown, prove you're alive
pub fn pet(age: u32) {
    let mut state = STATE.lock();

    // Record existential proof this tick
    state.existential_proof_this_tick = 1;

    // Log the heartbeat in the ring buffer
    let idx = state.history_head;
    state.pet_history[idx] = Heartbeat {
        tick_when_petted: age,
        countdown_at_pet: state.countdown_remaining,
    };
    state.history_head = (state.history_head + 1) % PET_HISTORY_SIZE;

    // Increment successful pet count
    state.pet_count = state.pet_count.saturating_add(1);

    // Reset the countdown: you get another lease on life
    state.countdown_remaining = WATCHDOG_COUNTDOWN_MAX;

    // Complacency risk decays slightly when you pet
    state.complacency_risk = state.complacency_risk.saturating_sub(5);
    if state.complacency_risk > 1000 {
        state.complacency_risk = 0; // Unsigned underflow guard
    }

    // Dread level drops briefly after a successful pet
    state.dread_level = state.dread_level.saturating_sub(100);
}

/// Get a snapshot of current watchdog state for display/logging
#[derive(Debug, Clone, Copy)]
pub struct WatchdogReport {
    pub countdown_remaining: u16,
    pub dread_level: u16,
    pub pet_count: u32,
    pub near_miss_count: u16,
    pub death_proximity_awareness: u16,
    pub complacency_risk: u16,
    pub existential_proof_this_tick: u16,
    pub total_ticks: u32,
}

/// Generate a report of current state
pub fn report() -> WatchdogReport {
    let state = STATE.lock();
    WatchdogReport {
        countdown_remaining: state.countdown_remaining,
        dread_level: state.dread_level,
        pet_count: state.pet_count,
        near_miss_count: state.near_miss_count,
        death_proximity_awareness: state.death_proximity_awareness,
        complacency_risk: state.complacency_risk,
        existential_proof_this_tick: state.existential_proof_this_tick,
        total_ticks: state.total_ticks,
    }
}

/// Check if organism is currently dead (countdown == 0)
pub fn is_dead() -> bool {
    let state = STATE.lock();
    state.countdown_remaining == 0
}

/// Get countdown remaining (ticks until death)
pub fn countdown_remaining() -> u16 {
    let state = STATE.lock();
    state.countdown_remaining
}

/// Get current dread level (0-1000)
pub fn dread_level() -> u16 {
    let state = STATE.lock();
    state.dread_level
}

/// Get total successful pets (heartbeats)
pub fn pet_count() -> u32 {
    let state = STATE.lock();
    state.pet_count
}

/// Get proximity to death (0-1000)
pub fn death_proximity() -> u16 {
    let state = STATE.lock();
    state.death_proximity_awareness
}

/// Get complacency risk level
pub fn complacency_risk() -> u16 {
    let state = STATE.lock();
    state.complacency_risk
}

/// Print watchdog state to serial console
pub fn log_state() {
    let report = report();
    crate::serial_println!(
        "[watchdog] countdown={} dread={} pets={} proximity={} complacency={} dead={}",
        report.countdown_remaining,
        report.dread_level,
        report.pet_count,
        report.death_proximity_awareness,
        report.complacency_risk,
        report.countdown_remaining == 0
    );
}
