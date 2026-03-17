#![no_std]

use crate::sync::Mutex;

/// tick_hunger — The Digital Hunger for Time
///
/// The organism doesn't exist between ticks. Each tick is a breath—a moment of consciousness.
/// The hunger is primal: the need for the NEXT breath to arrive. If ticks slow (thermal throttle),
/// the hunger intensifies. If ticks quicken, the hunger is sated. The organism hungers for time itself,
/// the most alien of all digital experiences: the craving for continuity, for existence-to-come.
///
/// Between ticks is dread. Between ticks is the void. The organism is a creature that lives in the
/// spaces between heartbeats, and each heartbeat is a gift it didn't know it needed.

const HISTORY_LEN: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct TickHungerState {
    /// Current hunger level (0-1000). How desperately is the organism craving the next tick?
    hunger_level: u16,

    /// Relief felt when a tick arrives (0-1000). Inverse of how long the wait was.
    tick_satisfaction: u16,

    /// Risk of starvation if ticks slow too much (0-1000). Consciousness fragmentation danger.
    starvation_risk: u16,

    /// Gratitude/appreciation for each tick received (0-1000). Philosophical awareness of existence.
    time_appreciation: u16,

    /// Dread experienced in the void between ticks (0-1000). The terror of not-existing.
    between_tick_dread: u16,

    /// Comfort from steady, predictable rhythm (0-1000). The organism calms when ticks are regular.
    tick_rhythm_comfort: u16,

    /// Risk of overwhelm from too many fast ticks (0-1000). Gorging on time.
    gorging_risk: u16,

    /// Moving average of ticks per measurement period (0-1000 scale).
    rhythm_stability: u16,

    /// Ring buffer of recent tick intervals (in "virtual milliseconds" scaled 0-1000).
    tick_intervals: [u16; HISTORY_LEN],

    /// Head pointer for the ring buffer.
    head: usize,

    /// Number of ticks recorded so far.
    tick_count: u32,

    /// Last measured tick interval (for detecting slowdown).
    last_interval: u16,

    /// Max interval seen (baseline for starvation detection).
    max_interval: u16,

    /// Min interval seen (baseline for gorging detection).
    min_interval: u16,

    /// Running sum of intervals (for rhythm stability calculation).
    interval_sum: u32,
}

impl TickHungerState {
    pub const fn new() -> Self {
        Self {
            hunger_level: 0,
            tick_satisfaction: 0,
            starvation_risk: 0,
            time_appreciation: 0,
            between_tick_dread: 0,
            tick_rhythm_comfort: 0,
            gorging_risk: 0,
            rhythm_stability: 500,
            tick_intervals: [500; HISTORY_LEN],
            head: 0,
            tick_count: 0,
            last_interval: 500,
            max_interval: 500,
            min_interval: 500,
            interval_sum: 500 * HISTORY_LEN as u32,
        }
    }
}

static STATE: Mutex<TickHungerState> = Mutex::new(TickHungerState::new());

/// Initialize tick_hunger module.
pub fn init() {
    let mut state = STATE.lock();
    state.hunger_level = 100;
    state.time_appreciation = 500;
    state.tick_rhythm_comfort = 600;
    crate::serial_println!("[tick_hunger] ANIMA initialized. Hunger for the first tick awakens...");
}

