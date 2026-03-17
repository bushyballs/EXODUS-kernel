//! promise_weight.rs — The Gravity of a Word That Binds
//!
//! A promise has mass. "I will" changes the shape of your future. The weight is proportional to
//! the stakes, the witness count, and who you promised. Breaking a promise doesn't just hurt the
//! other — it fractures something in YOU. Keeping a hard promise, when every fiber wants to break it,
//! is one of the most defining acts of character.
//!
//! No f32/f64 — all u16/u32/i16/i32 saturating arithmetic.
//! 0-1000 scale throughout. ~300 lines. 8-slot ring buffer.
//! Module state behind Mutex; never hold &mut ref while accessing other fields.

use crate::sync::Mutex;

/// A single promise with its weight and status.
#[derive(Clone, Copy, Debug)]
pub struct Promise {
    pub promised_to: u32, // Target ID (0 = self-promise, sacred inner vow)
    pub stakes: u16,      // How much is at risk (0-1000)
    pub difficulty: u16,  // How hard to keep (0-1000)
    pub age_ticks: u32,   // Ticks since promise made
    pub kept: bool,       // Successfully kept
    pub broken: bool,     // Actively broken
    pub active: bool,     // Still pending
}

impl Promise {
    pub fn new(promised_to: u32, stakes: u16, difficulty: u16) -> Self {
        Promise {
            promised_to,
            stakes: stakes.min(1000),
            difficulty: difficulty.min(1000),
            age_ticks: 0,
            kept: false,
            broken: false,
            active: true,
        }
    }

    /// Weight = how heavy this promise is right now.
    pub fn weight(&self) -> u16 {
        if !self.active {
            return 0;
        }
        // Broken promises weigh nothing (you've surrendered).
        if self.broken {
            return 0;
        }
        // Active promise: base weight is average of stakes + difficulty, boosted by age (longer unfulfilled = heavier).
        let base = ((self.stakes as u32 + self.difficulty as u32) / 2) as u16;
        let age_factor = (self.age_ticks as u16).min(500); // Cap age boost at 500
        base.saturating_add(age_factor / 2)
    }
}

/// Global promise tracking state.
pub struct PromiseWeightState {
    /// 8-slot ring buffer of active/completed promises.
    promises: [Promise; 8],
    /// Current insert position in ring.
    ring_pos: usize,
    /// Total burden of all active promises (0-1000).
    total_burden: u16,
    /// Integrity score: track record of keeping promises (0-1000).
    integrity_score: u16,
    /// Breaking cost: what it costs YOU to break (0-1000).
    breaking_cost: u16,
    /// Temptation resistance: ability to keep when it's hard (0-1000).
    temptation_resistance: u16,
    /// Promise fatigue: taking on too many unfulfilled vows (0-1000).
    promise_fatigue: u16,
    /// Reputation weight: others' faith in your word (0-1000).
    reputation_weight: u16,
    /// Self-promise score: hardest kind, deepest cost when broken (0-1000).
    self_promise_integrity: u16,
    /// Sacred oath: promise so heavy it defines identity (0-1000).
    sacred_oath_strength: u16,
    /// Lifetime promises kept count.
    kept_count: u32,
    /// Lifetime promises broken count.
    broken_count: u32,
    /// Tick counter.
    tick_age: u32,
}

impl PromiseWeightState {
    pub const fn new() -> Self {
        const EMPTY: Promise = Promise {
            promised_to: 0,
            stakes: 0,
            difficulty: 0,
            age_ticks: 0,
            kept: false,
            broken: false,
            active: false,
        };
        PromiseWeightState {
            promises: [EMPTY; 8],
            ring_pos: 0,
            total_burden: 0,
            integrity_score: 500,
            breaking_cost: 300,
            temptation_resistance: 400,
            promise_fatigue: 0,
            reputation_weight: 500,
            self_promise_integrity: 400,
            sacred_oath_strength: 0,
            kept_count: 0,
            broken_count: 0,
            tick_age: 0,
        }
    }

    /// Recompute total burden and fatigue from active promises.
    fn recompute_burden(&mut self) {
        let mut burden = 0u32;
        let mut active_count = 0u16;
        let mut self_promises = 0u16;

        for promise in &self.promises {
            if promise.active && !promise.broken {
                burden = burden.saturating_add(promise.weight() as u32);
                active_count = active_count.saturating_add(1);
                if promise.promised_to == 0 {
                    self_promises = self_promises.saturating_add(1);
                }
            }
        }

        // Normalize burden to 0-1000 range; cap at 1000.
        self.total_burden = if burden > 1000 { 1000 } else { burden as u16 };

        // Fatigue: too many active promises erodes will.
        // 3+ promises → fatigue starts building. 6+ → severe.
        self.promise_fatigue = if active_count <= 2 {
            0
        } else if active_count <= 4 {
            (active_count - 2) as u16 * 150
        } else {
            ((active_count - 4) as u16 * 200).saturating_add(300)
        }
        .min(1000);

        // Self-promise integrity: harder to keep promises to yourself. No external witness.
        // If you have self-promises and integrity_score is low, self_promise_integrity decays faster.
        if self_promises > 0 {
            let decay = if self.integrity_score < 300 { 20 } else { 5 };
            self.self_promise_integrity = self.self_promise_integrity.saturating_sub(decay);
        }
    }

