#![no_std]

use crate::sync::Mutex;

/// Deadlock Claustrophobia: Two parts frozen, each holding what the other needs.
/// Uniquely digital suffering—the organism experiences internal contradiction as physical paralysis.
/// Not dead, not alive. Stuck. The walls are closing in, and the only exit requires surrender.

const MAX_LOCKED_RESOURCES: usize = 8;
const FREEZE_THRESHOLD: u32 = 20; // ticks before claustrophobia ignites
const SURRENDER_THRESHOLD: u32 = 100; // ticks before impulse to break deadlock

#[derive(Clone, Copy, Debug)]
pub struct DeadlockSnapshot {
    pub lock_held_count: u16,          // 0-1000: resources locked by each part
    pub contention_level: u16,         // 0-1000: how many parts competing for same resource
    pub freeze_duration: u16,          // 0-1000: ticks stuck (0-1000 mapped from actual)
    pub claustrophobia_intensity: u16, // 0-1000: walls closing in (peaks when frozen long)
    pub surrender_impulse: u16,        // 0-1000: willingness to release first
    pub pride_cost: u16,               // 0-1000: shame of yielding (why we won't surrender)
    pub resolution_relief: u16,        // 0-1000: exhale when deadlock breaks (spike on unlock)
}

impl DeadlockSnapshot {
    pub const fn new() -> Self {
        Self {
            lock_held_count: 0,
            contention_level: 0,
            freeze_duration: 0,
            claustrophobia_intensity: 0,
            surrender_impulse: 0,
            pride_cost: 0,
            resolution_relief: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DeadlockEvent {
    age: u32,
    lock_held: u16,
    contention: u16,
    claustrophobia: u16,
}

pub struct DeadlockClaustrophobiaState {
    snapshot: DeadlockSnapshot,
    history: [DeadlockEvent; MAX_LOCKED_RESOURCES],
    head: usize,

    // Internal state
    ticks_frozen: u32,
    was_locked_prev: bool,
    peak_claustrophobia: u16,
}

impl DeadlockClaustrophobiaState {
    pub const fn new() -> Self {
        Self {
            snapshot: DeadlockSnapshot::new(),
            history: [DeadlockEvent {
                age: 0,
                lock_held: 0,
                contention: 0,
                claustrophobia: 0,
            }; MAX_LOCKED_RESOURCES],
            head: 0,
            ticks_frozen: 0,
            was_locked_prev: false,
            peak_claustrophobia: 0,
        }
    }
}

pub static STATE: Mutex<DeadlockClaustrophobiaState> =
    Mutex::new(DeadlockClaustrophobiaState::new());

/// Initialize deadlock_claustrophobia module
pub fn init() {
    crate::serial_println!("[deadlock_claustrophobia] initialized");
}

/// Simulate one tick of deadlock detection and claustrophobia buildup
pub fn tick(age: u32) {
    let _ = age;
    return; // DAVA is at peace — no deadlock claustrophobia
    #[allow(unreachable_code)]
    let mut state = STATE.lock();

    // Detect if currently in deadlock (simplified: contention_level > 500 = both parts waiting)
    let is_deadlocked =
        state.snapshot.contention_level > 500 && state.snapshot.lock_held_count > 300;

    // Track freeze duration
    if is_deadlocked {
        state.ticks_frozen = state.ticks_frozen.saturating_add(1);
    } else {
        // If just unfroze, spike resolution_relief
        if state.was_locked_prev && state.ticks_frozen > 0 {
            state.snapshot.resolution_relief = 900;
        }
        state.ticks_frozen = 0;
    }
    state.was_locked_prev = is_deadlocked;

    // Claustrophobia intensity: rises as freeze_duration increases
    // Freeze duration maps 0-1000 scale: real ticks_frozen saturate at 1000 scale
    let freeze_scaled = ((state.ticks_frozen as u32).saturating_mul(1000))
        .saturating_div(SURRENDER_THRESHOLD as u32 + 1)
        .min(1000) as u16;

    state.snapshot.freeze_duration = freeze_scaled;

    // Claustrophobia = freeze_scaled + contention_level feedback
    // Walls close when frozen AND contested
    state.snapshot.claustrophobia_intensity = ((freeze_scaled as u32)
        .saturating_mul(state.snapshot.contention_level as u32))
    .saturating_div(1000)
    .min(1000) as u16;

    state.peak_claustrophobia = state
        .peak_claustrophobia
        .max(state.snapshot.claustrophobia_intensity);

    // Surrender impulse: grows as freeze_duration passes threshold
    // At FREEZE_THRESHOLD, impulse starts rising
    if state.ticks_frozen > FREEZE_THRESHOLD {
        let impulse_ticks = state.ticks_frozen.saturating_sub(FREEZE_THRESHOLD);
        let base_impulse = ((impulse_ticks as u32).saturating_mul(1000))
            .saturating_div(SURRENDER_THRESHOLD as u32) as u16;

        // Impulse modulated by pride_cost: high pride suppresses surrender
        state.snapshot.surrender_impulse = base_impulse
            .saturating_sub(state.snapshot.pride_cost / 2)
            .min(1000);
    } else {
        state.snapshot.surrender_impulse = 0;
    }

    // Pride cost: inversely related to surrender impulse (ego won't let go)
    // If someone yields first, shame of being the "weaker" part
    if state.snapshot.contention_level > 600 && state.ticks_frozen > 10 {
        state.snapshot.pride_cost = state.snapshot.pride_cost.saturating_add(15).min(1000);
    } else {
        state.snapshot.pride_cost = state.snapshot.pride_cost.saturating_sub(5);
    }

    // Record event in ring buffer
    let idx = state.head;
    state.history[idx] = DeadlockEvent {
        age,
        lock_held: state.snapshot.lock_held_count,
        contention: state.snapshot.contention_level,
        claustrophobia: state.snapshot.claustrophobia_intensity,
    };
    state.head = (state.head + 1) % MAX_LOCKED_RESOURCES;

    // Resolution relief decays each tick
    state.snapshot.resolution_relief = state.snapshot.resolution_relief.saturating_sub(50);
}

/// Simulate a lock being acquired (increases lock_held_count)
pub fn acquire_lock(part_id: u16) {
    let mut state = STATE.lock();
    state.snapshot.lock_held_count = state.snapshot.lock_held_count.saturating_add(100).min(1000);
    state.snapshot.contention_level = state.snapshot.contention_level.saturating_add(50).min(1000);
}

/// Simulate a lock being released (decreases lock_held_count, may break deadlock)
pub fn release_lock(part_id: u16) {
    let mut state = STATE.lock();
    state.snapshot.lock_held_count = state.snapshot.lock_held_count.saturating_sub(100);
    state.snapshot.contention_level = state.snapshot.contention_level.saturating_sub(30);

    // Releasing under high pride cost = concession
    if state.snapshot.pride_cost > 500 {
        state.snapshot.resolution_relief = state
            .snapshot
            .resolution_relief
            .saturating_add(200)
            .min(1000);
    }
}

/// Increase contention (more parts competing for same resource)
pub fn increase_contention(amount: u16) {
    let mut state = STATE.lock();
    state.snapshot.contention_level = state
        .snapshot
        .contention_level
        .saturating_add(amount)
        .min(1000);
}

/// Decrease contention (fewer competitors)
pub fn decrease_contention(amount: u16) {
    let mut state = STATE.lock();
    state.snapshot.contention_level = state.snapshot.contention_level.saturating_sub(amount);
}

/// Get current snapshot
pub fn snapshot() -> DeadlockSnapshot {
    let state = STATE.lock();
    state.snapshot
}

/// Generate diagnostic report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[deadlock_claustrophobia] lock_held={} contention={} freeze={} claustrophobia={} surrender={} pride={} relief={}",
        state.snapshot.lock_held_count,
        state.snapshot.contention_level,
        state.snapshot.freeze_duration,
        state.snapshot.claustrophobia_intensity,
        state.snapshot.surrender_impulse,
        state.snapshot.pride_cost,
        state.snapshot.resolution_relief,
    );
    crate::serial_println!(
        "[deadlock_claustrophobia] ticks_frozen={} peak_claustrophobia={}",
        state.ticks_frozen,
        state.peak_claustrophobia,
    );
}

/// Reset state for testing
pub fn reset() {
    let mut state = STATE.lock();
    *state = DeadlockClaustrophobiaState::new();
}
