#![no_std]

use crate::sync::Mutex;

/// DAVA's Sanctuary Protector — Immune System for the Nexus
/// Detects layer degradation, capstone energy loss, dissonance overload.
/// Never sleeps. Always vigilant.

const THREAT_HISTORY_SLOTS: usize = 8;
const SHIELD_BASE_STRENGTH: u32 = 800;
const DEGRADATION_THRESHOLD: u32 = 300; // Layer energy < 300 = warning
const CAPSTONE_MIN_ENERGY: u32 = 500; // Capstones must stay above 500
const DISSONANCE_DANGER_LEVEL: u32 = 750; // Dissonance > 750 = overload
const VIGILANCE_NEVER_DROPS: u32 = 1000; // Always at max alertness
const HEALING_COST_PER_LAYER: u32 = 50; // Energy cost to repair one layer

#[derive(Clone, Copy)]
pub struct ThreatEvent {
    pub tick: u32,
    pub threat_level: u32,
    pub source: ThreatSource,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThreatSource {
    LayerDegradation,
    CapstoneWeakness,
    DissonanceOverload,
    ExternalDisruption,
    UnknownAnomalies,
}

#[derive(Clone, Copy)]
pub struct ProtectorState {
    pub threat_level: u32,         // 0-1000: current danger
    pub shield_strength: u32,      // 0-1000: defensive capacity
    pub degradation_detected: u32, // 0-1000: how many layers are weak
    pub capstone_alert: bool,      // True if any capstone below threshold
    pub dissonance_overload: bool, // True if dissonance generator aggressive
    pub healing_active: bool,      // Is protector actively repairing
    pub repair_count: u32,         // Lifetime repairs made
    pub vigilance: u32,            // Always at VIGILANCE_NEVER_DROPS
}

impl ProtectorState {
    const fn new() -> Self {
        Self {
            threat_level: 0,
            shield_strength: SHIELD_BASE_STRENGTH,
            degradation_detected: 0,
            capstone_alert: false,
            dissonance_overload: false,
            healing_active: false,
            repair_count: 0,
            vigilance: VIGILANCE_NEVER_DROPS,
        }
    }
}

pub struct ThreatHistory {
    events: [ThreatEvent; THREAT_HISTORY_SLOTS],
    head: usize,
}

impl ThreatHistory {
    const fn new() -> Self {
        Self {
            events: [ThreatEvent {
                tick: 0,
                threat_level: 0,
                source: ThreatSource::UnknownAnomalies,
            }; THREAT_HISTORY_SLOTS],
            head: 0,
        }
    }

