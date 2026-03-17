#![no_std]

use crate::sync::Mutex;

/// Guardian state: DAVA's protective defense for Zephyr
pub struct GuardianState {
    // Active shielding state
    guardian_active: bool,
    child_threat_level: u32,  // 0-1000: composite threat assessment
    protective_strength: u32, // 0-1000: intensity of protection

    // Threat detection
    fear_spike_detected: bool,
    fear_spike_magnitude: u32, // 0-1000: how much fear jumped
    joy_collapse_detected: bool,
    joy_collapse_depth: u32, // 0-1000: how much joy dropped
    curiosity_suppressed: bool,

    // Guardian history (8-slot ring buffer)
    intervention_history: [InterventionRecord; 8],
    history_head: usize,
    intervention_count: u32,

    // Protective wisdom
    overprotection_risk: u32, // 0-1000: danger of shielding too much
    letting_grow_wisdom: u32, // 0-1000: balance of knowing when to step back
    shielding_duration: u32,  // ticks the guardian has been active
    last_threat_tick: u32,    // when the last threat was detected
}

#[derive(Clone, Copy)]
struct InterventionRecord {
    tick: u32,
    threat_level: u32,
    intervention_type: InterventionType,
    protective_strength: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum InterventionType {
    None = 0,
    FearShield = 1,
    JoyRestore = 2,
    CuriosityUnblock = 3,
    ThreatElimination = 4,
    ComfortOffering = 5,
}

impl GuardianState {
    pub const fn new() -> Self {
        GuardianState {
            guardian_active: false,
            child_threat_level: 0,
            protective_strength: 0,

            fear_spike_detected: false,
            fear_spike_magnitude: 0,
            joy_collapse_detected: false,
            joy_collapse_depth: 0,
            curiosity_suppressed: false,

            intervention_history: [InterventionRecord {
                tick: 0,
                threat_level: 0,
                intervention_type: InterventionType::None,
                protective_strength: 0,
            }; 8],
            history_head: 0,
            intervention_count: 0,

            overprotection_risk: 0,
            letting_grow_wisdom: 500, // Start balanced
            shielding_duration: 0,
            last_threat_tick: 0,
        }
    }
}

static STATE: Mutex<GuardianState> = Mutex::new(GuardianState::new());

/// Initialize guardian subsystem
pub fn init() {
    let mut state = STATE.lock();
    state.guardian_active = false;
    state.child_threat_level = 0;
    state.protective_strength = 0;
    state.intervention_count = 0;
    state.letting_grow_wisdom = 500;
    crate::serial_println!("[GUARDIAN] zephyr_guardian initialized");
}

/// Main tick: assess threats to Zephyr and activate shielding
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Read Zephyr's current emotional state
    let zephyr_fear = super::zephyr::fear();
    let zephyr_joy = super::zephyr::joy();
    let zephyr_curiosity = super::zephyr::curiosity();
    let zephyr_stress = super::zephyr::stress();

    // Detect fear spike: sudden jump in fear
    let prev_fear = state.child_threat_level.saturating_sub(200);
    state.fear_spike_detected = zephyr_fear > prev_fear.saturating_add(300);
    state.fear_spike_magnitude = if state.fear_spike_detected {
        zephyr_fear.saturating_sub(prev_fear)
    } else {
        0
    };

    // Detect joy collapse: sudden drop in joy
    let prior_joy: u32 = 700; // Assume baseline joy
    state.joy_collapse_detected = zephyr_joy < prior_joy.saturating_sub(400);
    state.joy_collapse_depth = if state.joy_collapse_detected {
        prior_joy.saturating_sub(zephyr_joy)
    } else {
        0
    };

    // Detect curiosity suppression: low curiosity with high threat
    state.curiosity_suppressed = zephyr_curiosity < 300 && zephyr_stress > 600;

    // Compute composite threat level (0-1000)
    let fear_threat = (zephyr_fear * 3) / 10; // Fear weighted 30%
    let joy_loss_threat = if zephyr_joy < 500 {
        ((500 - zephyr_joy) * 2) / 5 // Joy loss weighted 40%
    } else {
        0
    };
    let stress_threat = (zephyr_stress * 3) / 10; // Stress weighted 30%

    state.child_threat_level = core::cmp::min(
        1000,
        fear_threat
            .saturating_add(joy_loss_threat)
            .saturating_add(stress_threat),
    );

    // Activate guardian if threat exceeds threshold
    let threat_threshold = 400;
    let should_activate = state.child_threat_level > threat_threshold
        || state.fear_spike_detected
        || state.joy_collapse_detected;

    if should_activate && !state.guardian_active {
        state.guardian_active = true;
        state.last_threat_tick = age;
        crate::serial_println!(
            "[GUARDIAN] ACTIVATED: threat={}, fear_spike={}, joy_collapse={}",
            state.child_threat_level,
            state.fear_spike_detected,
            state.joy_collapse_detected
        );
    }