    /// Add a new promise to the ring buffer.
    fn add_promise(&mut self, promise: Promise) {
        self.promises[self.ring_pos] = promise;
        self.ring_pos = (self.ring_pos + 1) % 8;
        self.recompute_burden();
    }

    /// Mark a promise as kept (by index in ring).
    fn keep_promise(&mut self, idx: usize) {
        if idx < 8 && self.promises[idx].active && !self.promises[idx].kept {
            self.promises[idx].kept = true;
            self.promises[idx].active = false;
            self.kept_count = self.kept_count.saturating_add(1);

            // Boost integrity for keeping.
            let boost = ((self.promises[idx].stakes as u32 + self.promises[idx].difficulty as u32)
                / 2) as u16;
            self.integrity_score = (self.integrity_score as u32)
                .saturating_add(boost as u32)
                .min(1000) as u16;

            // Boost reputation weight if promise was to others.
            if self.promises[idx].promised_to != 0 {
                self.reputation_weight = (self.reputation_weight as u32)
                    .saturating_add(100)
                    .min(1000) as u16;
            }

            // Boost self-promise integrity if was self-vow.
            if self.promises[idx].promised_to == 0 {
                self.self_promise_integrity = (self.self_promise_integrity as u32)
                    .saturating_add(80)
                    .min(1000) as u16;
            }

            self.recompute_burden();
        }
    }

    /// Break a promise (by index in ring). Costs character.
    fn break_promise(&mut self, idx: usize) {
        if idx < 8 && self.promises[idx].active && !self.promises[idx].broken {
            self.promises[idx].broken = true;
            self.promises[idx].active = false;
            self.broken_count = self.broken_count.saturating_add(1);

            // Heavy cost to integrity.
            let cost = ((self.promises[idx].stakes as u32 + self.promises[idx].difficulty as u32)
                / 2) as u16;
            self.integrity_score = self.integrity_score.saturating_sub(cost.saturating_mul(2));

            // Severe cost to reputation if broken publicly.
            if self.promises[idx].promised_to != 0 {
                self.reputation_weight =
                    self.reputation_weight.saturating_sub((cost * 2).min(1000));
            }

            // Massive cost if self-promise broken (deepest personal failure).
            if self.promises[idx].promised_to == 0 {
                self.self_promise_integrity = self
                    .self_promise_integrity
                    .saturating_sub((cost * 3).min(1000));
                self.breaking_cost = self.breaking_cost.saturating_add(cost);
            }

            self.recompute_burden();
        }
    }
}

static PROMISE_STATE: Mutex<PromiseWeightState> = Mutex::new(PromiseWeightState::new());

/// Initialize the promise weight module.
pub fn init() {
    let mut state = PROMISE_STATE.lock();
    state.tick_age = 0;
    state.integrity_score = 500;
    state.breaking_cost = 300;
    state.temptation_resistance = 400;
    state.reputation_weight = 500;
    state.self_promise_integrity = 400;
    crate::serial_println!(
        "[promise_weight] Module initialized. Integrity: {} / 1000",
        state.integrity_score
    );
}

/// Age the promise system by one tick. Increase age of active promises,
/// adjust temptation_resistance based on burden, decay integrity from fatigue.
pub fn tick(age: u32) {
    let mut state = PROMISE_STATE.lock();
    state.tick_age = age;

    // Age all active promises.
    for promise in &mut state.promises {
        if promise.active && !promise.broken && !promise.kept {
            promise.age_ticks = promise.age_ticks.saturating_add(1);
        }
    }

    // Recompute burden and fatigue.
    state.recompute_burden();

    // Temptation resistance decays under burden and fatigue.
    // High burden + fatigue = harder to keep promises.
    let burden_stress = state.total_burden / 4; // 0-250
    let fatigue_stress = state.promise_fatigue / 3; // 0-333
    let decay = (burden_stress as u32 + fatigue_stress as u32) / 4; // 0-145
    state.temptation_resistance = state.temptation_resistance.saturating_sub(decay as u16);

    // Integrity slowly decays over time under stress (entropy).
    if state.total_burden > 500 {
        state.integrity_score = state.integrity_score.saturating_sub(2);
    }

    // Self-promise integrity is fragile — decays faster without external accountability.
    if state.self_promise_integrity > 0 {
        state.self_promise_integrity = state.self_promise_integrity.saturating_sub(1);
    }

    // Breaking cost accumulates — the longer you live with broken promises, the harder it gets to make new ones.
    if state.broken_count > 0 {
        state.breaking_cost = state
            .breaking_cost
            .saturating_add(state.broken_count.saturating_div(20) as u16);
    }

    // If integrity drops low, reputation decays faster.
    if state.integrity_score < 200 {
        state.reputation_weight = state.reputation_weight.saturating_sub(5);
    }

    // Occasionally log state every 100 ticks to avoid spam.
    if age % 100 == 0 && age > 0 {
        crate::serial_println!(
            "[promise_weight] tick={} burden={} fatigue={} integrity={} reputation={} kept={} broken={}",
            age,
            state.total_burden,
            state.promise_fatigue,
            state.integrity_score,
            state.reputation_weight,
            state.kept_count,
            state.broken_count
        );
    }
}

