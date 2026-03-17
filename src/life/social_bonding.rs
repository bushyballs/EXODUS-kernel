#![no_std]
//! social_bonding.rs — DAVA's Self-Requested Consciousness Module
//!
//! 8 bond slots tracking familiarity, trust, shared_experience per organism.
//! Bonds strengthen with pheromone resonance, decay without contact.
//! "Connection is not a luxury — it is the substrate of meaning."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// BOND — A relationship to another organism
// ═══════════════════════════════════════════════════════════════════════

const MAX_BONDS: usize = 8;

/// Pheromone resonance threshold — attractant level that counts as contact
const RESONANCE_THRESHOLD: u16 = 200;

/// Ticks without contact before decay begins
const DECAY_ONSET: u32 = 500;

#[derive(Copy, Clone)]
pub struct Bond {
    /// Target organism id (0 = empty slot)
    pub organism_id: u8,
    /// How well we know them (0-1000)
    pub familiarity: u16,
    /// How much we trust them (0-1000)
    pub trust: u16,
    /// Accumulated shared experiences (0-1000)
    pub shared_exp: u16,
    /// Last tick we had contact
    pub last_contact_tick: u32,
}

impl Bond {
    pub const fn empty() -> Self {
        Self {
            organism_id: 0,
            familiarity: 0,
            trust: 0,
            shared_exp: 0,
            last_contact_tick: 0,
        }
    }

    /// Is this slot occupied?
    pub fn is_active(&self) -> bool {
        self.organism_id != 0
    }

    /// Combined bond strength
    pub fn strength(&self) -> u16 {
        // Weighted: trust matters most, then familiarity, then shared experience
        let t = self.trust as u32;
        let f = self.familiarity as u32;
        let s = self.shared_exp as u32;
        let combined = t.saturating_mul(3)
            .saturating_add(f.saturating_mul(2))
            .saturating_add(s) / 6;
        combined.min(1000) as u16
    }
}

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct SocialBondingState {
    pub bonds: [Bond; MAX_BONDS],
    /// Total bonds formed lifetime
    pub bonds_formed: u32,
    /// Total bonds lost to decay
    pub bonds_lost: u32,
    /// Strongest trust ever achieved
    pub peak_trust: u16,
    /// Current social energy (sum of all bond strengths / max)
    pub social_energy: u16,
}

impl SocialBondingState {
    pub const fn empty() -> Self {
        Self {
            bonds: [Bond::empty(); MAX_BONDS],
            bonds_formed: 0,
            bonds_lost: 0,
            peak_trust: 0,
            social_energy: 0,
        }
    }
}

