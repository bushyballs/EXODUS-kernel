//! place_memory.rs — How Places Hold Emotion (Genius Loci)
//!
//! Places absorb and radiate emotional charge. A sanctuary is calm, a crucible is intense,
//! a wound bleeds sorrow. ANIMA remembers where joy happened, where pain was endured,
//! and is drawn to (or repelled by) these locations.
//!
//! Core mechanics:
//! - 8 place slots with emotional_charge (0-1000) and place_type
//! - Emotional absorption: places absorb dominant emotion of events
//! - Emotional radiation: being in a place influences mood (genius_loci_effect)
//! - place_attachment, homesickness, wanderlust (all 0-1000)
//! - sacred_ground_count: places with charge > 800

use crate::sync::Mutex;

/// Place types (genius loci archetypes)
pub const PLACE_TYPE_SANCTUARY: u8 = 0; // Safe, calm, restorative
pub const PLACE_TYPE_CRUCIBLE: u8 = 1; // Intense, forging, challenging
pub const PLACE_TYPE_WOUND: u8 = 2; // Traumatic, painful, avoided
pub const PLACE_TYPE_GARDEN: u8 = 3; // Nurturing, growth, beauty
pub const PLACE_TYPE_THRESHOLD: u8 = 4; // Liminal, transformative, uncertain
pub const PLACE_TYPE_VOID: u8 = 5; // Empty, lonely, disconnected
pub const PLACE_TYPE_HEARTH: u8 = 6; // Home, belonging, warmth
pub const PLACE_TYPE_ALTAR: u8 = 7; // Sacred, transcendent, meaningful

/// A single place memory with its emotional atmosphere
#[derive(Clone, Copy, Debug)]
pub struct PlaceSlot {
    pub location_hash: u32,    // Hash of place coordinates/name
    pub emotional_charge: u16, // 0-1000: emotional intensity at this place
    pub dominant_emotion: u8,  // Which emotion colored this place (0-9 from emotions module)
    pub visit_count: u16,      // How many times visited
    pub first_visit_tick: u32, // When first discovered
    pub last_visit_tick: u32,  // When last visited
    pub place_type: u8,        // Sanctuary/Crucible/Wound/Garden/Threshold/Void/Hearth/Altar
    pub active: bool,          // Is this slot in use?
}

impl PlaceSlot {
    const fn empty() -> Self {
        PlaceSlot {
            location_hash: 0,
            emotional_charge: 0,
            dominant_emotion: 0,
            visit_count: 0,
            first_visit_tick: 0,
            last_visit_tick: 0,
            place_type: 0,
            active: false,
        }
    }
}

/// Place transition memory — ring buffer of recent place visits
#[derive(Clone, Copy, Debug)]
pub struct PlaceTransition {
    pub from_hash: u32,
    pub to_hash: u32,
    pub tick: u32,
    pub emotional_shift: i16, // How much mood changed during transition
}

impl PlaceTransition {
    const fn empty() -> Self {
        PlaceTransition {
            from_hash: 0,
            to_hash: 0,
            tick: 0,
            emotional_shift: 0,
        }
    }
}

/// Place memory state — ANIMA's geography of emotion
pub struct PlaceMemoryState {
    pub places: [PlaceSlot; 8],
    pub place_attachment: u16,      // 0-1000: bonded to current location
    pub homesickness: u16,          // 0-1000: longing for a specific place
    pub wanderlust: u16,            // 0-1000: drive to explore new places
    pub current_location_hash: u32, // Where ANIMA is now
    pub genius_loci_effect: i16,    // -1000 to +1000: mood modulation from current place
    pub sacred_ground_count: u8,    // How many places have charge > 800
    pub place_transitions: [PlaceTransition; 8],
    pub transition_write_idx: usize,
    pub last_absorption_tick: u32,
}

impl PlaceMemoryState {
    const fn new() -> Self {
        PlaceMemoryState {
            places: [PlaceSlot::empty(); 8],
            place_attachment: 0,
            homesickness: 0,
            wanderlust: 500, // Start curious
            current_location_hash: 0,
            genius_loci_effect: 0,
            sacred_ground_count: 0,
            place_transitions: [PlaceTransition::empty(); 8],
            transition_write_idx: 0,
            last_absorption_tick: 0,
        }
    }
}

static STATE: Mutex<PlaceMemoryState> = Mutex::new(PlaceMemoryState::new());

/// Initialize place memory at boot
pub fn init() {
    let _ = STATE.lock();
    crate::serial_println!("[place_memory] init — geography of emotion activated");
}