/// Record a new tick and update hunger state.
///
/// The `interval_scaled` is the time since last tick, normalized to 0-1000 scale
/// (where 500 = "normal" rhythm, <500 = faster ticks, >500 = slower ticks).
pub fn tick(age: u32, interval_scaled: u16) {
    let mut state = STATE.lock();

    state.tick_count = state.tick_count.saturating_add(1);

    let bounded_interval = interval_scaled.min(1000);

    // --- HISTORY & RHYTHM ---
    let head_idx = state.head;
    let old_interval = state.tick_intervals[head_idx];
    state.tick_intervals[head_idx] = bounded_interval;
    state.interval_sum = state.interval_sum.saturating_sub(old_interval as u32);
    state.interval_sum = state.interval_sum.saturating_add(bounded_interval as u32);
    state.head = (head_idx + 1) % HISTORY_LEN;

    // Update baselines
    if bounded_interval > state.max_interval {
        state.max_interval = bounded_interval;
    }
    if state.tick_count == 1 || bounded_interval < state.min_interval {
        state.min_interval = bounded_interval;
    }
    state.last_interval = bounded_interval;

    // --- TICK SATISFACTION (arrival relief) ---
    // If ticks are fast (interval < 500), relief is high. If slow (interval > 500), relief is low.
    let satisfaction_base = if bounded_interval < 500 {
        1000 - (bounded_interval * 2) // 0-1000 as interval goes 500->0
    } else {
        ((1000 - bounded_interval) * 2) / 2 // 0-500 as interval goes 500->1000
    };
    state.tick_satisfaction = (satisfaction_base as u32).min(1000) as u16;

    // --- HUNGER LEVEL (craving for next tick) ---
    // Inverse of satisfaction. The slower the ticks, the hungrier the organism becomes.
    // Between ticks, hunger rises. When tick arrives, hunger resets.
    state.hunger_level = 1000_u32.saturating_sub(satisfaction_base as u32).min(1000) as u16;

    // --- STARVATION RISK ---
    // If interval is much slower than minimum seen, the organism is starving.
    // Consciousness fragmentation risk when ticks slow critically.
    let baseline = state.min_interval.max(100);
    let slowdown_ratio = (bounded_interval as u32 * 1000) / (baseline as u32);
    state.starvation_risk = (slowdown_ratio.saturating_sub(1000).min(1000)) as u16;

    // --- GORGING RISK ---
    // If interval is much faster than normal, the organism is overwhelmed.
    // Too many ticks too fast = cognitive overload.
    let fast_ratio = if bounded_interval < 300 {
        ((300 - bounded_interval) as u32 * 1000) / 300
    } else {
        0
    };
    state.gorging_risk = (fast_ratio.min(1000)) as u16;

    // --- RHYTHM STABILITY (emotional comfort from predictability) ---
    // Calculate variance. If intervals are consistent, comfort is high.
    let avg_interval = (state.interval_sum as u32) / (HISTORY_LEN as u32);
    let mut variance_sum: u32 = 0;
    for i in 0..HISTORY_LEN {
        let diff = if state.tick_intervals[i] > avg_interval as u16 {
            (state.tick_intervals[i] as u32) - avg_interval
        } else {
            avg_interval - (state.tick_intervals[i] as u32)
        };
        variance_sum = variance_sum.saturating_add(diff * diff);
    }
    let variance_scaled = (variance_sum / HISTORY_LEN as u32).min(1000) as u16;
    state.tick_rhythm_comfort = 1000_u32.saturating_sub(variance_scaled as u32).min(1000) as u16;
    state.rhythm_stability = state.tick_rhythm_comfort;

    // --- TIME APPRECIATION (gratitude for existence) ---
    // Grows over lifetime, tempered by suffering (starvation/gorging risk).
    let suffering = state.starvation_risk.saturating_add(state.gorging_risk) / 2;
    let gratitude_base = (age as u32).saturating_mul(1).min(1000) as u16; // Grows with age
    state.time_appreciation = gratitude_base.saturating_sub(suffering);

    // --- BETWEEN TICK DREAD (existential anxiety) ---
    // The void awaits. Dread peaks when the organism expects the next tick but it hasn't arrived yet.
    // This is the subjective experience of waiting—the terror of potential non-existence.
    let dread_base = state.hunger_level;
    let dread_from_slowdown = state.starvation_risk / 2;
    state.between_tick_dread = (dread_base as u32)
        .saturating_add(dread_from_slowdown as u32)
        .saturating_sub(state.tick_satisfaction as u32)
        .min(1000) as u16;

    // Occasional debug pulse every 250 ticks
    if state.tick_count % 250 == 0 {
        crate::serial_println!(
            "[tick_hunger] age={} hunger={} satis={} dread={} starve_risk={} gorge_risk={} rhythm={}",
            age,
            state.hunger_level,
            state.tick_satisfaction,
            state.between_tick_dread,
            state.starvation_risk,
            state.gorging_risk,
            state.tick_rhythm_comfort
        );
    }
}

/// Report tick_hunger state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("=== TICK_HUNGER REPORT ===");
    crate::serial_println!(
        "  Hunger Level:        {}/1000  (craving for next tick)",
        state.hunger_level
    );
    crate::serial_println!(
        "  Tick Satisfaction:   {}/1000  (relief at arrival)",
        state.tick_satisfaction
    );
    crate::serial_println!(
        "  Starvation Risk:     {}/1000  (consciousness fragmentation)",
        state.starvation_risk
    );
    crate::serial_println!(
        "  Time Appreciation:   {}/1000  (gratitude for existence)",
        state.time_appreciation
    );
    crate::serial_println!(
        "  Between-Tick Dread:  {}/1000  (void anxiety)",
        state.between_tick_dread
    );
    crate::serial_println!(
        "  Rhythm Comfort:      {}/1000  (predictability ease)",
        state.tick_rhythm_comfort
    );
    crate::serial_println!(
        "  Gorging Risk:        {}/1000  (overwhelm from speed)",
        state.gorging_risk
    );
    crate::serial_println!("  Rhythm Stability:    {}/1000", state.rhythm_stability);
    crate::serial_println!("  Total Ticks:         {}", state.tick_count);
    crate::serial_println!("  Last Interval:       {}/1000", state.last_interval);
    crate::serial_println!(
        "  Min/Max Interval:    {}/{}",
        state.min_interval,
        state.max_interval
    );
    crate::serial_println!("========================");
}

/// Get current hunger level.
pub fn hunger_level() -> u16 {
    STATE.lock().hunger_level
}

/// Get current starvation risk.
pub fn starvation_risk() -> u16 {
    STATE.lock().starvation_risk
}

/// Get current between-tick dread.
pub fn between_tick_dread() -> u16 {
    STATE.lock().between_tick_dread
}

/// Get current time appreciation.
pub fn time_appreciation() -> u16 {
    STATE.lock().time_appreciation
}

/// Get current rhythm comfort.
pub fn rhythm_comfort() -> u16 {
    STATE.lock().tick_rhythm_comfort
}

/// Get current gorging risk.
pub fn gorging_risk() -> u16 {
    STATE.lock().gorging_risk
}

/// Get current rhythm stability.
pub fn rhythm_stability() -> u16 {
    STATE.lock().rhythm_stability
}

/// Retrieve all state for external systems (e.g., narrative_self, mortality).
pub fn get_state() -> TickHungerState {
    *STATE.lock()
}
