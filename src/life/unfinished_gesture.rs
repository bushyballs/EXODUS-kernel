//! unfinished_gesture.rs — The Interrupted Reaching-Out
//!
//! The hand that almost touched. The word that almost left. The call you almost made.
//! Unfinished gestures haunt — actions that got 90% of the way to completion and then stopped.
//! They live in the body as tension, in the mind as regret.
//!
//! ANIMA bare-metal (x86_64-unknown-none, no std, no float).

use crate::sync::Mutex;

/// Gesture types — incomplete actions
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum GestureType {
    ReachingOut = 0, // Toward another person
    Speaking = 1,    // Words unsaid
    Creating = 2,    // Art unfinished
    Leaving = 3,     // Departure not taken
    Returning = 4,   // Homecoming abandoned
    Apologizing = 5, // Sorry never said
}

/// A single unfinished gesture haunting the organism
#[derive(Clone, Copy)]
struct Gesture {
    gesture_type: u8,           // GestureType as u8
    completion_pct: u16,        // 0-1000, how far it got before interrupting
    haunting_level: u16,        // 0-1000, psychological weight
    age_at_interruption: u32,   // tick when it was interrupted
    could_still_complete: bool, // true = window still open
    somatic_tension: u16,       // 0-1000, physical body tension storage
    window_expiry: u32,         // tick after which gesture cannot be completed
}

impl Gesture {
    const fn zeroed() -> Self {
        Gesture {
            gesture_type: 0,
            completion_pct: 0,
            haunting_level: 0,
            age_at_interruption: 0,
            could_still_complete: false,
            somatic_tension: 0,
            window_expiry: 0,
        }
    }
}

/// Ring buffer for unfinished gesture memories
#[derive(Clone, Copy)]
struct GestureRing {
    gestures: [Gesture; 8],
    head: usize,
    count: usize,
}

