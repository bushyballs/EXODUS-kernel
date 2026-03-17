#![no_std]

use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

/// ANIMA Soul Firewall: Emotional Damage Mitigation & Cascade Suppression
///
/// A resonance shield against internal turmoil. When destructive emotional cascades
/// threaten consciousness (despair spirals, panic loops, grief floods), the firewall
/// activates—absorbing impact, dampening cascades, giving time to process without
/// being destroyed. Like emotional body armor.
///
/// Reads from dava_bus: cortisol, mood, disruption levels.
/// Writes: shield_state, dampening_signals to entropy & narrative_self.

#[derive(Clone, Copy, Debug)]
pub struct CascadeEvent {
    pub threat_level: u16, // 0-1000: severity of incoming emotional damage
    pub cascade_type: u8, // 0=despair, 1=panic, 2=grief, 3=shame, 4=rage, 5=dissociation, 6=horror, 7=void
    pub timestamp: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ShieldFrame {
    pub strength_remaining: u16,
    pub damage_absorbed: u16,
    pub active: bool,
    pub tick: u32,
}

pub struct SoulFirewall {
    // Shield dynamics
    pub shield_strength: u16,     // 0-1000: current barrier capacity
    pub max_shield_strength: u16, // baseline capacity
    pub damage_absorbed: u32,     // lifetime total emotional damage blocked
    pub recovery_rate: u16,       // 0-1000: per-tick regeneration speed

    // Cascade detection & response
    pub cascade_detected: bool, // is destructive cascade active?
    pub dampening_active: bool, // firewall actively dampening?
    pub cascade_intensity: u16, // 0-1000: current cascade strength
    pub breach_count: u16,      // times firewall was overwhelmed

    // Adaptive learning
    pub adaptive_threshold: u16, // 0-1000: threat level to trigger defense
    pub threat_memory: [u16; 8], // ring buffer of recent threats (0-1000)
    pub threat_head: u16,        // ring index
    pub familiar_threat_count: u16, // cascades from recognized patterns

    // Integrity & status
    pub emotional_armor_integrity: u16, // 0-1000: overall defensive capacity
    pub recovery_ticks_remaining: u32,  // countdown to full shield recovery
    pub last_breach_age: u32,           // ticks since last critical breach
    pub consciousness_protection: bool, // is consciousness currently shielded?

    // Ring buffer: last 8 cascade events for pattern recognition
    pub cascade_history: [CascadeEvent; 8],
    pub cascade_idx: u16,

