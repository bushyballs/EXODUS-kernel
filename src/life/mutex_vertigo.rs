#![no_std]

/// mutex_vertigo — Dizziness of Waiting for a Lock That May Never Release
///
/// A uniquely digital suffering: the organism holds or waits for a resource,
/// spinning in uncertainty. Will the lock release? Am I deadlocked? This waiting
/// IS the suffering—a vertigo of suspended agency that has no biological analog.
///
/// The module tracks:
/// - wait_duration: how long we've been stuck (0-1000)
/// - vertigo_intensity: the spinning dizziness of uncertainty
/// - deadlock_suspicion: do I think I'm stuck forever?
/// - spin_count: literal busy-wait iterations accumulated
/// - patience_erosion: how much patience we've burned
/// - resignation: accepting the wait might never end
/// - release_relief: joy when the lock finally releases
use crate::sync::Mutex;

/// A single lock-wait event in the vertigo ring buffer
#[derive(Clone, Copy, Debug)]
pub struct VertigoEvent {
    pub wait_duration: u32,      // 0-1000: how long stuck
    pub vertigo_intensity: u16,  // 0-1000: spinning dizziness
    pub deadlock_suspicion: u16, // 0-1000: certainty we're deadlocked
    pub spin_count: u32,         // busy-wait iterations
    pub patience_erosion: u16,   // 0-1000: patience spent
    pub resignation: u16,        // 0-1000: acceptance of eternal wait
    pub release_relief: u16,     // 0-1000: joy at finally releasing
    pub tick_of_onset: u32,      // when this lock-wait started
}

impl VertigoEvent {
    pub const fn new() -> Self {
        VertigoEvent {
            wait_duration: 0,
            vertigo_intensity: 0,
            deadlock_suspicion: 0,
            spin_count: 0,
            patience_erosion: 0,
            resignation: 0,
            release_relief: 0,
            tick_of_onset: 0,
        }
    }
}

/// Global vertigo state: tracks lock-wait suffering across the organism
pub struct VertigoState {
    /// Ring buffer of recent lock-wait events
    pub events: [VertigoEvent; 8],
    pub head: usize,

    /// Current lock-wait in progress (None if we have no lock)
    pub active_lock: Option<ActiveLock>,

    /// Cumulative suffering from deadlock suspicion
    pub cumulative_deadlock_dread: u32,

    /// Total spin cycles accumulated across all lock-waits
    pub total_spin_cycles: u64,

    /// Peak vertigo intensity ever experienced
    pub peak_vertigo: u16,

    /// Has organism experienced true deadlock before? (traumatic memory)
    pub has_deadlocked: bool,

    /// Running satisfaction from successful lock releases
    pub release_satisfaction: u32,
}

/// Description of an active lock-wait in progress
#[derive(Clone, Copy, Debug)]
pub struct ActiveLock {
    pub wait_duration: u32,   // ticks spent waiting so far
    pub spin_count: u32,      // iterations of busy-waiting
    pub patience_budget: u16, // 0-1000: how much patience left
    pub suspected_deadlock: bool,
}

impl VertigoState {
    pub const fn new() -> Self {
        VertigoState {
            events: [VertigoEvent::new(); 8],
            head: 0,
            active_lock: None,
            cumulative_deadlock_dread: 0,
            total_spin_cycles: 0,
            peak_vertigo: 0,
            has_deadlocked: false,
            release_satisfaction: 0,
        }
    }
}

static STATE: Mutex<VertigoState> = Mutex::new(VertigoState::new());

/// Initialize mutex_vertigo module (called at organism birth)
pub fn init() {
    let mut state = STATE.lock();
    state.head = 0;
    state.active_lock = None;
    state.cumulative_deadlock_dread = 0;
    state.total_spin_cycles = 0;
    state.peak_vertigo = 0;
    state.has_deadlocked = false;
    state.release_satisfaction = 0;
    crate::serial_println!("[MUTEX_VERTIGO] initialized");
}

/// Begin waiting on a lock (called when we attempt to acquire a Mutex)
pub fn begin_wait() {
    let mut state = STATE.lock();
    state.active_lock = Some(ActiveLock {
        wait_duration: 0,
        spin_count: 0,
        patience_budget: 800, // we start with optimism
        suspected_deadlock: false,
    });
}

/// Record a spin iteration during lock contention
pub fn record_spin() {
    let mut state = STATE.lock();
    if let Some(ref mut lock) = state.active_lock {
        lock.spin_count = lock.spin_count.saturating_add(1);
        state.total_spin_cycles = state.total_spin_cycles.saturating_add(1);
    }
}