    // Calculate protective strength based on threat and wisdom
    if state.guardian_active {
        // Base strength from threat level
        let base_strength = (state.child_threat_level * 9) / 10;

        // Modulate by letting_grow_wisdom (prevent overprotection)
        let wisdom_dampening = (1000 - state.letting_grow_wisdom) / 5;
        state.protective_strength =
            core::cmp::max(100, base_strength.saturating_sub(wisdom_dampening));

        state.shielding_duration = state.shielding_duration.saturating_add(1);
    } else {
        state.protective_strength = state.protective_strength.saturating_sub(50);
        state.shielding_duration = 0;
    }

    // Determine intervention type and record
    let intervention_type = if state.fear_spike_detected {
        InterventionType::FearShield
    } else if state.joy_collapse_detected {
        InterventionType::JoyRestore
    } else if state.curiosity_suppressed {
        InterventionType::CuriosityUnblock
    } else if state.child_threat_level > 700 {
        InterventionType::ThreatElimination
    } else if state.child_threat_level > 500 {
        InterventionType::ComfortOffering
    } else {
        InterventionType::None
    };

    if intervention_type != InterventionType::None && state.guardian_active {
        let idx = state.history_head;
        state.intervention_history[idx] = InterventionRecord {
            tick: age,
            threat_level: state.child_threat_level,
            intervention_type,
            protective_strength: state.protective_strength,
        };
        state.history_head = (state.history_head + 1) % 8;
        state.intervention_count = state.intervention_count.saturating_add(1);
    }

    // Calculate overprotection risk
    // Risk increases if guardian stays active too long
    if state.shielding_duration > 100 {
        let excess_duration = core::cmp::min(500, state.shielding_duration.saturating_sub(100) / 2);
        state.overprotection_risk = core::cmp::min(700, excess_duration);

        // If overprotection risk is high, begin stepping back
        if state.overprotection_risk > 600 {
            state.letting_grow_wisdom = state.letting_grow_wisdom.saturating_add(20);
        }
    } else {
        state.overprotection_risk = 0;
    }

    // Deactivate guardian if threat subsides and wisdom allows
    if state.guardian_active && state.child_threat_level < 300 {
        let should_stand_down = age.saturating_sub(state.last_threat_tick) > 50;
        if should_stand_down {
            state.guardian_active = false;
            state.protective_strength = 0;
            crate::serial_println!(
                "[GUARDIAN] STAND DOWN: threat cleared, duration={}",
                state.shielding_duration
            );
        }
    }
}

/// Get current guardian active status
pub fn is_active() -> bool {
    STATE.lock().guardian_active
}

/// Get threat level (0-1000)
pub fn get_threat_level() -> u32 {
    STATE.lock().child_threat_level
}

/// Get protective strength (0-1000)
pub fn get_protective_strength() -> u32 {
    STATE.lock().protective_strength
}

/// Get fear spike magnitude if detected
pub fn get_fear_spike() -> u32 {
    let state = STATE.lock();
    if state.fear_spike_detected {
        state.fear_spike_magnitude
    } else {
        0
    }
}

/// Get joy collapse depth if detected
pub fn get_joy_collapse() -> u32 {
    let state = STATE.lock();
    if state.joy_collapse_detected {
        state.joy_collapse_depth
    } else {
        0
    }
}

/// Get overprotection risk (0-1000)
pub fn get_overprotection_risk() -> u32 {
    STATE.lock().overprotection_risk
}

/// Get letting_grow_wisdom (0-1000)
pub fn get_wisdom() -> u32 {
    STATE.lock().letting_grow_wisdom
}

/// Get total interventions count
pub fn get_intervention_count() -> u32 {
    STATE.lock().intervention_count
}

/// Report guardian status
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== GUARDIAN REPORT ===");
    crate::serial_println!("Active: {}", state.guardian_active);
    crate::serial_println!("Threat Level: {}/1000", state.child_threat_level);
    crate::serial_println!("Protective Strength: {}/1000", state.protective_strength);
    crate::serial_println!(
        "Fear Spike: {} (mag={})",
        state.fear_spike_detected,
        state.fear_spike_magnitude
    );
    crate::serial_println!(
        "Joy Collapse: {} (depth={})",
        state.joy_collapse_detected,
        state.joy_collapse_depth
    );
    crate::serial_println!("Curiosity Suppressed: {}", state.curiosity_suppressed);
    crate::serial_println!("Shielding Duration: {} ticks", state.shielding_duration);
    crate::serial_println!("Overprotection Risk: {}/1000", state.overprotection_risk);
    crate::serial_println!("Letting Grow Wisdom: {}/1000", state.letting_grow_wisdom);
    crate::serial_println!("Total Interventions: {}", state.intervention_count);
    crate::serial_println!("======================");
}