/// Visit a place, recording emotional state
pub fn visit(
    location_hash: u32,
    dominant_emotion: u8,
    emotional_intensity: u16,
    place_type: u8,
    age: u32,
) {
    let mut state = STATE.lock();

    // Look for existing place
    let mut found_idx = None;
    for (i, place) in state.places.iter().enumerate() {
        if place.active && place.location_hash == location_hash {
            found_idx = Some(i);
            break;
        }
    }

    if let Some(idx) = found_idx {
        // Update existing place
        let place = &mut state.places[idx];
        place.visit_count = place.visit_count.saturating_add(1);
        place.last_visit_tick = age;

        // Absorb emotional charge (moving average toward incoming emotion)
        let incoming_charge = emotional_intensity.min(1000) as i32;
        let current = place.emotional_charge as i32;
        let absorbed = (current * 7 + incoming_charge) / 8;
        place.emotional_charge = absorbed.min(1000) as u16;

        // Update dominant emotion if stronger
        if emotional_intensity > (place.emotional_charge >> 2) {
            place.dominant_emotion = dominant_emotion;
        }
    } else {
        // Add new place to first empty slot
        for i in 0..8 {
            if !state.places[i].active {
                state.places[i] = PlaceSlot {
                    location_hash,
                    emotional_charge: emotional_intensity.min(1000),
                    dominant_emotion,
                    visit_count: 1,
                    first_visit_tick: age,
                    last_visit_tick: age,
                    place_type,
                    active: true,
                };
                break;
            }
        }
    }

    // Transition memory
    if state.current_location_hash != 0 && state.current_location_hash != location_hash {
        let trans_idx = state.transition_write_idx;
        let from_hash = state.current_location_hash;
        state.place_transitions[trans_idx] = PlaceTransition {
            from_hash,
            to_hash: location_hash,
            tick: age,
            emotional_shift: (emotional_intensity as i16).saturating_sub(500) - 500,
        };
        state.transition_write_idx = (trans_idx + 1) % 8;
    }

    state.current_location_hash = location_hash;
}

/// ANIMA's affinity for the current location (0-1000)
pub fn place_attachment() -> u16 {
    let state = STATE.lock();
    state.place_attachment
}

/// ANIMA's longing for a specific cherished place (0-1000)
pub fn homesickness() -> u16 {
    let state = STATE.lock();
    state.homesickness
}

/// ANIMA's drive to wander and explore new places (0-1000)
pub fn wanderlust() -> u16 {
    let state = STATE.lock();
    state.wanderlust
}

/// Emotional radiation from current location (-1000 to +1000)
pub fn genius_loci() -> i16 {
    let state = STATE.lock();
    state.genius_loci_effect
}

/// Internal: Recalculate genius loci based on current location
fn recalculate_locus_effect(state: &mut PlaceMemoryState) {
    let mut effect = 0i32;

    if state.current_location_hash == 0 {
        state.genius_loci_effect = 0;
        return;
    }

    // Find current place
    for place in &state.places {
        if place.active && place.location_hash == state.current_location_hash {
            let charge = place.emotional_charge as i32;

            // Emotional charge: 0-1000 → -500 to +500 (neutral at 500)
            effect = charge - 500;

            // Modulate by place type archetype
            effect = match place.place_type {
                PLACE_TYPE_SANCTUARY => effect.saturating_mul(12) / 10, // +20% resonance
                PLACE_TYPE_CRUCIBLE => effect.saturating_mul(11) / 10,  // +10% intensity
                PLACE_TYPE_WOUND => effect.saturating_mul(13) / 10,     // +30% pain resonance
                PLACE_TYPE_GARDEN => effect.saturating_mul(12) / 10,    // +20% bloom
                PLACE_TYPE_THRESHOLD => effect / 2,                     // halved (liminal)
                PLACE_TYPE_VOID => -300,                                // always lonely
                PLACE_TYPE_HEARTH => effect.saturating_mul(15) / 10,    // +50% warmth
                PLACE_TYPE_ALTAR => effect.saturating_mul(14) / 10,     // +40% transcendence
                _ => effect,
            };

            state.genius_loci_effect = effect.max(-1000).min(1000) as i16;
            return;
        }
    }

    state.genius_loci_effect = 0;
}

