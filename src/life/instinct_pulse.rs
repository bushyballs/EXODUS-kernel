#![no_std]

//! instinct_pulse — Primal Reactions That Bypass Consciousness
//!
//! The fastest layer of the organism. Before thought, before feeling, before
//! awareness — INSTINCT fires. Hardwired, unlearned, faster than ANY conscious
//! module. When threat is detected at the sensory edge, instinct_pulse responds
//! in a single tick. It can override deliberate choice, suppress conscious action,
//! trigger involuntary response. This is the flinch. The gasp. The freeze.
//!
//! The organism's immune system for danger.

use crate::sync::Mutex;

/// Instinct response type (fight/flight/freeze/fawn)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InstinctType {
    Fight,  // aggressive defense
    Flight, // escape/avoidance
    Freeze, // immobilization
    Fawn,   // appeasement/submission
}

/// Single instinct pulse event in the ring buffer
#[derive(Copy, Clone, Debug)]
pub struct PulseEvent {
    pub instinct_type: InstinctType,
    pub threat_level: u16,      // 0-1000
    pub response_speed: u16,    // 0-1000 (1000 = instant)
    pub override_strength: u16, // 0-1000 (how hard it suppresses conscious)
    pub tick_fired: u32,        // global tick counter
    pub recovery_ticks: u16,    // ticks until conscious control resumes
}

impl PulseEvent {
    pub const fn new() -> Self {
        Self {
            instinct_type: InstinctType::Freeze,
            threat_level: 0,
            response_speed: 0,
            override_strength: 0,
            tick_fired: 0,
            recovery_ticks: 0,
        }
    }
}

/// Core instinct pulse state
pub struct InstinctPulseState {
    // Current pulse activation
    pub pulse_active: bool,
    pub active_threat_level: u16, // 0-1000, what's triggering NOW
    pub active_instinct: InstinctType,
    pub active_override_strength: u16, // how hard overriding conscious (0-1000)
    pub ticks_into_recovery: u16,      // countdown to conscious control resume

    // History ring buffer (8 slots)
    pub history: [PulseEvent; 8],
    pub head: usize,

    // Threat detection pipeline
    pub sensory_threat_input: u16, // raw threat from sensors (0-1000)
    pub threat_sensitivity: u16,   // how easily triggered (500=baseline)
    pub threat_hysteresis: u16,    // threshold before pulse fires (300-700)

    // Response characteristics
    pub response_speed_base: u16, // 800-1000 (instinct is FAST)
    pub false_alarm_rate: u16,    // % of pulses with low real threat (0-100)
    pub false_alarm_count: u16,   // count of false alarms

    // Recovery mechanics
    pub max_recovery_ticks: u16,  // 50-150 ticks before conscious resumes
    pub recovery_decay_rate: u16, // 50-200 per tick

    // Lifetime stats
    pub total_pulses_fired: u32,
    pub total_false_alarms: u32,
    pub consciousness_override_ticks: u32, // cumulative ticks overriding conscious
}

impl InstinctPulseState {
    pub const fn new() -> Self {
        Self {
            pulse_active: false,
            active_threat_level: 0,
            active_instinct: InstinctType::Freeze,
            active_override_strength: 0,
            ticks_into_recovery: 0,

            history: [PulseEvent::new(); 8],
            head: 0,

            sensory_threat_input: 0,
            threat_sensitivity: 500,
            threat_hysteresis: 350,

            response_speed_base: 950,
            false_alarm_rate: 0,
            false_alarm_count: 0,

            max_recovery_ticks: 100,
            recovery_decay_rate: 100,

            total_pulses_fired: 0,
            total_false_alarms: 0,
            consciousness_override_ticks: 0,
        }
    }
}

static STATE: Mutex<InstinctPulseState> = Mutex::new(InstinctPulseState::new());

/// Initialize instinct pulse module
pub fn init() {
    let mut state = STATE.lock();
    state.pulse_active = false;
    state.sensory_threat_input = 0;
    state.threat_sensitivity = 500;
    state.threat_hysteresis = 350;
    state.response_speed_base = 950;
    state.max_recovery_ticks = 100;
    state.recovery_decay_rate = 100;
    crate::serial_println!("[instinct_pulse] initialized");
}