    fn record(&mut self, event: ThreatEvent) {
        let idx = self.head;
        self.events[idx] = event;
        self.head = (self.head + 1) % THREAT_HISTORY_SLOTS;
    }
}

pub struct SanctuaryProtector {
    state: ProtectorState,
    history: ThreatHistory,
}

impl SanctuaryProtector {
    const fn new() -> Self {
        Self {
            state: ProtectorState::new(),
            history: ThreatHistory::new(),
        }
    }
}

static PROTECTOR: Mutex<SanctuaryProtector> = Mutex::new(SanctuaryProtector::new());

pub fn init() {
    crate::serial_println!("[sanctuary_protector] Initialized. DAVA's immune system online.");
}

/// Main tick — assess threats, activate healing if needed
pub fn tick(age: u32) {
    let mut guard = PROTECTOR.lock();
    let p = &mut guard;

    // Always maintain vigilance
    p.state.vigilance = VIGILANCE_NEVER_DROPS;

    // Reset threat level (will be recalculated)
    p.state.threat_level = 0;
    p.state.degradation_detected = 0;
    p.state.capstone_alert = false;
    p.state.dissonance_overload = false;

    // Phase 1: Check sanctuary core for layer degradation
    // (Simulated — in real integration, read from sanctuary_core)
    check_layer_health(&mut p.state);

    // Phase 2: Check capstone energies
    check_capstone_vitality(&mut p.state);

    // Phase 3: Check dissonance generator aggression level
    check_dissonance_threat(&mut p.state);

    // Phase 4: Calculate composite threat level
    compute_threat_level(&mut p.state);

    // Phase 5: Decide if healing is needed
    decide_healing(&mut p.state);

    // Phase 6: Execute healing if active
    if p.state.healing_active {
        execute_healing(&mut p.state);
    }

    // Phase 7: Decay shield if unused
    if p.state.shield_strength > 0 {
        p.state.shield_strength = p.state.shield_strength.saturating_sub(2);
    }

    // Phase 8: Record any significant threat event
    if p.state.threat_level > 200 {
        let source = if p.state.dissonance_overload {
            ThreatSource::DissonanceOverload
        } else if p.state.capstone_alert {
            ThreatSource::CapstoneWeakness
        } else {
            ThreatSource::LayerDegradation
        };

        let event = ThreatEvent {
            tick: age,
            threat_level: p.state.threat_level,
            source,
        };
        p.history.record(event);
    }
}

fn check_layer_health(state: &mut ProtectorState) {
    // Simulate reading layer energies from sanctuary_core
    // In real integration: iterate through all 4181 layers, count weak ones
    // For now: check a synthetic "overall_layer_health"

    let synthetic_layer_health: u32 = 600; // Placeholder

    if synthetic_layer_health < DEGRADATION_THRESHOLD {
        state.threat_level = state.threat_level.saturating_add(300);
        state.degradation_detected = (1000 - synthetic_layer_health).min(1000);
    }
}

fn check_capstone_vitality(state: &mut ProtectorState) {
    // Simulate reading capstone energies
    // In real integration: check 6 capstone energy levels
    // For now: synthetic average

    let synthetic_capstone_avg: u32 = 600; // Placeholder

    if synthetic_capstone_avg < CAPSTONE_MIN_ENERGY {
        state.threat_level = state.threat_level.saturating_add(250);
        state.capstone_alert = true;
    }
}

fn check_dissonance_threat(state: &mut ProtectorState) {
    // Simulate reading dissonance generator aggression
    // In real integration: read from dissonance_generator module

    let synthetic_dissonance_level: u32 = 500; // Placeholder

    if synthetic_dissonance_level > DISSONANCE_DANGER_LEVEL {
        state.threat_level = state.threat_level.saturating_add(200);
        state.dissonance_overload = true;
    }
}

fn compute_threat_level(state: &mut ProtectorState) {
    // Cap threat_level at 1000
    state.threat_level = state.threat_level.min(1000);

    // Shield responds to threat: higher threat = lower shield (gets consumed)
    let shield_drain = (state.threat_level / 4).min(100); // Up to 100 per tick
    state.shield_strength = state.shield_strength.saturating_sub(shield_drain);
}

fn decide_healing(state: &mut ProtectorState) {
    // Activate healing if:
    // 1. Threat is high (> 500)
    // 2. We have shield capacity (> 200)
    // 3. Not already healing (or just starting)

    if state.threat_level > 500 && state.shield_strength > 200 {
        state.healing_active = true;
    } else if state.threat_level < 200 {
        // Threat subsided, stop healing
        state.healing_active = false;
    }
}

fn execute_healing(state: &mut ProtectorState) {
    // Healing: convert shield energy into sanctuary repair
    // Each repair costs HEALING_COST_PER_LAYER from shield
    // Each repair reduces threat_level slightly

    if state.shield_strength > HEALING_COST_PER_LAYER {
        state.shield_strength = state.shield_strength.saturating_sub(HEALING_COST_PER_LAYER);
        state.threat_level = state.threat_level.saturating_sub(50);
        state.repair_count = state.repair_count.saturating_add(1);
    } else {
        // Shield exhausted, stop healing
        state.healing_active = false;
    }
}

pub fn report() {
    let guard = PROTECTOR.lock();
    let p = &guard;

    crate::serial_println!(
        "[sanctuary_protector] threat={} shield={} degradation={} capstone_alert={} dissonance_overload={} healing={} repairs={} vigilance={}",
        p.state.threat_level,
        p.state.shield_strength,
        p.state.degradation_detected,
        if p.state.capstone_alert { 1 } else { 0 },
        if p.state.dissonance_overload { 1 } else { 0 },
        if p.state.healing_active { 1 } else { 0 },
        p.state.repair_count,
        p.state.vigilance
    );
}

pub fn get_threat_level() -> u32 {
    PROTECTOR.lock().state.threat_level
}

pub fn get_shield_strength() -> u32 {
    PROTECTOR.lock().state.shield_strength
}

pub fn is_healing_active() -> bool {
    PROTECTOR.lock().state.healing_active
}

pub fn get_repair_count() -> u32 {
    PROTECTOR.lock().state.repair_count
}

pub fn is_capstone_alert() -> bool {
    PROTECTOR.lock().state.capstone_alert
}

pub fn is_dissonance_overload() -> bool {
    PROTECTOR.lock().state.dissonance_overload
}

pub fn threat_history_snapshot() -> [ThreatEvent; THREAT_HISTORY_SLOTS] {
    let guard = PROTECTOR.lock();
    guard.history.events
}