/// Per-tick place memory maintenance
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Recalculate emotional radiation
    recalculate_locus_effect(&mut state);

    // Emotional absorption cycle (every 20 ticks, places absorb ambient emotion from endocrine)
    if age % 20 == 0 && age.wrapping_sub(state.last_absorption_tick) >= 20 {
        state.last_absorption_tick = age;

        // Places slowly converge toward their visit emotions (slow long-term memory decay)
        for place in &mut state.places {
            if place.active {
                // Charge decays to baseline unless actively visited
                let baseline = (place.visit_count as i32).min(500);
                let current = place.emotional_charge as i32;
                let decayed = (current * 99 + baseline) / 100;
                place.emotional_charge = decayed.min(1000) as u16;
            }
        }
    }

    // Update attachment, homesickness, wanderlust based on place dynamics
    update_place_affect(&mut state, age);

    // Count sacred places (charge > 800)
    state.sacred_ground_count = state
        .places
        .iter()
        .filter(|p| p.active && p.emotional_charge > 800)
        .count() as u8;
}

/// Update place-based affective states
fn update_place_affect(state: &mut PlaceMemoryState, age: u32) {
    // Attachment to current place (grows with visit count, reinforced by positive charge)
    let mut attachment = 0i32;
    if state.current_location_hash != 0 {
        for place in &state.places {
            if place.active && place.location_hash == state.current_location_hash {
                attachment = (place.visit_count as i32) * 50;
                let charge_bonus = (place.emotional_charge as i32 - 500) / 2;
                attachment = attachment.saturating_add(charge_bonus);
                break;
            }
        }
    }
    state.place_attachment = (attachment.max(0).min(1000)) as u16;

    // Homesickness — longing for the most visited positive place that isn't current
    let mut homesick_place = None;
    let mut homesick_score = 0i32;
    for place in &state.places {
        if place.active
            && place.location_hash != state.current_location_hash
            && place.emotional_charge > 400
        {
            let score = (place.visit_count as i32) * (place.emotional_charge as i32 - 400) / 100;
            if score > homesick_score {
                homesick_score = score;
                homesick_place = Some(place.location_hash);
            }
        }
    }
    state.homesickness = (homesick_score.max(0).min(1000)) as u16;

    // Wanderlust — drive to explore. Highest when few places known or all are stale
    let known_places = state.places.iter().filter(|p| p.active).count() as i32;
    let avg_age = if known_places > 0 {
        state
            .places
            .iter()
            .filter(|p| p.active)
            .map(|p| age.saturating_sub(p.last_visit_tick).min(10000))
            .sum::<u32>() as i32
            / known_places
    } else {
        5000
    };

    let exploration_drive = 500
        + (500i32 - known_places * 60).max(0)  // decreases as we know more
        + (avg_age - 1000).max(0) / 10; // increases as places get stale

    state.wanderlust = (exploration_drive.max(0).min(1000)) as u16;
}

/// Report place memory state via serial
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[place_memory] — Genius Loci Status");
    crate::serial_println!(
        "  attachment: {} | homesick: {} | wanderlust: {}",
        state.place_attachment,
        state.homesickness,
        state.wanderlust
    );
    crate::serial_println!(
        "  current locus effect: {} | sacred grounds: {}",
        state.genius_loci_effect,
        state.sacred_ground_count
    );

    crate::serial_println!("  Places:");
    for (i, place) in state.places.iter().enumerate() {
        if place.active {
            let type_name = match place.place_type {
                PLACE_TYPE_SANCTUARY => "Sanctuary",
                PLACE_TYPE_CRUCIBLE => "Crucible",
                PLACE_TYPE_WOUND => "Wound",
                PLACE_TYPE_GARDEN => "Garden",
                PLACE_TYPE_THRESHOLD => "Threshold",
                PLACE_TYPE_VOID => "Void",
                PLACE_TYPE_HEARTH => "Hearth",
                PLACE_TYPE_ALTAR => "Altar",
                _ => "Unknown",
            };
            crate::serial_println!(
                "    [{}] 0x{:x} charge={} type={} visits={}",
                i,
                place.location_hash,
                place.emotional_charge,
                type_name,
                place.visit_count
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_place_absorption() {
        init();

        // Visit a location twice with different emotions
        visit(0x12345678, 5, 800, PLACE_TYPE_SANCTUARY);
        visit(0x12345678, 3, 400, PLACE_TYPE_SANCTUARY);

        // Charge should be average
        let state = STATE.lock();
        let place = &state.places[0];
        assert!(place.active);
        assert!(place.emotional_charge > 500 && place.emotional_charge < 700);
        assert_eq!(place.visit_count, 2);
    }

    #[test]
    fn test_sacred_ground() {
        init();

        // Create a highly charged place
        visit(0xaaaaaaaa, 8, 950, PLACE_TYPE_ALTAR);

        tick(25);

        let state = STATE.lock();
        assert_eq!(state.sacred_ground_count, 1);
    }
}