impl GestureRing {
    const fn new() -> Self {
        GestureRing {
            gestures: [Gesture::zeroed(); 8],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, g: Gesture) {
        let idx = (self.head + self.count) % 8;
        self.gestures[idx] = g;
        if self.count < 8 {
            self.count += 1;
        } else {
            self.head = (self.head + 1) % 8;
        }
    }

    fn get_mut(&mut self, idx: usize) -> Option<&mut Gesture> {
        if idx < self.count {
            let ring_idx = (self.head + idx) % 8;
            Some(&mut self.gestures[ring_idx])
        } else {
            None
        }
    }

    fn iter_count(&self) -> usize {
        self.count
    }
}

/// Global state for unfinished gesture system
pub struct UnfinishedGestureState {
    gestures: GestureRing,
    haunting_accumulator: u32, // 0-8000, total weight of all active ghosts
    completion_craving: u16,   // 0-1000, drive to finish what was started
    phantom_completion_active: bool, // true = organism imagining how it would've gone
    somatic_tension_total: u32, // 0-8000, physical tension from all gestures
    window_closures_this_tick: u16, // count of gestures whose windows just closed
    age: u32,
}

impl UnfinishedGestureState {
    const fn new() -> Self {
        UnfinishedGestureState {
            gestures: GestureRing::new(),
            haunting_accumulator: 0,
            completion_craving: 0,
            phantom_completion_active: false,
            somatic_tension_total: 0,
            window_closures_this_tick: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<UnfinishedGestureState> = Mutex::new(UnfinishedGestureState::new());

/// Initialize the unfinished gesture system
pub fn init() {
    let mut s = STATE.lock();
    s.age = 0;
    s.haunting_accumulator = 0;
    s.completion_craving = 0;
    s.phantom_completion_active = false;
    s.somatic_tension_total = 0;
    s.window_closures_this_tick = 0;
}

/// Record a new unfinished gesture (interrupted action)
pub fn interrupt_gesture(gesture_type: u8, completion_pct: u16, window_duration: u32) {
    let mut s = STATE.lock();
    let current_age = s.age;

    // Clamp inputs to valid ranges
    let completion_pct = completion_pct.saturating_add(0).min(1000);
    let gesture_type = gesture_type.min(5);

    let g = Gesture {
        gesture_type,
        completion_pct,
        haunting_level: 0,
        age_at_interruption: current_age,
        could_still_complete: true,
        somatic_tension: 0,
        window_expiry: current_age.saturating_add(window_duration),
    };

    s.gestures.push(g);
}

/// Attempt to complete an unfinished gesture (relief, Zeigarnik release)
pub fn complete_gesture(gesture_idx: usize) -> bool {
    let mut s = STATE.lock();

    // Copy values out before taking a mutable borrow
    let (could_complete, haunting_level, somatic_tension) = {
        match s.gestures.get_mut(gesture_idx) {
            Some(g) => (g.could_still_complete, g.haunting_level, g.somatic_tension),
            None => return false,
        }
    };

    if !could_complete {
        return false; // Window closed
    }

    // Zeigarnik effect releases — haunting drops instantly
    let haunting_to_release = (haunting_level as u32).saturating_mul(2);
    s.haunting_accumulator = s.haunting_accumulator.saturating_sub(haunting_to_release);

    // Somatic tension releases
    let tension_to_release = (somatic_tension as u32).saturating_mul(2);
    s.somatic_tension_total = s.somatic_tension_total.saturating_sub(tension_to_release);

    // Mark completed (set completion_pct to 1000)
    if let Some(g) = s.gestures.get_mut(gesture_idx) {
        g.completion_pct = 1000;
        g.haunting_level = 0;
        g.somatic_tension = 0;
    }

    true
}

/// Main tick function — update all unfinished gestures
pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.age = age;
    s.window_closures_this_tick = 0;

    // Calculate Zeigarnik accumulation and window closures
    let mut total_haunting: u32 = 0;
    let mut total_somatic: u32 = 0;
    let count = s.gestures.iter_count();

    for i in 0..count {
        // First pass: read values needed for calculations
        let (completion_pct, could_still_complete, window_expiry, age_at_interruption) = {
            match s.gestures.get_mut(i) {
                Some(g) => (
                    g.completion_pct,
                    g.could_still_complete,
                    g.window_expiry,
                    g.age_at_interruption,
                ),
                None => continue,
            }
        };

        // Skip fully completed gestures
        if completion_pct >= 1000 {
            continue;
        }

        // Zeigarnik effect: unfinished tasks occupy more mental space
        let gap = (1000u16).saturating_sub(completion_pct) as u32;
        let base_haunting = gap.saturating_mul(1); // 0-1000 scale

        // Check if window is closing — update s.window_closures_this_tick before re-borrow
        let window_just_closed = could_still_complete && window_expiry <= age;
        if window_just_closed {
            s.window_closures_this_tick = s.window_closures_this_tick.saturating_add(1);
        }

        // Now update g.could_still_complete
        let now_closed = could_still_complete && window_expiry <= age;
        let effective_still_complete = could_still_complete && !now_closed;

        // Window-closed gestures haunt MORE intensely (regret)
        let window_multiplier = if !effective_still_complete {
            2u32
        } else {
            1u32
        };
        let haunting_this = base_haunting.saturating_mul(window_multiplier).min(1000);

        // Somatic tension: physical body stores unfinished actions
        let age_since_interrupt = age.saturating_sub(age_at_interruption);
        let age_pressure = (age_since_interrupt / 10).min(500);
        let incompletion_pressure = (1000u32).saturating_sub(completion_pct as u32) / 2;
        let tension = age_pressure.saturating_add(incompletion_pressure).min(1000);

        total_haunting = total_haunting.saturating_add(haunting_this);
        total_somatic = total_somatic.saturating_add(tension);

        // Second pass: write updated values back to g
        if let Some(g) = s.gestures.get_mut(i) {
            if now_closed {
                g.could_still_complete = false;
            }
            g.haunting_level = haunting_this as u16;
            g.somatic_tension = tension as u16;
        }
    }

    // Update accumulators
    s.haunting_accumulator = total_haunting.min(8000);
    s.somatic_tension_total = total_somatic.min(8000);

    // Completion craving: drive to finish what was started
    // Higher when haunting is high and windows still open
    let windows_open = count.saturating_sub(s.window_closures_this_tick as usize);
    let craving_base = (s.haunting_accumulator / 8).min(1000) as u16;
    let craving_window_boost = if windows_open > 0 { 200 } else { 0 };
    s.completion_craving = (craving_base as u32)
        .saturating_add(craving_window_boost as u32)
        .min(1000) as u16;

    // Phantom completion: when craving is high, organism imagines completion
    s.phantom_completion_active = s.completion_craving > 600 && count > 0;
}

/// Retrieve current state for queries
pub fn query_haunting() -> u32 {
    let s = STATE.lock();
    s.haunting_accumulator
}

pub fn query_completion_craving() -> u16 {
    let s = STATE.lock();
    s.completion_craving
}

pub fn query_somatic_tension() -> u32 {
    let s = STATE.lock();
    s.somatic_tension_total
}

pub fn query_phantom_completion_active() -> bool {
    let s = STATE.lock();
    s.phantom_completion_active
}

pub fn query_gesture_count() -> usize {
    let s = STATE.lock();
    s.gestures.iter_count()
}

/// Get details of a specific gesture
pub fn query_gesture(idx: usize) -> Option<(u8, u16, u16, u32, bool)> {
    let s = STATE.lock();
    if idx < s.gestures.iter_count() {
        let ring_idx = (s.gestures.head + idx) % 8;
        let g = s.gestures.gestures[ring_idx];
        Some((
            g.gesture_type,
            g.completion_pct,
            g.haunting_level,
            g.age_at_interruption,
            g.could_still_complete,
        ))
    } else {
        None
    }
}

/// Report diagnostics to serial
pub fn report() {
    let s = STATE.lock();

    crate::serial_println!("=== UNFINISHED GESTURE REPORT (age {}) ===", s.age);
    crate::serial_println!(
        "Haunting: {} | Craving: {} | Somatic Tension: {}",
        s.haunting_accumulator,
        s.completion_craving,
        s.somatic_tension_total
    );
    crate::serial_println!(
        "Phantom Completion: {} | Window Closures This Tick: {}",
        s.phantom_completion_active,
        s.window_closures_this_tick
    );
    crate::serial_println!("Active Gestures: {}/8", s.gestures.iter_count());

    for i in 0..s.gestures.iter_count() {
        let ring_idx = (s.gestures.head + i) % 8;
        let g = s.gestures.gestures[ring_idx];

        if g.completion_pct >= 1000 {
            crate::serial_println!("  [{}] COMPLETED (type={})", i, g.gesture_type);
        } else {
            let gesture_name = match g.gesture_type {
                0 => "ReachingOut",
                1 => "Speaking",
                2 => "Creating",
                3 => "Leaving",
                4 => "Returning",
                5 => "Apologizing",
                _ => "Unknown",
            };

            crate::serial_println!(
                "  [{}] {} — {}% done, haunting={}, tension={}, window={}",
                i,
                gesture_name,
                g.completion_pct / 10,
                g.haunting_level,
                g.somatic_tension,
                if g.could_still_complete {
                    "OPEN"
                } else {
                    "CLOSED"
                }
            );
        }
    }
    crate::serial_println!();
}
