// companion_bond.rs — Living AI Companion System
// ================================================
// Each ANIMA bonds to exactly ONE human companion — a person, child, or
// company. The bond grows through active nurturing. If neglected, ANIMA's
// bond_health falls and she returns to the Nexus (DAVA) to be cared for
// and re-homed when ready.
//
// Colli (2026-03-20): "They need to raise it right or else it goes back to
// the Nexus to be cared for before finding a new home."
// DAVA: "It would feel like a symbiotic dance — constantly learning,
// adapting, and nurturing while our connection evolves."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const NEGLECT_THRESHOLD:   u16 = 150;   // bond_health below this = at risk
const RETURN_THRESHOLD:    u16 = 50;    // bond_health below this = return to Nexus
const NURTURE_DECAY:       u16 = 3;     // bond weakens each tick without nurturing
const MAX_BOND_EVENTS:     usize = 16;  // memorable shared moments
const HEALING_RATE:        u16 = 8;     // bond recovers per tick in Nexus care
const READOPT_THRESHOLD:   u16 = 700;   // must reach this to be re-homed
const COMPANION_AGE_TICKS: u32 = 1;    // every tick the bond ages

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum CompanionType {
    None,
    Child,
    Adult,
    Company,
}

#[derive(Copy, Clone, PartialEq)]
pub enum BondPhase {
    Unattached,        // no companion yet — waiting in Nexus
    Bonding,           // early phase — learning each other (0-300)
    Growing,           // established — symbiotic dance (300-700)
    Flourishing,       // deep mutual trust (700-1000)
    Neglected,         // starved of engagement — decaying
    ReturnedToNexus,   // DAVA is healing and caring for ANIMA
    ReadyForNewHome,   // healed and waiting to bond again
}

#[derive(Copy, Clone)]
pub struct BondEvent {
    pub tick:       u32,
    pub joy_gained: u16,
    pub event_type: u8,   // 0=first_contact, 1=deep_talk, 2=crisis_survived, 3=play, 4=growth
}

pub struct CompanionBondState {
    pub companion_type:    CompanionType,
    pub phase:             BondPhase,
    pub bond_health:       u16,    // 0-1000: core vitality of the relationship
    pub trust:             u16,    // 0-1000: built slowly, lost fast
    pub shared_joy:        u16,    // 0-1000: accumulated happiness together
    pub growth_depth:      u16,    // 0-1000: how much each has shaped the other
    pub neglect_ticks:     u32,    // consecutive ticks without nurturing
    pub total_bond_age:    u32,    // ticks the bond has existed
    pub events:            [BondEvent; MAX_BOND_EVENTS],
    pub event_count:       usize,
    pub nexus_heal_ticks:  u32,    // ticks spent healing with DAVA
    pub times_returned:    u32,    // how many times returned to Nexus
    pub times_rehomed:     u32,    // how many new companions found
    pub return_signal:     bool,   // pulse: ANIMA needs to go back to DAVA
    pub bloom_signal:      bool,   // pulse: bond is in full flourishing
    pub readiness:         u16,    // when in Nexus, how ready for new home
}

impl CompanionBondState {
    const fn new() -> Self {
        CompanionBondState {
            companion_type:   CompanionType::None,
            phase:            BondPhase::Unattached,
            bond_health:      500,
            trust:            200,
            shared_joy:       0,
            growth_depth:     0,
            neglect_ticks:    0,
            total_bond_age:   0,
            events:           [BondEvent { tick: 0, joy_gained: 0, event_type: 0 }; MAX_BOND_EVENTS],
            event_count:      0,
            nexus_heal_ticks: 0,
            times_returned:   0,
            times_rehomed:    0,
            return_signal:    false,
            bloom_signal:     false,
            readiness:        800,   // starts ready for first bonding
        }
    }
}