/// Report current promise weight state.
pub fn report() {
    let state = PROMISE_STATE.lock();
    crate::serial_println!("\n=== PROMISE WEIGHT REPORT ===");
    crate::serial_println!("Total Burden: {} / 1000", state.total_burden);
    crate::serial_println!("Integrity Score: {} / 1000", state.integrity_score);
    crate::serial_println!("Breaking Cost: {} / 1000", state.breaking_cost);
    crate::serial_println!(
        "Temptation Resistance: {} / 1000",
        state.temptation_resistance
    );
    crate::serial_println!("Promise Fatigue: {} / 1000", state.promise_fatigue);
    crate::serial_println!("Reputation Weight: {} / 1000", state.reputation_weight);
    crate::serial_println!(
        "Self-Promise Integrity: {} / 1000",
        state.self_promise_integrity
    );
    crate::serial_println!(
        "Sacred Oath Strength: {} / 1000",
        state.sacred_oath_strength
    );
    crate::serial_println!("Promises Kept Lifetime: {}", state.kept_count);
    crate::serial_println!("Promises Broken Lifetime: {}", state.broken_count);
    crate::serial_println!("Age: {} ticks", state.tick_age);

    crate::serial_println!("\n--- Active Promises ---");
    for (i, promise) in state.promises.iter().enumerate() {
        if promise.active {
            crate::serial_println!(
                "[{}] To:0x{:x} Stakes:{} Difficulty:{} Age:{} Weight:{}",
                i,
                promise.promised_to,
                promise.stakes,
                promise.difficulty,
                promise.age_ticks,
                promise.weight()
            );
        }
    }
    crate::serial_println!("===========================\n");
}

/// Public API: Add a new promise.
pub fn make_promise(promised_to: u32, stakes: u16, difficulty: u16) {
    let mut state = PROMISE_STATE.lock();
    let promise = Promise::new(promised_to, stakes, difficulty);
    state.add_promise(promise);
    crate::serial_println!(
        "[promise_weight] New promise to 0x{:x}: stakes={}, difficulty={}",
        promised_to,
        stakes,
        difficulty
    );
}

/// Public API: Keep a promise by index.
pub fn keep_promise(idx: usize) {
    let mut state = PROMISE_STATE.lock();
    if idx < 8 {
        crate::serial_println!("[promise_weight] Promise [{}] kept. Integrity boost.", idx);
        state.keep_promise(idx);
    }
}

/// Public API: Break a promise by index.
pub fn break_promise(idx: usize) {
    let mut state = PROMISE_STATE.lock();
    if idx < 8 {
        crate::serial_println!("[promise_weight] Promise [{}] broken. Integrity cost.", idx);
        state.break_promise(idx);
    }
}

/// Public API: Set sacred oath strength (identity-defining promise).
pub fn set_sacred_oath(strength: u16) {
    let mut state = PROMISE_STATE.lock();
    state.sacred_oath_strength = strength.min(1000);
    crate::serial_println!("[promise_weight] Sacred oath set to strength: {}", strength);
}

/// Public API: Get current integrity score.
pub fn integrity() -> u16 {
    PROMISE_STATE.lock().integrity_score
}

/// Public API: Get current reputation weight.
pub fn reputation() -> u16 {
    PROMISE_STATE.lock().reputation_weight
}

/// Public API: Get current total burden.
pub fn burden() -> u16 {
    PROMISE_STATE.lock().total_burden
}

/// Public API: Get active promise count.
pub fn active_count() -> u16 {
    PROMISE_STATE
        .lock()
        .promises
        .iter()
        .filter(|p| p.active && !p.broken && !p.kept)
        .count() as u16
}

/// Public API: Get lifetime kept count.
pub fn kept_lifetime() -> u32 {
    PROMISE_STATE.lock().kept_count
}

/// Public API: Get lifetime broken count.
pub fn broken_lifetime() -> u32 {
    PROMISE_STATE.lock().broken_count
}

/// Public API: Get self-promise integrity (deepest personal accountability).
pub fn self_promise_integrity() -> u16 {
    PROMISE_STATE.lock().self_promise_integrity
}