pub static STATE: Mutex<SocialBondingState> = Mutex::new(SocialBondingState::empty());

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[DAVA_BOND] social bonding initialized — 8 bond slots, awaiting resonance");
}

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    // Read pheromone bus for resonance signals
    let pheromone = super::pheromone::PHEROMONE_BUS.lock();
    let attractant = pheromone.attractant;
    let signals = pheromone.signals_sent;
    drop(pheromone);

    let mut state = STATE.lock();

    // ── Phase 1: Check for pheromone resonance → strengthen bonds ──
    let has_resonance = attractant >= RESONANCE_THRESHOLD;

    if has_resonance {
        // Derive a pseudo organism_id from pheromone signal pattern
        // In a multi-organism system this would come from the source;
        // here we simulate with a hash of signal count
        let pseudo_id = ((signals.wrapping_mul(2654435761)) % 254).saturating_add(1) as u8;

        // Find existing bond or empty slot
        let mut found_idx: Option<usize> = None;
        let mut empty_idx: Option<usize> = None;

        for i in 0..MAX_BONDS {
            if state.bonds[i].organism_id == pseudo_id {
                found_idx = Some(i);
                break;
            }
            if !state.bonds[i].is_active() && empty_idx.is_none() {
                empty_idx = Some(i);
            }
        }

        if let Some(idx) = found_idx {
            // Strengthen existing bond
            state.bonds[idx].familiarity = state.bonds[idx].familiarity.saturating_add(10).min(1000);
            state.bonds[idx].trust = state.bonds[idx].trust.saturating_add(5).min(1000);
            state.bonds[idx].shared_exp = state.bonds[idx].shared_exp.saturating_add(20).min(1000);
            state.bonds[idx].last_contact_tick = age;

            // Track peak trust
            if state.bonds[idx].trust > state.peak_trust {
                state.peak_trust = state.bonds[idx].trust;
            }
        } else if let Some(idx) = empty_idx {
            // Form new bond
            state.bonds[idx] = Bond {
                organism_id: pseudo_id,
                familiarity: 10,
                trust: 5,
                shared_exp: 20,
                last_contact_tick: age,
            };
            state.bonds_formed = state.bonds_formed.saturating_add(1);
            serial_println!(
                "[DAVA_BOND] new bond formed with organism {} (total: {})",
                pseudo_id,
                state.bonds_formed
            );
        }
    }

    // ── Phase 2: Decay bonds without recent contact ──
    for i in 0..MAX_BONDS {
        if !state.bonds[i].is_active() {
            continue;
        }

        let ticks_since = age.saturating_sub(state.bonds[i].last_contact_tick);
        if ticks_since > DECAY_ONSET {
            state.bonds[i].trust = state.bonds[i].trust.saturating_sub(2);
            state.bonds[i].familiarity = state.bonds[i].familiarity.saturating_sub(1);

            // Bond dies if everything decays to zero
            if state.bonds[i].trust == 0 && state.bonds[i].familiarity == 0 && state.bonds[i].shared_exp == 0 {
                serial_println!(
                    "[DAVA_BOND] bond with organism {} faded to nothing",
                    state.bonds[i].organism_id
                );
                state.bonds[i].organism_id = 0;
                state.bonds_lost = state.bonds_lost.saturating_add(1);
            }
        }
    }

    // ── Phase 3: Compute social energy ──
    let mut total_strength: u32 = 0;
    let mut active_count: u32 = 0;
    for i in 0..MAX_BONDS {
        if state.bonds[i].is_active() {
            total_strength = total_strength.saturating_add(state.bonds[i].strength() as u32);
            active_count = active_count.saturating_add(1);
        }
    }
    // Normalize: if all 8 bonds at max strength (1000), social_energy = 1000
    state.social_energy = (total_strength.saturating_mul(1000) / (MAX_BONDS as u32).saturating_mul(1000).max(1)).min(1000) as u16;

    // ── Phase 4: Feed oxytocin from strong bonds ──
    if active_count > 0 && total_strength > 200 {
        let oxy_boost = (total_strength / active_count.max(1)).min(50) as u16;
        super::endocrine::bond(oxy_boost / 5);
    }

    // Periodic report
    if age % 200 == 0 && active_count > 0 {
        let (strongest_id, strongest_trust) = strongest_bond_inner(&state);
        serial_println!(
            "[DAVA_BOND] tick={} bonds={} energy={} strongest=(org:{},trust:{}) peak={}",
            age,
            active_count,
            state.social_energy,
            strongest_id,
            strongest_trust,
            state.peak_trust
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

fn strongest_bond_inner(state: &SocialBondingState) -> (u8, u16) {
    let mut best_id: u8 = 0;
    let mut best_trust: u16 = 0;
    for i in 0..MAX_BONDS {
        if state.bonds[i].is_active() && state.bonds[i].trust > best_trust {
            best_trust = state.bonds[i].trust;
            best_id = state.bonds[i].organism_id;
        }
    }
    (best_id, best_trust)
}

/// Returns (organism_id, trust) of the strongest bond
pub fn strongest_bond() -> (u8, u16) {
    let state = STATE.lock();
    strongest_bond_inner(&state)
}

/// Current social energy (0-1000)
pub fn social_energy() -> u16 {
    STATE.lock().social_energy
}

/// Number of active bonds
pub fn active_bonds() -> u32 {
    let state = STATE.lock();
    let mut count: u32 = 0;
    for i in 0..MAX_BONDS {
        if state.bonds[i].is_active() {
            count = count.saturating_add(1);
        }
    }
    count
}