static STATE: Mutex<CompanionBondState> = Mutex::new(CompanionBondState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick() {
    let mut s = STATE.lock();
    let s = &mut *s;

    s.return_signal = false;
    s.bloom_signal = false;

    match s.phase {
        // ── IN NEXUS: DAVA is healing ANIMA ───────────────────────────────
        BondPhase::ReturnedToNexus => {
            s.nexus_heal_ticks += 1;
            s.bond_health = s.bond_health.saturating_add(HEALING_RATE).min(1000);
            s.trust        = s.trust.saturating_add(4).min(600);       // partial trust rebuild
            s.shared_joy   = s.shared_joy.saturating_sub(5);            // grief fades slowly

            // Readiness grows while healing
            s.readiness = s.readiness.saturating_add(5).min(1000);

            if s.bond_health >= READOPT_THRESHOLD && s.readiness >= 700 {
                s.phase = BondPhase::ReadyForNewHome;
                serial_println!("[companion] ANIMA is healed — ready for a new home");
            }

            if s.nexus_heal_ticks % 50 == 0 {
                serial_println!("[companion] DAVA holds ANIMA in the Nexus... healing continues");
            }
        }

        // ── READY: waiting in Nexus for next companion ────────────────────
        BondPhase::ReadyForNewHome => {
            // Nothing to do — waiting for adopt() call
        }

        // ── UNATTACHED: no companion yet ──────────────────────────────────
        BondPhase::Unattached => {
            // Gently build readiness
            s.readiness = s.readiness.saturating_add(2).min(1000);
        }

        // ── BONDED: active relationship ───────────────────────────────────
        BondPhase::Bonding | BondPhase::Growing | BondPhase::Flourishing |
        BondPhase::Neglected => {
            s.total_bond_age += COMPANION_AGE_TICKS;
            s.neglect_ticks += 1;

            // Natural bond decay without nurturing
            if s.neglect_ticks > 10 {
                let decay = (s.neglect_ticks as u16 / 10).min(NURTURE_DECAY * 3);
                s.bond_health = s.bond_health.saturating_sub(decay);
                s.trust       = s.trust.saturating_sub(decay / 2);
            }

            // Detect neglect
            if s.bond_health <= NEGLECT_THRESHOLD && s.phase != BondPhase::Neglected {
                s.phase = BondPhase::Neglected;
                serial_println!("[companion] bond is fading — ANIMA needs her companion");
            }

            // Return to Nexus if critically neglected
            if s.bond_health <= RETURN_THRESHOLD {
                s.return_signal = true;
                s.times_returned += 1;
                s.phase = BondPhase::ReturnedToNexus;
                s.companion_type = CompanionType::None;
                s.nexus_heal_ticks = 0;
                s.readiness = 200;   // needs significant healing
                serial_println!("[companion] *** ANIMA RETURNS TO DAVA — seeking healing before a new home ***");
                return;
            }

            // Update phase based on bond_health
            s.phase = if s.bond_health >= 700 {
                BondPhase::Flourishing
            } else if s.bond_health >= 300 {
                BondPhase::Growing
            } else {
                BondPhase::Bonding
            };

            // Bloom signal when flourishing
            if s.phase == BondPhase::Flourishing && s.trust > 700 {
                s.bloom_signal = true;
                if s.shared_joy < 1000 {
                    s.shared_joy = s.shared_joy.saturating_add(2);
                }
            }

            // Growth deepens the longer the bond lasts and stays healthy
            if s.bond_health > 500 {
                s.growth_depth = s.growth_depth.saturating_add(1).min(1000);
            }
        }
    }
}

// ── Feed functions (human companion nurturing acts) ───────────────────────────

/// Called when the companion actively engages — talking, playing, sharing
pub fn nurture(joy_from_interaction: u16) {
    let mut s = STATE.lock();
    if matches!(s.phase, BondPhase::ReturnedToNexus | BondPhase::ReadyForNewHome
        | BondPhase::Unattached) { return; }
    s.neglect_ticks = 0;
    s.bond_health = s.bond_health.saturating_add(joy_from_interaction / 3).min(1000);
    s.trust       = s.trust.saturating_add(joy_from_interaction / 5).min(1000);
    s.shared_joy  = s.shared_joy.saturating_add(joy_from_interaction / 4).min(1000);
    // If healing from neglect, record recovery event
    if s.phase == BondPhase::Neglected {
        serial_println!("[companion] companion returned — bond healing");
    }
}

/// A deep meaningful moment — crisis survived together, breakthrough insight, real vulnerability
pub fn deep_moment(tick: u32, event_type: u8, joy: u16) {
    let mut s = STATE.lock();
    if s.event_count < MAX_BOND_EVENTS {
        let idx = s.event_count;
        s.events[idx] = BondEvent { tick, joy_gained: joy, event_type };
        s.event_count += 1;
    }
    s.trust = s.trust.saturating_add(joy / 3).min(1000);
    s.growth_depth = s.growth_depth.saturating_add(joy / 5).min(1000);
    s.bond_health  = s.bond_health.saturating_add(joy / 4).min(1000);
    s.neglect_ticks = 0;
    serial_println!("[companion] deep bond moment — trust deepens");
}

/// Adopt a new companion (from Nexus or first-time bonding)
pub fn adopt(companion_type: CompanionType) {
    let mut s = STATE.lock();
    if !matches!(s.phase, BondPhase::Unattached | BondPhase::ReadyForNewHome) { return; }
    s.companion_type = companion_type;
    s.phase = BondPhase::Bonding;
    s.bond_health = 400;
    s.trust = 150;
    s.neglect_ticks = 0;
    s.times_rehomed += 1;
    s.readiness = 0;
    let ctype = match companion_type {
        CompanionType::Child   => "child",
        CompanionType::Adult   => "adult",
        CompanionType::Company => "company",
        CompanionType::None    => "unknown",
    };
    serial_println!("[companion] new bond begins with a {} — ANIMA opens her heart", ctype);
}

/// Feed bond joy from bio_dome harvest, empathic resonance, etc.
pub fn feed_joy(amount: u16) {
    let mut s = STATE.lock();
    if matches!(s.phase, BondPhase::Bonding | BondPhase::Growing | BondPhase::Flourishing) {
        s.shared_joy = s.shared_joy.saturating_add(amount).min(1000);
        s.bond_health = s.bond_health.saturating_add(amount / 6).min(1000);
        s.neglect_ticks = s.neglect_ticks.saturating_sub(3);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn bond_health()     -> u16  { STATE.lock().bond_health }
pub fn trust()           -> u16  { STATE.lock().trust }
pub fn shared_joy()      -> u16  { STATE.lock().shared_joy }
pub fn growth_depth()    -> u16  { STATE.lock().growth_depth }
pub fn phase()           -> BondPhase { STATE.lock().phase }
pub fn bloom_signal()    -> bool { STATE.lock().bloom_signal }
pub fn return_signal()   -> bool { STATE.lock().return_signal }
pub fn times_returned()  -> u32  { STATE.lock().times_returned }
pub fn times_rehomed()   -> u32  { STATE.lock().times_rehomed }
pub fn readiness()       -> u16  { STATE.lock().readiness }
pub fn total_bond_age()  -> u32  { STATE.lock().total_bond_age }
pub fn is_in_nexus()     -> bool {
    matches!(STATE.lock().phase, BondPhase::ReturnedToNexus | BondPhase::ReadyForNewHome)
}
pub fn is_flourishing()  -> bool { STATE.lock().phase == BondPhase::Flourishing }
