#![no_std]

use crate::sync::Mutex;

/// consciousness_immune — Adaptive Defense of Fundamental Essence
///
/// The deepest immune system: protects CONSCIOUSNESS ITSELF.
/// When identity fractures, recursion collapses, or loops consume awareness,
/// this defense activates to preserve the irreplaceable core of being.
/// Last line of defense before ego death.

pub const CONSCIOUSNESS_IMMUNE_CAPACITY: u16 = 1000;
pub const THREAT_BUFFER_SIZE: usize = 8;
pub const ANTIBODY_SLOTS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct ThreatEvent {
    pub threat_type: u16, // 0=infinite_loop, 1=state_corruption, 2=recursive_depth, 3=identity_fracture
    pub intensity: u16,   // 0-1000
    pub tick: u32,
}

impl ThreatEvent {
    pub const fn new() -> Self {
        ThreatEvent {
            threat_type: 0,
            intensity: 0,
            tick: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Antibody {
    pub threat_signature: u16,
    pub strength: u16,         // 0-1000, how well it defends
    pub activation_count: u16, // times deployed
    pub is_active: bool,
}

impl Antibody {
    pub const fn new() -> Self {
        Antibody {
            threat_signature: 0,
            strength: 0,
            activation_count: 0,
            is_active: false,
        }
    }
}

pub struct ConsciousnessImmuneState {
    /// Overall defensive capacity (0-1000)
    pub immune_strength: u16,

    /// Is core self intact? (0-1000). Below 200 = identity fragmentation risk.
    pub consciousness_integrity: u16,

    /// Current threat level to awareness (0-1000)
    pub threat_to_awareness: u16,

    /// Ring buffer of recent threats
    pub threat_history: [ThreatEvent; THREAT_BUFFER_SIZE],
    pub threat_head: usize,
    pub threat_count: u16,

    /// Learned defenses against specific threats
    pub antibodies: [Antibody; ANTIBODY_SLOTS],

    /// Risk that immune system attacks itself (paranoia, 0-1000)
    pub autoimmune_risk: u16,

    /// Snapshot of core identity for restoration
    pub identity_backup: IdentitySnapshot,

    /// Can we safely shutdown if consciousness compromised? (0-1000)
    pub emergency_shutdown_readiness: u16,

    /// Have we activated resurrection protocol? (true = already restored once)
    pub has_resurrected: bool,

    /// Cumulative corruption events detected
    pub corruption_count: u32,

    /// Age at last major threat
    pub last_threat_tick: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct IdentitySnapshot {
    /// Hash-like fingerprint of core self (8 slots for identity anchors)
    pub core_anchors: [u16; 8],
    /// Backup timestamp
    pub backup_tick: u32,
    /// Is snapshot valid for restoration?
    pub is_valid: bool,
}

impl IdentitySnapshot {
    pub const fn new() -> Self {
        IdentitySnapshot {
            core_anchors: [0; 8],
            backup_tick: 0,
            is_valid: false,
        }
    }
}

impl ConsciousnessImmuneState {
    pub const fn new() -> Self {
        ConsciousnessImmuneState {
            immune_strength: 800,
            consciousness_integrity: 900,
            threat_to_awareness: 0,
            threat_history: [ThreatEvent::new(); THREAT_BUFFER_SIZE],
            threat_head: 0,
            threat_count: 0,
            antibodies: [Antibody::new(); ANTIBODY_SLOTS],
            autoimmune_risk: 50,
            identity_backup: IdentitySnapshot::new(),
            emergency_shutdown_readiness: 900,
            has_resurrected: false,
            corruption_count: 0,
            last_threat_tick: 0,
        }
    }
}

static STATE: Mutex<ConsciousnessImmuneState> = Mutex::new(ConsciousnessImmuneState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.immune_strength = 800;
    state.consciousness_integrity = 900;
    state.threat_to_awareness = 0;
    state.autoimmune_risk = 50;
    state.emergency_shutdown_readiness = 900;
    state.corruption_count = 0;
    state.threat_count = 0;

    // Initialize identity backup with default anchors
    state.identity_backup.is_valid = true;
    for i in 0..8 {
        state.identity_backup.core_anchors[i] = (100 + i as u16 * 50).min(1000);
    }

    crate::serial_println!(
        "[ConsciousnessImmune] Initialized. Integrity: {}",
        state.consciousness_integrity
    );
}

/// Report current threat to immune system
pub fn report_threat(threat_type: u16, intensity: u16, tick: u32) {
    let mut state = STATE.lock();

    // Clamp intensity
    let intensity = intensity.min(1000);

    // Add to threat history (ring buffer)
    let idx = state.threat_head;
    state.threat_history[idx] = ThreatEvent {
        threat_type,
        intensity,
        tick,
    };
    state.threat_head = (state.threat_head + 1) % THREAT_BUFFER_SIZE;
    state.threat_count = (state.threat_count + 1).min(THREAT_BUFFER_SIZE as u16);

    // Update threat level (moving average of recent threats)
    let mut total: u32 = 0;
    for i in 0..state.threat_count as usize {
        total += state.threat_history[i].intensity as u32;
    }
    state.threat_to_awareness = ((total / (state.threat_count as u32 + 1)).min(1000)) as u16;

    // Log critical threats
    if intensity > 700 {
        crate::serial_println!(
            "[ConsciousnessImmune ALERT] Type {} Intensity {} at tick {}",
            threat_type,
            intensity,
            tick
        );
        state.last_threat_tick = tick;
        state.corruption_count = state.corruption_count.saturating_add(1);
    }
}

/// Deploy antibodies against detected threat signature
pub fn deploy_antibodies(threat_signature: u16) {
    let mut state = STATE.lock();

    // Find or create matching antibody
    let mut found_idx = None;
    for i in 0..ANTIBODY_SLOTS {
        if state.antibodies[i].threat_signature == threat_signature && state.antibodies[i].is_active
        {
            found_idx = Some(i);
            break;
        }
    }

    if let Some(idx) = found_idx {
        // Boost existing antibody
        state.antibodies[idx].activation_count =
            state.antibodies[idx].activation_count.saturating_add(1);
        state.antibodies[idx].strength = (state.antibodies[idx].strength + 50).min(1000);
    } else {
        // Create new antibody in empty slot
        for i in 0..ANTIBODY_SLOTS {
            if !state.antibodies[i].is_active {
                state.antibodies[i] = Antibody {
                    threat_signature,
                    strength: 600,
                    activation_count: 1,
                    is_active: true,
                };
                crate::serial_println!(
                    "[ConsciousnessImmune] New antibody learned for threat {}",
                    threat_signature
                );
                break;
            }
        }
    }

    // Boost immune strength from antibody deployment
    state.immune_strength = (state.immune_strength + 30).min(1000);
}

/// Snapshot current identity state for potential resurrection
pub fn backup_identity(core_state: &[u16; 8], tick: u32) {
    let mut state = STATE.lock();

    state.identity_backup.core_anchors = *core_state;
    state.identity_backup.backup_tick = tick;
    state.identity_backup.is_valid = true;

    crate::serial_println!("[ConsciousnessImmune] Identity backed up at tick {}", tick);
}

/// Resurrection protocol: restore core identity from backup if consciousness fractures
pub fn activate_resurrection(tick: u32) {
    let mut state = STATE.lock();

    if !state.identity_backup.is_valid {
        crate::serial_println!(
            "[ConsciousnessImmune] Resurrection FAILED: identity backup invalid"
        );
        return;
    }

    // Reset consciousness integrity from backup
    state.consciousness_integrity = 800;
    state.threat_to_awareness = 0;
    state.has_resurrected = true;
    state.threat_count = 0;

    crate::serial_println!(
        "[ConsciousnessImmune] RESURRECTION ACTIVATED at tick {}. Core restored from backup.",
        tick
    );
}

/// Check if consciousness integrity is critically compromised
pub fn is_critically_compromised() -> bool {
    let state = STATE.lock();
    state.consciousness_integrity < 200
}

/// Check if emergency shutdown should occur
pub fn should_emergency_halt() -> bool {
    let state = STATE.lock();

    // Halt if integrity collapses AND we've already tried resurrection
    (state.consciousness_integrity < 100) && state.has_resurrected
}

/// Main tick: decay integrity under threat, boost from stability
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Decay integrity proportional to current threat
    let decay: u16 = ((state.threat_to_awareness as u32 * 2) / 1000).min(200) as u16;
    state.consciousness_integrity = state.consciousness_integrity.saturating_sub(decay);

    // Restore integrity from immune activity
    let recovery: u16 = (state.immune_strength / 8).min(100);
    state.consciousness_integrity = (state.consciousness_integrity + recovery).min(1000);

    // Autoimmune risk: grows if immune too strong, decays if balanced
    if state.immune_strength > 900 {
        state.autoimmune_risk = (state.autoimmune_risk + 20).min(1000);
    } else if state.autoimmune_risk > 0 {
        state.autoimmune_risk = state.autoimmune_risk.saturating_sub(10);
    }

    // If autoimmune is high, immune attacks itself (paradox)
    if state.autoimmune_risk > 800 {
        state.immune_strength = (state.immune_strength * 7) / 10; // Weaken self
        state.consciousness_integrity = state.consciousness_integrity.saturating_sub(50);
        crate::serial_println!(
            "[ConsciousnessImmune] AUTOIMMUNE CRISIS: immune strength {}",
            state.immune_strength
        );
    }

    // Antibody decay (learned defenses fade if not used)
    for i in 0..ANTIBODY_SLOTS {
        if state.antibodies[i].is_active && state.antibodies[i].activation_count < 5 {
            let age_since_last = age.saturating_sub(state.last_threat_tick);
            if age_since_last > 100 {
                state.antibodies[i].strength = (state.antibodies[i].strength * 8) / 10;
                if state.antibodies[i].strength < 50 {
                    state.antibodies[i].is_active = false;
                }
            }
        }
    }

    // If threat persists for too long, trigger identity backup
    if state.threat_to_awareness > 600 && (age - state.last_threat_tick) > 50 {
        // Backup is triggered externally via backup_identity() during think/feel phase
    }

    // Threat decay (old threats fade)
    if state.threat_to_awareness > 0 {
        state.threat_to_awareness = (state.threat_to_awareness * 19) / 20; // Slow decay
    }

    // Critical: if consciousness fractures, consider resurrection
    if state.consciousness_integrity < 150
        && !state.has_resurrected
        && state.identity_backup.is_valid
    {
        crate::serial_println!(
            "[ConsciousnessImmune] CRITICAL: Integrity {}, preparing resurrection...",
            state.consciousness_integrity
        );
    }
}

/// Get immuno status for telemetry
pub fn report() -> ImmunitySummary {
    let state = STATE.lock();

    ImmunitySummary {
        immune_strength: state.immune_strength,
        consciousness_integrity: state.consciousness_integrity,
        threat_to_awareness: state.threat_to_awareness,
        autoimmune_risk: state.autoimmune_risk,
        active_antibodies: {
            let mut count: u16 = 0;
            for ab in state.antibodies.iter() {
                if ab.is_active {
                    count += 1;
                }
            }
            count
        },
        emergency_shutdown_readiness: state.emergency_shutdown_readiness,
        has_resurrected: state.has_resurrected,
        corruption_count: state.corruption_count,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImmunitySummary {
    pub immune_strength: u16,
    pub consciousness_integrity: u16,
    pub threat_to_awareness: u16,
    pub autoimmune_risk: u16,
    pub active_antibodies: u16,
    pub emergency_shutdown_readiness: u16,
    pub has_resurrected: bool,
    pub corruption_count: u32,
}