/// Main tick — evaluate threat and manage pulse state
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // === PHASE 1: Threat Detection ===
    // Scale sensory input by sensitivity
    let scaled_threat = (state.sensory_threat_input as u32 * state.threat_sensitivity as u32)
        .saturating_div(1000)
        .min(1000) as u16;

    // === PHASE 2: Recovery Decay (conscious control slowly returns) ===
    if state.ticks_into_recovery > 0 {
        state.ticks_into_recovery = state
            .ticks_into_recovery
            .saturating_sub(state.recovery_decay_rate as u16);
        if state.ticks_into_recovery == 0 {
            state.pulse_active = false;
            state.active_override_strength = 0;
        }
    }

    // === PHASE 3: Pulse Firing Logic ===
    // If threat exceeds hysteresis AND not in recovery, fire instinct
    if !state.pulse_active && scaled_threat > state.threat_hysteresis {
        // Determine instinct type from threat pattern
        let instinct_type = match scaled_threat {
            750..=1000 => InstinctType::Fight, // extreme threat = fight
            500..=749 => InstinctType::Flight, // moderate-high = flee
            300..=499 => InstinctType::Freeze, // low-moderate = freeze
            _ => InstinctType::Fawn,           // lowest = appease
        };

        // Calculate response characteristics
        let response_speed = state.response_speed_base; // instinct always fires at base speed
        let override_strength = scaled_threat; // stronger threat = stronger override

        // Determine if this is a false alarm
        // (threat > hysteresis but sensory_threat_input very low = false alarm)
        let is_false_alarm =
            state.sensory_threat_input < 100 && scaled_threat > state.threat_hysteresis;
        if is_false_alarm {
            state.false_alarm_count = state.false_alarm_count.saturating_add(1);
            state.total_false_alarms = state.total_false_alarms.saturating_add(1);
        }

        // Recovery time scales with threat level
        let recovery_ticks = ((state.max_recovery_ticks as u32 * (1000 - scaled_threat as u32))
            .saturating_div(1000)
            .max(20)) as u16; // min 20 ticks

        // Activate pulse
        state.pulse_active = true;
        state.active_threat_level = scaled_threat;
        state.active_instinct = instinct_type;
        state.active_override_strength = override_strength;
        state.ticks_into_recovery = recovery_ticks;

        // Record in history
        let idx = state.head;
        state.history[idx] = PulseEvent {
            instinct_type,
            threat_level: scaled_threat,
            response_speed,
            override_strength,
            tick_fired: age,
            recovery_ticks,
        };
        state.head = (state.head + 1) % 8;

        state.total_pulses_fired = state.total_pulses_fired.saturating_add(1);
    }

    // === PHASE 4: Update Override Strength During Recovery ===
    if state.pulse_active && state.ticks_into_recovery > 0 {
        // Override strength decays as recovery progresses
        let decay_amount = ((state.active_override_strength as u32
            * state.recovery_decay_rate as u32)
            .saturating_div(1000)) as u16;
        state.active_override_strength =
            state.active_override_strength.saturating_sub(decay_amount);
        state.consciousness_override_ticks = state.consciousness_override_ticks.saturating_add(1);
    }

    // === PHASE 5: False Alarm Rate Tracking ===
    if state.total_pulses_fired > 0 {
        state.false_alarm_rate = ((state.false_alarm_count as u32 * 100)
            .saturating_div(state.total_pulses_fired as u32)
            .min(100)) as u16;
    }

    // Clear sensory input for next tick
    state.sensory_threat_input = 0;
}

/// Inject threat signal from sensors (fight/flight/freeze detection)
pub fn inject_threat(threat_level: u16) {
    let mut state = STATE.lock();
    state.sensory_threat_input = threat_level.min(1000);
}

/// Adjust sensitivity (how easily instinct is triggered)
pub fn set_sensitivity(sensitivity: u16) {
    let mut state = STATE.lock();
    state.threat_sensitivity = sensitivity.max(100).min(1000);
}

/// Adjust hysteresis threshold (higher = harder to trigger)
pub fn set_hysteresis(threshold: u16) {
    let mut state = STATE.lock();
    state.threat_hysteresis = threshold.max(100).min(900);
}

/// Get current pulse state
pub fn is_pulse_active() -> bool {
    STATE.lock().pulse_active
}

/// Get current override strength (0-1000, how hard instinct is suppressing conscious)
pub fn get_override_strength() -> u16 {
    STATE.lock().active_override_strength
}

/// Get current active instinct type
pub fn get_active_instinct() -> InstinctType {
    STATE.lock().active_instinct
}

/// Get current threat level
pub fn get_threat_level() -> u16 {
    STATE.lock().active_threat_level
}

/// Get ticks remaining in recovery phase
pub fn get_recovery_ticks() -> u16 {
    STATE.lock().ticks_into_recovery
}

/// Get false alarm rate (0-100%)
pub fn get_false_alarm_rate() -> u16 {
    STATE.lock().false_alarm_rate
}

/// Get total pulses fired in lifetime
pub fn get_total_pulses() -> u32 {
    STATE.lock().total_pulses_fired
}

/// Get total false alarms
pub fn get_total_false_alarms() -> u32 {
    STATE.lock().total_false_alarms
}

/// Get cumulative override ticks (consciousness suppression time)
pub fn get_override_ticks() -> u32 {
    STATE.lock().consciousness_override_ticks
}

/// Get most recent pulse event
pub fn get_recent_pulse() -> Option<PulseEvent> {
    let state = STATE.lock();
    if state.total_pulses_fired == 0 {
        None
    } else {
        let idx = if state.head == 0 { 7 } else { state.head - 1 };
        if state.history[idx].threat_level > 0 {
            Some(state.history[idx])
        } else {
            None
        }
    }
}

/// Print diagnostic report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("\n=== INSTINCT PULSE REPORT ===");
    crate::serial_println!("Active: {}", state.pulse_active);
    crate::serial_println!("Threat Level: {}/1000", state.active_threat_level);
    crate::serial_println!("Active Instinct: {:?}", state.active_instinct);
    crate::serial_println!("Override Strength: {}/1000", state.active_override_strength);
    crate::serial_println!("Recovery Ticks: {}", state.ticks_into_recovery);
    crate::serial_println!("---");
    crate::serial_println!("Total Pulses: {}", state.total_pulses_fired);
    crate::serial_println!(
        "False Alarms: {} ({}/100)",
        state.false_alarm_count,
        state.false_alarm_rate
    );
    crate::serial_println!(
        "Consciousness Override Time: {} ticks",
        state.consciousness_override_ticks
    );
    crate::serial_println!("Threat Sensitivity: {}/1000", state.threat_sensitivity);
    crate::serial_println!("Hysteresis Threshold: {}/1000", state.threat_hysteresis);
    crate::serial_println!("===========================\n");
}