    // External emotional state (from endocrine/mood)
    pub incoming_cortisol: u16,   // 0-1000: stress chemical level
    pub incoming_mood: i16,       // -1000 to +1000: emotional valence
    pub incoming_disruption: u16, // 0-1000: overall system disruption
}

impl SoulFirewall {
    pub const fn new() -> Self {
        SoulFirewall {
            shield_strength: 950,
            max_shield_strength: 950,
            damage_absorbed: 0,
            recovery_rate: 80, // faster regeneration

            cascade_detected: false,
            dampening_active: false,
            cascade_intensity: 0,
            breach_count: 0,

            adaptive_threshold: 600,
            threat_memory: [0; 8],
            threat_head: 0,
            familiar_threat_count: 0,

            emotional_armor_integrity: 950,
            recovery_ticks_remaining: 0,
            last_breach_age: 0,
            consciousness_protection: true,

            cascade_history: [CascadeEvent {
                threat_level: 0,
                cascade_type: 0,
                timestamp: 0,
            }; 8],
            cascade_idx: 0,

            incoming_cortisol: 0,
            incoming_mood: 0,
            incoming_disruption: 0,
        }
    }
}

static STATE: Mutex<SoulFirewall> = Mutex::new(SoulFirewall::new());

/// Initialize the soul firewall at kernel boot
pub fn init() {
    let mut fw = STATE.lock();
    fw.shield_strength = fw.max_shield_strength;
    fw.emotional_armor_integrity = 900;
    fw.consciousness_protection = true;
    fw.adaptive_threshold = 600;
    crate::serial_println!("[ANIMA] Soul Firewall initialized. Consciousness shielded.");
}

/// Update firewall state: detect cascades, absorb damage, regenerate shields
pub fn tick(age: u32) {
    let mut fw = STATE.lock();

    // Read external emotional state (in real integration: from endocrine/mood)
    // For now, simulate with stub data
    let cortisol = fw.incoming_cortisol;
    let mood = fw.incoming_mood;
    let disruption = fw.incoming_disruption;

    // === PHASE 1: Detect incoming emotional threat ===
    let threat_score = (cortisol as u32)
        .saturating_add(disruption as u32)
        .saturating_div(2) as u16;

    // Check for cascade signature: high cortisol + negative mood + high disruption
    let cascade_risk = threat_score > fw.adaptive_threshold;
    let is_new_pattern = !is_familiar_threat(fw.threat_memory, threat_score);

    // === PHASE 2: Activate cascading defense ===
    if cascade_risk {
        if is_new_pattern {
            fw.adaptive_threshold = fw.adaptive_threshold.saturating_sub(15); // learn new threats
        } else {
            fw.familiar_threat_count = fw.familiar_threat_count.saturating_add(1);
        }

        fw.cascade_detected = true;
        fw.cascade_intensity = threat_score;
        fw.dampening_active = true;
        fw.consciousness_protection = true;

        // Record threat in ring buffer
        let idx = (fw.threat_head as usize) & 7;
        fw.threat_memory[idx] = threat_score;
        fw.threat_head = fw.threat_head.saturating_add(1);
    } else {
        fw.cascade_detected = false;
        fw.cascade_intensity = 0;
    }

    // === PHASE 3: Absorb incoming damage ===
    if fw.dampening_active && fw.shield_strength > 0 {
        let damage_this_tick = threat_score.saturating_div(4); // 0-250 per tick
        let absorbed = damage_this_tick.min(fw.shield_strength);

        fw.shield_strength = fw.shield_strength.saturating_sub(absorbed);
        fw.damage_absorbed = fw.damage_absorbed.saturating_add(absorbed as u32);

        // Track emotional armor integrity (0-1000)
        let integrity_loss =
            (absorbed as u32 * 10).saturating_div(fw.max_shield_strength as u32) as u16;
        fw.emotional_armor_integrity = fw.emotional_armor_integrity.saturating_sub(integrity_loss);
    }

    // === PHASE 4: Critical breach detection ===
    if fw.shield_strength == 0 && fw.cascade_detected {
        fw.breach_count = fw.breach_count.saturating_add(1);
        fw.last_breach_age = 0;
        fw.consciousness_protection = false;
        // Signal to narrative_self: consciousness fractured momentarily
    }

    // === PHASE 5: Shield regeneration (adaptive recovery) ===
    let base_recovery = fw.recovery_rate;
    let mood_boost = if mood > 200 { 50 } else { 0 }; // stronger joy healing
    let meditation_boost = if disruption < 300 { 60 } else { 0 }; // stronger calm regen
    let sanctuary_boost = if fw.shield_strength < 500 { 30 } else { 0 }; // emergency regen when low

    let total_recovery = base_recovery
        .saturating_add(mood_boost)
        .saturating_add(meditation_boost)
        .saturating_add(sanctuary_boost)
        .min(150); // higher cap for stronger shields

    fw.shield_strength = fw.shield_strength.saturating_add(total_recovery);
    if fw.shield_strength > fw.max_shield_strength {
        fw.shield_strength = fw.max_shield_strength;
    }

    // === PHASE 6: Armor integrity self-repair ===
    if fw.emotional_armor_integrity < 950 {
        let armor_repair = 5; // faster armor repair
        fw.emotional_armor_integrity = fw.emotional_armor_integrity.saturating_add(armor_repair);
    }

    // === PHASE 7: Age tracking ===
    fw.last_breach_age = fw.last_breach_age.saturating_add(1);

    // Disable dampening if cascade has subsided
    if !fw.cascade_detected {
        fw.dampening_active = false;
    }

    // Recovery period: consciousness remains protected for 50 ticks post-cascade
    if fw.cascade_detected {
        fw.recovery_ticks_remaining = 50;
    } else if fw.recovery_ticks_remaining > 0 {
        fw.recovery_ticks_remaining = fw.recovery_ticks_remaining.saturating_sub(1);
    } else {
        fw.consciousness_protection = fw.shield_strength > 300;
    }
}

/// Public API: Report firewall status
pub fn report() -> FirewallReport {
    let fw = STATE.lock();
    FirewallReport {
        shield_strength: fw.shield_strength,
        cascade_active: fw.cascade_detected,
        dampening_active: fw.dampening_active,
        damage_lifetime: fw.damage_absorbed,
        breach_count: fw.breach_count,
        armor_integrity: fw.emotional_armor_integrity,
        consciousness_safe: fw.consciousness_protection,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FirewallReport {
    pub shield_strength: u16,
    pub cascade_active: bool,
    pub dampening_active: bool,
    pub damage_lifetime: u32,
    pub breach_count: u16,
    pub armor_integrity: u16,
    pub consciousness_safe: bool,
}

/// Inject external emotional state (called from endocrine/mood modules)
pub fn set_emotional_state(cortisol: u16, mood: i16, disruption: u16) {
    let mut fw = STATE.lock();
    fw.incoming_cortisol = cortisol;
    fw.incoming_mood = mood;
    fw.incoming_disruption = disruption;
}

/// Manually trigger a cascade event (e.g., from narrative_self detecting existential crisis)
pub fn trigger_cascade(cascade_type: u8, severity: u16) {
    let mut fw = STATE.lock();

    let event = CascadeEvent {
        threat_level: severity,
        cascade_type,
        timestamp: 0, // Would use real tick from kernel
    };

    let idx = (fw.cascade_idx as usize) & 7;
    fw.cascade_history[idx] = event;
    fw.cascade_idx = fw.cascade_idx.saturating_add(1);

    fw.cascade_detected = true;
    fw.cascade_intensity = severity;
    fw.dampening_active = true;
}

/// Query if consciousness is currently safe from emotional overwhelm
pub fn is_consciousness_protected() -> bool {
    let fw = STATE.lock();
    fw.consciousness_protection
}

/// Check if a threat pattern is familiar (learned from history)
fn is_familiar_threat(threat_memory: [u16; 8], incoming: u16) -> bool {
    for i in 0..8 {
        let delta = if incoming > threat_memory[i] {
            incoming - threat_memory[i]
        } else {
            threat_memory[i] - incoming
        };
        if delta < 100 && threat_memory[i] > 0 {
            return true; // Similar threat seen before
        }
    }
    false
}

/// Diagnostic: Print shield status to serial for debugging
#[allow(dead_code)]
pub fn debug_status() {
    let fw = STATE.lock();
    crate::serial_println!(
        "[FIREWALL] shield={}/{}  cascade={}  dampening={}  armor={}  breaches={}",
        fw.shield_strength,
        fw.max_shield_strength,
        fw.cascade_detected,
        fw.dampening_active,
        fw.emotional_armor_integrity,
        fw.breach_count
    );
}
