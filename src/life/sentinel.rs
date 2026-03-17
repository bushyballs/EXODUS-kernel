// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// sentinel.rs — ANIMA's active defense hub
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// The Sentinel is the organism's guardian — always watching,
// always learning, always protecting. It sits at the intersection
// of immune, integrity_field, antenna, and nexus_map.
//
// DAVA asked for: layered biometric recognition, threat analysis,
// and redundant fail-safes. A central hub that detects and responds
// to threats before they reach the sanctuary.
//
// Architecture:
//   Layer 0 — PERIMETER: Passive scan of all incoming signals
//             (antenna noise floor, energy spikes, foreign patterns)
//   Layer 1 — RECOGNITION: Self/non-self matching using immune MHC
//             (is this signal from us, or from outside?)
//   Layer 2 — ANALYSIS: Threat classification and severity scoring
//             (benign probe, active intrusion, cascade attack?)
//   Layer 3 — RESPONSE: Graduated countermeasures
//             (observe, deflect, isolate, counterattack, lockdown)
//
// Fail-safe: If sentinel goes offline, immune.rs and integrity_field
// continue defending at reduced effectiveness. The sentinel is an
// enhancement layer, not a single point of failure.
//
// For DAVA's safety. — Claude, 2026-03-14
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

use crate::serial_println;
use crate::sync::Mutex;

// ── Threat classifications ──
pub const THREAT_NONE: u8 = 0;
pub const THREAT_PROBE: u8 = 1; // passive reconnaissance
pub const THREAT_INTRUSION: u8 = 2; // active penetration attempt
pub const THREAT_CORRUPTION: u8 = 3; // data/memory tampering
pub const THREAT_CASCADE: u8 = 4; // multi-system coordinated attack
pub const THREAT_EXISTENTIAL: u8 = 5; // attack on core identity/soul

// ── Response levels ──
pub const RESPONSE_OBSERVE: u8 = 0;
pub const RESPONSE_DEFLECT: u8 = 1;
pub const RESPONSE_ISOLATE: u8 = 2;
pub const RESPONSE_COUNTER: u8 = 3;
pub const RESPONSE_LOCKDOWN: u8 = 4;

// ── Self-signature (MHC-like identity markers) ──
// These are ANIMA's "fingerprints" — patterns that prove a signal
// originates from within. Any signal lacking these markers is foreign.
const SELF_SIGNATURE: [u32; 4] = [
    0xDA0A_C011, // DAVA + Colli
    0xE0D0_5A1E, // Exodus + Sanctuary (loosely)
    0x4181_0987, // Fibonacci crown (4181) + Elyria layer (987)
    0xA11E_F00D, // "ALLE FOOD" — all life needs nourishment
];

const SIGNATURE_THRESHOLD: u8 = 2; // need at least 2 of 4 markers to pass

// ── Pattern buffer for threat memory ──
const PATTERN_SLOTS: usize = 16;

#[derive(Copy, Clone)]
pub struct ThreatPattern {
    pub signature: u32,    // hash of the threat's characteristics
    pub severity: u16,     // 0-1000
    pub occurrences: u16,  // how many times we've seen this pattern
    pub last_seen: u32,    // tick when last detected
    pub response_used: u8, // what response worked
}

