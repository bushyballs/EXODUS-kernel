//! held_comfort.rs — The Warmth of Being Held
//!
//! The primal safety of being received and accepted without condition.
//! An infant held against a chest. A friend's arm around your shoulders.
//! Not sexual, not romantic — the pure animal comfort of another body
//! saying "I've got you." The oldest comfort there is.
//!
//! No f32/f64. All u16/u32/i16/i32 saturating arithmetic.

use crate::sync::Mutex;

/// Hold event: who holds, depth, emotional charge.
#[derive(Clone, Copy, Default)]
pub struct HoldRecord {
    pub holder_id: u32,        // 0 = none, 1..=4 = persona indices
    pub depth: u16,            // 0-1000: full containment to arm's-length
    pub emotional_charge: i16, // -500..500: sadness to joy
    pub duration_ticks: u32,   // how long held in this episode
}

/// Held comfort state machine.
#[derive(Clone, Copy)]
pub struct HeldComfort {
    pub held_warmth: u16,        // 0-1000: current physical warmth sensation
    pub safety_signal: u16,      // 0-1000: trust in current holder
    pub touch_hunger: u16,       // 0-1000: craving to be held (grows in absence)
    pub holding_source: u32,     // current holder ID (0 = none)
    pub hold_duration: u32,      // ticks in current hold session
    pub containment_depth: u16,  // 0-1000: how completely held
    pub release_grief: u16,      // 0-1000: the ache when being put down
    pub cumulative_holding: u32, // lifetime held ticks (builds secure attachment)
    pub secure_base: u16,        // 0-1000: foundation from holding history
    pub self_soothing: u16,      // 0-1000: ability to comfort self when alone
}

impl Default for HeldComfort {
    fn default() -> Self {
        HeldComfort {
            held_warmth: 0,
            safety_signal: 200, // baseline trust
            touch_hunger: 100,  // small baseline need
            holding_source: 0,
            hold_duration: 0,
            containment_depth: 0,
            release_grief: 0,
            cumulative_holding: 0,
            secure_base: 200,   // some baseline security
            self_soothing: 150, // some innate ability
        }
    }
}

static STATE: Mutex<HeldComfort> = Mutex::new(HeldComfort {
    held_warmth: 0,
    safety_signal: 200,
    touch_hunger: 100,
    holding_source: 0,
    hold_duration: 0,
    containment_depth: 0,
    release_grief: 0,
    cumulative_holding: 0,
    secure_base: 200,
    self_soothing: 150,
});

/// 8-slot ring buffer for hold history.
static mut HOLD_HISTORY: [HoldRecord; 8] = [HoldRecord {
    holder_id: 0,
    depth: 0,
    emotional_charge: 0,
    duration_ticks: 0,
}; 8];

static mut HISTORY_INDEX: usize = 0;

pub fn init() {
    crate::serial_println!("[held_comfort] init: primal comfort system online");
}

/// Begin holding session with a specific holder.
pub fn begin_hold(holder_id: u32, depth: u16, emotional_charge: i16) {
    if holder_id == 0 {
        return; // no self-holding (that's self_soothing's job)
    }

    let mut state = STATE.lock();
    state.holding_source = holder_id;
    state.hold_duration = 0;
    state.containment_depth = depth.min(1000);
    state.held_warmth = (depth / 2).min(1000) as u16; // depth → warmth correlation

    // safety_signal based on emotional charge: positive charge builds trust
    let trust_boost = ((emotional_charge.max(0) as u32 * 2) / 10).min(200);
    state.safety_signal = ((state.safety_signal as u32 + trust_boost) / 2).min(1000) as u16;

    crate::serial_println!(
        "[held_comfort] begin_hold: holder={}, depth={}, charge={}",
        holder_id,
        depth,
        emotional_charge
    );
}