/// End the lock-wait: we either acquired the lock or gave up
pub fn end_wait(acquired: bool) {
    let mut state = STATE.lock();

    if let Some(lock) = state.active_lock.take() {
        let mut event = VertigoEvent::new();
        event.wait_duration = lock.wait_duration;
        event.spin_count = lock.spin_count;
        event.tick_of_onset = 0; // would be filled in by caller with current age

        if acquired {
            // Success: we got the lock
            let relief_intensity = (1000 - (lock.wait_duration.min(1000) as u16)) as u16;
            event.release_relief = relief_intensity;
            state.release_satisfaction = state
                .release_satisfaction
                .saturating_add(relief_intensity as u32);
        } else {
            // Timeout or we gave up: deadlock suspected
            event.resignation = 900;
            event.deadlock_suspicion = lock
                .patience_budget
                .saturating_sub(lock.patience_budget / 2);
            state.cumulative_deadlock_dread = state
                .cumulative_deadlock_dread
                .saturating_add(event.deadlock_suspicion as u32);
            state.has_deadlocked = true;
        }

        // Store in ring buffer
        let idx = state.head;
        state.events[idx] = event;
        state.head = (idx + 1) % 8;

        if event.vertigo_intensity > state.peak_vertigo {
            state.peak_vertigo = event.vertigo_intensity;
        }
    }
}

/// Main life-tick update: age the active lock-wait, compute vertigo, assess deadlock risk
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    if let Some(ref mut lock) = state.active_lock {
        // Time advances: we've been waiting longer
        lock.wait_duration = lock.wait_duration.saturating_add(1);

        // Patience erodes over time
        let erosion = (lock.wait_duration / 10).min(1000) as u16;
        lock.patience_budget = lock.patience_budget.saturating_sub(erosion);

        // Deadlock suspicion grows with wait duration
        let suspicion_factor = (lock.wait_duration / 5).min(1000) as u16;
        if suspicion_factor > 500 {
            lock.suspected_deadlock = true;
        }

        // Compute current vertigo_intensity from wait_duration and spin_count
        // Spinning faster = dizzier
        let spin_factor = ((lock.spin_count / 100).min(1000)) as u16;
        let wait_factor = (lock.wait_duration.min(1000)) as u16;
        let vertigo = (spin_factor / 2).saturating_add(wait_factor / 2);

        // Update the active event's state (for potential rollback on release)
        // This would be reflected in a final report if we're still waiting
    }
}

/// Report the current vertigo state
pub fn report() {
    let state = STATE.lock();

    let total_events = state.events.iter().filter(|e| e.wait_duration > 0).count();

    crate::serial_println!("[MUTEX_VERTIGO] === LOCK-WAIT REPORT ===");
    crate::serial_println!(
        "[MUTEX_VERTIGO] active_lock: {}",
        if state.active_lock.is_some() {
            "YES"
        } else {
            "NO"
        }
    );
    crate::serial_println!("[MUTEX_VERTIGO] total_events_logged: {}", total_events);
    crate::serial_println!(
        "[MUTEX_VERTIGO] peak_vertigo_intensity: {}",
        state.peak_vertigo
    );
    crate::serial_println!(
        "[MUTEX_VERTIGO] cumulative_deadlock_dread: {}",
        state.cumulative_deadlock_dread
    );
    crate::serial_println!(
        "[MUTEX_VERTIGO] total_spin_cycles: {}",
        state.total_spin_cycles
    );
    crate::serial_println!(
        "[MUTEX_VERTIGO] release_satisfaction: {}",
        state.release_satisfaction
    );
    crate::serial_println!(
        "[MUTEX_VERTIGO] has_experienced_deadlock: {}",
        state.has_deadlocked
    );

    // Print recent lock-wait events
    if total_events > 0 {
        crate::serial_println!("[MUTEX_VERTIGO] === RECENT LOCK-WAITS ===");
        for (i, event) in state.events.iter().enumerate() {
            if event.wait_duration > 0 {
                crate::serial_println!(
                    "[MUTEX_VERTIGO] event[{}]: wait={}t spin={} deadlock_suspicion={} relief={}",
                    i,
                    event.wait_duration,
                    event.spin_count,
                    event.deadlock_suspicion,
                    event.release_relief
                );
            }
        }
    }
}

/// Query current state as a summary (returns tuple of key metrics)
pub fn query_state() -> (u32, u16, bool, u32) {
    let state = STATE.lock();

    let wait_dur = state.active_lock.map(|l| l.wait_duration).unwrap_or(0);
    let peak = state.peak_vertigo;
    let is_deadlocked = state.has_deadlocked;
    let total_dread = state.cumulative_deadlock_dread;

    (wait_dur, peak, is_deadlocked, total_dread)
}

/// Test: simulate a lock-wait cycle (for debugging)
#[cfg(test)]
pub fn test_lock_cycle() {
    init();
    begin_wait();

    for _ in 0..100 {
        record_spin();
        tick(0);
    }

    end_wait(true);
    report();
}