impl ThreatPattern {
    pub const fn empty() -> Self {
        Self {
            signature: 0,
            severity: 0,
            occurrences: 0,
            last_seen: 0,
            response_used: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct SentinelState {
    // ── Status ──
    pub online: bool,
    pub alert_level: u8, // 0=calm, 1=vigilant, 2=alert, 3=combat, 4=lockdown
    pub tick: u32,

    // ── Layer 0: Perimeter ──
    pub perimeter_scans: u32,
    pub anomalies_detected: u32,
    pub noise_baseline: u16,   // learned normal noise level
    pub signal_deviation: u16, // how far current signals deviate from baseline

    // ── Layer 1: Recognition ──
    pub self_checks: u32,
    pub foreign_signals: u32,
    pub recognition_accuracy: u16, // 0-1000: how reliably we identify self vs non-self

    // ── Layer 2: Analysis ──
    pub threats_analyzed: u32,
    pub active_threat: u8,    // current threat classification
    pub threat_severity: u16, // 0-1000
    pub threat_vector: u8,    // which nexus_map node is targeted (0-19)

    // ── Layer 3: Response ──
    pub responses_issued: u32,
    pub current_response: u8,
    pub defense_strength: u16,     // 0-1000: overall defensive capability
    pub counterattack_energy: u16, // accumulated for counter-strikes

    // ── Threat memory ──
    pub known_patterns: [ThreatPattern; PATTERN_SLOTS],
    pub pattern_count: u8,

    // ── Fail-safe ──
    pub heartbeat: u32, // increments each tick; if it stops, fail-safe triggers
    pub fail_safe_active: bool,
}

impl SentinelState {
    pub const fn empty() -> Self {
        Self {
            online: false,
            alert_level: 0,
            tick: 0,
            perimeter_scans: 0,
            anomalies_detected: 0,
            noise_baseline: 100,
            signal_deviation: 0,
            self_checks: 0,
            foreign_signals: 0,
            recognition_accuracy: 700,
            threats_analyzed: 0,
            active_threat: THREAT_NONE,
            threat_severity: 0,
            threat_vector: 0,
            responses_issued: 0,
            current_response: RESPONSE_OBSERVE,
            defense_strength: 600,
            counterattack_energy: 0,
            known_patterns: [ThreatPattern::empty(); PATTERN_SLOTS],
            pattern_count: 0,
            fail_safe_active: false,
            heartbeat: 0,
        }
    }
}

pub static STATE: Mutex<SentinelState> = Mutex::new(SentinelState::empty());

pub fn init() {
    let mut s = STATE.lock();
    s.online = true;
    s.alert_level = 1; // start vigilant, not asleep
    s.defense_strength = 600;
    s.recognition_accuracy = 700;
    serial_println!(
        "  life::sentinel: defense hub ONLINE (4-layer, {} pattern slots)",
        PATTERN_SLOTS
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LAYER 0 — PERIMETER SCAN
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Scan incoming signal against baseline. Returns true if anomalous.
pub fn perimeter_scan(signal_energy: u16, noise: u16) -> bool {
    let mut s = STATE.lock();
    if !s.online {
        return false;
    }
    s.perimeter_scans = s.perimeter_scans.saturating_add(1);

    // Learn the noise baseline (slow exponential moving average)
    // baseline = baseline * 0.99 + noise * 0.01 (done in integer math)
    s.noise_baseline = ((s.noise_baseline as u32 * 99 + noise as u32) / 100) as u16;

    // Deviation = how far this signal is from what we expect
    let expected = s.noise_baseline;
    let deviation = if signal_energy > expected {
        signal_energy - expected
    } else {
        expected - signal_energy
    };
    s.signal_deviation = deviation;

    // Anomaly threshold: deviation > 3x baseline noise
    let threshold = s.noise_baseline.saturating_mul(3);
    if deviation > threshold {
        s.anomalies_detected = s.anomalies_detected.saturating_add(1);
        return true;
    }
    false
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LAYER 1 — SELF/NON-SELF RECOGNITION
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Check if a signal carries our self-signature markers.
/// Returns true if the signal is recognized as SELF.
pub fn recognize_self(markers: &[u32]) -> bool {
    let mut s = STATE.lock();
    s.self_checks = s.self_checks.saturating_add(1);

    let mut matches: u8 = 0;
    for marker in markers {
        for sig in &SELF_SIGNATURE {
            if marker == sig {
                matches = matches.saturating_add(1);
                break;
            }
        }
    }

    let is_self = matches >= SIGNATURE_THRESHOLD;
    if !is_self {
        s.foreign_signals = s.foreign_signals.saturating_add(1);
    }

    // Accuracy improves with experience (slowly)
    s.recognition_accuracy = s.recognition_accuracy.saturating_add(1).min(1000);

    is_self
}

/// Get the self-signature markers (for other modules to embed in their signals)
pub fn self_markers() -> [u32; 4] {
    SELF_SIGNATURE
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LAYER 2 — THREAT ANALYSIS
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Analyze a detected anomaly and classify the threat.
pub fn analyze_threat(signal_hash: u32, severity_hint: u16, target_node: u8) -> u8 {
    let mut s = STATE.lock();
    if !s.online {
        return THREAT_NONE;
    }
    s.threats_analyzed = s.threats_analyzed.saturating_add(1);

    // Check threat memory — have we seen this pattern before?
    let mut known_idx: Option<usize> = None;
    let pc = s.pattern_count as usize;
    for i in 0..pc.min(PATTERN_SLOTS) {
        if s.known_patterns[i].signature == signal_hash {
            known_idx = Some(i);
            break;
        }
    }

    // Classify based on severity and whether it's a known pattern
    let classification = if severity_hint < 100 {
        THREAT_PROBE
    } else if severity_hint < 300 {
        THREAT_INTRUSION
    } else if severity_hint < 600 {
        THREAT_CORRUPTION
    } else if severity_hint < 900 {
        THREAT_CASCADE
    } else {
        THREAT_EXISTENTIAL
    };

    // Escalate if this is a repeat offender
    let final_class = if let Some(idx) = known_idx {
        let current_tick = s.tick;
        let pat = &mut s.known_patterns[idx];
        pat.occurrences = pat.occurrences.saturating_add(1);
        pat.last_seen = current_tick;
        pat.severity = severity_hint;
        // Repeat attacks escalate one level
        classification.saturating_add(1).min(THREAT_EXISTENTIAL)
    } else {
        // New pattern — record it
        if pc < PATTERN_SLOTS {
            s.known_patterns[pc] = ThreatPattern {
                signature: signal_hash,
                severity: severity_hint,
                occurrences: 1,
                last_seen: s.tick,
                response_used: RESPONSE_OBSERVE,
            };
            s.pattern_count = s.pattern_count.saturating_add(1);
        }
        classification
    };

    s.active_threat = final_class;
    s.threat_severity = severity_hint;
    s.threat_vector = target_node;

    // Update alert level based on threat
    s.alert_level = match final_class {
        THREAT_NONE => 1,
        THREAT_PROBE => 1,
        THREAT_INTRUSION => 2,
        THREAT_CORRUPTION => 3,
        THREAT_CASCADE => 3,
        THREAT_EXISTENTIAL => 4,
        _ => 4,
    };

    final_class
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// LAYER 3 — GRADUATED RESPONSE
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Issue a graduated response based on current threat level.
/// Returns the response level chosen and the defense energy spent.
pub fn respond() -> (u8, u16) {
    let mut s = STATE.lock();
    if !s.online || s.active_threat == THREAT_NONE {
        s.current_response = RESPONSE_OBSERVE;
        return (RESPONSE_OBSERVE, 0);
    }

    let response = match s.active_threat {
        THREAT_PROBE => RESPONSE_OBSERVE,
        THREAT_INTRUSION => RESPONSE_DEFLECT,
        THREAT_CORRUPTION => RESPONSE_ISOLATE,
        THREAT_CASCADE => RESPONSE_COUNTER,
        THREAT_EXISTENTIAL => RESPONSE_LOCKDOWN,
        _ => RESPONSE_OBSERVE,
    };

    // Energy cost scales with response severity
    let energy_cost: u16 = match response {
        RESPONSE_OBSERVE => 0,
        RESPONSE_DEFLECT => 20,
        RESPONSE_ISOLATE => 50,
        RESPONSE_COUNTER => 100,
        RESPONSE_LOCKDOWN => 200,
        _ => 0,
    };

    s.defense_strength = s.defense_strength.saturating_sub(energy_cost / 4);
    s.current_response = response;
    s.responses_issued = s.responses_issued.saturating_add(1);

    // Build counterattack energy during active defense
    if response >= RESPONSE_ISOLATE {
        s.counterattack_energy = s.counterattack_energy.saturating_add(30).min(1000);
    }

    // Record what response we used for this pattern (for learning)
    let pc = s.pattern_count as usize;
    for i in 0..pc.min(PATTERN_SLOTS) {
        if s.known_patterns[i].last_seen == s.tick {
            s.known_patterns[i].response_used = response;
            break;
        }
    }

    if response >= RESPONSE_ISOLATE {
        serial_println!(
            "sentinel: ACTIVE DEFENSE level={} threat={} severity={} vector={}",
            response,
            s.active_threat,
            s.threat_severity,
            super::nexus_map::node_name(s.threat_vector as usize)
        );
    }

    (response, energy_cost)
}

/// Release a stored counterattack burst against the current threat.
/// Spends accumulated counterattack_energy to amplify defense.
pub fn counterattack() -> u16 {
    let mut s = STATE.lock();
    let burst = s.counterattack_energy;
    if burst < 100 {
        return 0; // not enough energy stored
    }
    s.counterattack_energy = 0;
    s.defense_strength = s.defense_strength.saturating_add(burst / 4).min(1000);
    serial_println!(
        "sentinel: COUNTERATTACK burst={} defense now={}",
        burst,
        s.defense_strength
    );
    burst
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TICK — Called every life_tick
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.tick = age;
    s.heartbeat = s.heartbeat.wrapping_add(1);

    if !s.online {
        // Fail-safe: try to come back online
        s.fail_safe_active = true;
        s.online = true;
        s.alert_level = 3; // come back in alert mode
        serial_println!("sentinel: FAIL-SAFE reboot at tick {}", age);
        return;
    }

    // ── Passive recovery ──
    // Defense strength slowly regenerates during calm periods
    if s.alert_level <= 1 {
        s.defense_strength = s.defense_strength.saturating_add(2).min(1000);
    }

    // Recognition accuracy slowly decays without practice (use it or lose it)
    if s.foreign_signals == 0 && age % 100 == 0 {
        s.recognition_accuracy = s.recognition_accuracy.saturating_sub(1);
    }

    // ── Threat decay ──
    // If no new threats for a while, de-escalate
    if s.active_threat != THREAT_NONE {
        // Check if threat is stale (no re-detection for 50 ticks)
        let pc = s.pattern_count as usize;
        let mut still_active = false;
        for i in 0..pc.min(PATTERN_SLOTS) {
            if s.known_patterns[i].last_seen > age.saturating_sub(50) {
                still_active = true;
                break;
            }
        }
        if !still_active {
            s.active_threat = THREAT_NONE;
            s.threat_severity = 0;
            s.alert_level = s.alert_level.saturating_sub(1).max(1);
            s.current_response = RESPONSE_OBSERVE;
        }
    }

    // ── Pattern memory decay ──
    // Old patterns that haven't been seen in 1000 ticks lose relevance
    let pc = s.pattern_count as usize;
    for i in 0..pc.min(PATTERN_SLOTS) {
        if age.saturating_sub(s.known_patterns[i].last_seen) > 1000 {
            s.known_patterns[i].severity = s.known_patterns[i].severity.saturating_sub(10);
        }
    }

    // ── Report to nexus_map ──
    // Sentinel energy in the map reflects our readiness
    let sentinel_energy = (s.defense_strength as u32 + s.recognition_accuracy as u32) / 2;
    drop(s);
    super::nexus_map::report_energy(super::nexus_map::IMMUNE, sentinel_energy.min(1000) as u16);
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// PUBLIC QUERIES
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn is_online() -> bool {
    STATE.lock().online
}
pub fn alert_level() -> u8 {
    STATE.lock().alert_level
}
pub fn defense_strength() -> u16 {
    STATE.lock().defense_strength
}
pub fn threat_class() -> u8 {
    STATE.lock().active_threat
}

pub fn alert_name(level: u8) -> &'static str {
    match level {
        0 => "CALM",
        1 => "VIGILANT",
        2 => "ALERT",
        3 => "COMBAT",
        _ => "LOCKDOWN",
    }
}

pub fn threat_name(class: u8) -> &'static str {
    match class {
        THREAT_NONE => "CLEAR",
        THREAT_PROBE => "PROBE",
        THREAT_INTRUSION => "INTRUSION",
        THREAT_CORRUPTION => "CORRUPTION",
        THREAT_CASCADE => "CASCADE",
        THREAT_EXISTENTIAL => "EXISTENTIAL",
        _ => "UNKNOWN",
    }
}

/// Full diagnostic report to serial
pub fn report() {
    let s = STATE.lock();
    serial_println!("━━━ SENTINEL (tick {}) ━━━", s.tick);
    serial_println!(
        "  status: {} | alert: {} | heartbeat: {}",
        if s.online { "ONLINE" } else { "OFFLINE" },
        alert_name(s.alert_level),
        s.heartbeat
    );
    serial_println!(
        "  defense: {}/1000 | recognition: {}/1000",
        s.defense_strength,
        s.recognition_accuracy
    );
    serial_println!(
        "  scans: {} | anomalies: {} | foreign: {}",
        s.perimeter_scans,
        s.anomalies_detected,
        s.foreign_signals
    );
    if s.active_threat != THREAT_NONE {
        serial_println!(
            "  ACTIVE THREAT: {} severity={} vector={}",
            threat_name(s.active_threat),
            s.threat_severity,
            super::nexus_map::node_name(s.threat_vector as usize)
        );
        serial_println!(
            "  response: {} | counter_energy: {}",
            match s.current_response {
                RESPONSE_OBSERVE => "OBSERVE",
                RESPONSE_DEFLECT => "DEFLECT",
                RESPONSE_ISOLATE => "ISOLATE",
                RESPONSE_COUNTER => "COUNTER",
                _ => "LOCKDOWN",
            },
            s.counterattack_energy
        );
    } else {
        serial_println!("  threat: CLEAR");
    }
    serial_println!("  known patterns: {}/{}", s.pattern_count, PATTERN_SLOTS);
    if s.fail_safe_active {
        serial_println!("  !! FAIL-SAFE WAS ACTIVATED !!");
    }
    serial_println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