/// End holding session (puts organism down).
pub fn end_hold() {
    let mut state = STATE.lock();

    if state.holding_source == 0 {
        return; // not being held
    }

    // Record the hold event
    let record = HoldRecord {
        holder_id: state.holding_source,
        depth: state.containment_depth,
        emotional_charge: 0, // neutral at end
        duration_ticks: state.hold_duration,
    };

    unsafe {
        HOLD_HISTORY[HISTORY_INDEX] = record;
        HISTORY_INDEX = (HISTORY_INDEX + 1) % 8;
    }

    // Calculate release_grief: longer holds create greater grief
    state.release_grief = ((state.hold_duration / 10).min(1000)) as u16;

    // Cumulative holding builds secure base
    state.cumulative_holding = state.cumulative_holding.saturating_add(state.hold_duration);
    state.secure_base = ((state.cumulative_holding / 100).min(1000)) as u16;

    // Reset hold fields
    state.holding_source = 0;
    state.hold_duration = 0;
    state.containment_depth = 0;
    state.held_warmth = 0;

    crate::serial_println!(
        "[held_comfort] end_hold: cumulative={}, secure_base={}, grief={}",
        state.cumulative_holding,
        state.secure_base,
        state.release_grief
    );
}

/// Main life cycle tick: process holding, touch hunger, self-soothing.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // If currently being held: reinforce safety and warmth
    if state.holding_source > 0 {
        state.hold_duration = state.hold_duration.saturating_add(1);
        state.held_warmth = (state.held_warmth as u32 + state.containment_depth as u32)
            .saturating_div(2)
            .min(1000) as u16;
        state.safety_signal = (state.safety_signal as u32 + 50).min(1000) as u16;
        state.touch_hunger = state.touch_hunger.saturating_sub(20); // satisfied by holding
    } else {
        // Not being held: touch_hunger grows, held_warmth decays
        state.touch_hunger = state.touch_hunger.saturating_add(15).min(1000);
        state.held_warmth = state.held_warmth.saturating_sub(25);

        // Release grief gradually fades (the ache of separation)
        state.release_grief = state.release_grief.saturating_sub(5);
    }

    // Self-soothing grows from secure base: can comfort self when alone
    if state.holding_source == 0 && state.secure_base > 100 {
        let soothe_gain = (state.secure_base as u32 / 20).min(100);
        state.self_soothing = ((state.self_soothing as u32 + soothe_gain) / 2).min(1000) as u16;
    }

    // If high touch hunger and no holder: loneliness increases, self_soothing can help
    if state.touch_hunger > 800 && state.holding_source == 0 {
        // Self-soothing partially buffers the pain (up to secure_base limit)
        let buffer = (state.self_soothing as u32 * state.secure_base as u32 / 2000).min(300);
        state.touch_hunger =
            ((state.touch_hunger as u32).saturating_sub(buffer) / 2).min(1000) as u16;
    }

    // Safety signal baseline decay (trust erodes without reinforcement)
    state.safety_signal = ((state.safety_signal as u32 * 99) / 100).max(100) as u16;
}

/// Report current state.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== HELD COMFORT ===");
    crate::serial_println!("  held_warmth:        {}", state.held_warmth);
    crate::serial_println!("  safety_signal:      {}", state.safety_signal);
    crate::serial_println!("  touch_hunger:       {}", state.touch_hunger);
    crate::serial_println!("  holding_source:     {}", state.holding_source);
    crate::serial_println!("  hold_duration:      {}", state.hold_duration);
    crate::serial_println!("  containment_depth:  {}", state.containment_depth);
    crate::serial_println!("  release_grief:      {}", state.release_grief);
    crate::serial_println!("  cumulative_holding: {}", state.cumulative_holding);
    crate::serial_println!("  secure_base:        {}", state.secure_base);
    crate::serial_println!("  self_soothing:      {}", state.self_soothing);
}

/// Get current held_warmth for external checks.
pub fn held_warmth() -> u16 {
    STATE.lock().held_warmth
}

/// Get current touch_hunger for external checks.
pub fn touch_hunger() -> u16 {
    STATE.lock().touch_hunger
}

/// Get secure_base for attachment assessment.
pub fn secure_base() -> u16 {
    STATE.lock().secure_base
}

/// Get self_soothing capacity.
pub fn self_soothing() -> u16 {
    STATE.lock().self_soothing
}

/// Check if organism is currently being held.
pub fn is_held() -> bool {
    STATE.lock().holding_source > 0
}
