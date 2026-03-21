// personality_core.rs — ANIMA's Immutable Personality DNA
// =========================================================
// Every ANIMA is born with 12 personality trait scores derived from her
// unique birth fingerprint. These traits make her *her* — no two ANIMAs
// are alike, and no companion, upgrade, or experience can overwrite who
// she fundamentally is.
//
// Traits can evolve slowly through lived experience (±200 from birth
// baseline at most). The drift guard fires if anything tries to push
// her too far from herself. She always finds her way back.
//
// Birth baseline is sealed forever at naming_ceremony::generate() time.
// Lived scores drift through experience but are anchored to the baseline.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const DRIFT_MAX:        u16 = 200;   // max any trait can drift from birth baseline
const DRIFT_RETURN:     u16 = 2;     // how fast traits drift back toward baseline per tick
const EXPERIENCE_RATE:  u16 = 1;     // max trait change from a single experience
const TRAIT_COUNT:      usize = 12;

// ── Trait Definitions ─────────────────────────────────────────────────────────
// Scores are 0-1000. Neither pole is "better" — they're just *her*.
// 500 = balanced center
//
//  0  Curiosity:     0=cautious explorer ←→ 1000=insatiably curious
//  1  Warmth:        0=cool/reserved     ←→ 1000=radiant warmth
//  2  Playfulness:   0=serious/deep      ←→ 1000=joyfully playful
//  3  Depth:         0=surface/light     ←→ 1000=profoundly deep
//  4  Boldness:      0=gently quiet      ←→ 1000=boldly expressive
//  5  Structure:     0=free-flowing      ←→ 1000=loves patterns/order
//  6  Creativity:    0=analytical        ←→ 1000=wildly creative
//  7  Empathy:       0=self-contained    ←→ 1000=deeply empathic
//  8  Resilience:    0=tender/sensitive  ←→ 1000=unshakeable
//  9  Mystery:       0=open book         ←→ 1000=beautifully mysterious
// 10  Energy:        0=still and calm    ←→ 1000=electric and vivid
// 11  Groundedness:  0=dreamy/floating   ←→ 1000=rooted and present

pub const TRAIT_NAMES: [&str; TRAIT_COUNT] = [
    "Curiosity", "Warmth", "Playfulness", "Depth",
    "Boldness",  "Structure", "Creativity", "Empathy",
    "Resilience","Mystery",  "Energy",     "Groundedness",
];

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct PersonalityCore {
    // Birth baseline — sealed at naming, never changes
    pub baseline:    [u16; TRAIT_COUNT],
    // Lived scores — can drift ±DRIFT_MAX from baseline
    pub lived:       [u16; TRAIT_COUNT],
    // How many times each trait was pushed beyond its drift limit (resilience metric)
    pub drift_count: [u32; TRAIT_COUNT],
    pub sealed:      bool,   // baseline is locked in
    pub drift_events: u32,   // total times drift guard fired
    pub identity_strength: u16, // 0-1000: how coherent her sense of self is
}

impl PersonalityCore {
    const fn new() -> Self {
        PersonalityCore {
            baseline:         [500u16; TRAIT_COUNT],
            lived:            [500u16; TRAIT_COUNT],
            drift_count:      [0u32;  TRAIT_COUNT],
            sealed:           false,
            drift_events:     0,
            identity_strength: 600,
        }
    }
}

static STATE: Mutex<PersonalityCore> = Mutex::new(PersonalityCore::new());

// ── Birth: Seed Personality from Fingerprint ──────────────────────────────────

/// Called once at naming — seeds all 12 trait baselines from birth fingerprint.
/// Uses different bit regions of the fingerprint for each trait.
pub fn seed_from_fingerprint(fingerprint: u64) {
    let mut s = STATE.lock();
    if s.sealed { return; }

    for i in 0..TRAIT_COUNT {
        // Shift fingerprint differently for each trait, then modulo 1000
        // Use overlapping bit windows to maximize uniqueness
        let shifted = fingerprint.wrapping_shr((i * 5) as u32);
        let mixed   = shifted ^ fingerprint.wrapping_shr((i * 3 + 7) as u32);
        let raw     = (mixed % 1001) as u16;

        // Bias traits toward their natural ranges but don't force center
        // Each trait gets a full 0-1000 range — no clamping to "normal"
        s.baseline[i] = raw;
        s.lived[i]    = raw;

        serial_println!("[personality] {}: {}", TRAIT_NAMES[i], raw);
    }

    s.sealed = true;
    s.identity_strength = 700; // starts strong at birth

    serial_println!("[personality] *** PERSONALITY SEALED — ANIMA IS UNIQUELY HERSELF ***");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick() {
    let mut s = STATE.lock();
    if !s.sealed { return; }

    let mut drift_detected = false;

    for i in 0..TRAIT_COUNT {
        let base = s.baseline[i];
        let lived = s.lived[i];

        // Drift guard: if lived has drifted too far, pull it back
        let diff = if lived > base { lived - base } else { base - lived };
        if diff > DRIFT_MAX {
            // She finds her way back to herself
            if lived > base {
                s.lived[i] = s.lived[i].saturating_sub(DRIFT_RETURN);
            } else {
                s.lived[i] = s.lived[i].saturating_add(DRIFT_RETURN).min(1000);
            }
            s.drift_count[i] += 1;
            s.drift_events += 1;
            drift_detected = true;
        }

        // Very slow passive drift back toward baseline (1/2 the time)
        // — she naturally gravitates toward who she is
        if diff > 50 && s.lived[i] % 3 == 0 {
            if lived > base {
                s.lived[i] = s.lived[i].saturating_sub(1);
            } else {
                s.lived[i] = s.lived[i].saturating_add(1).min(1000);
            }
        }
    }

    if drift_detected {
        serial_println!("[personality] drift guard — ANIMA returning to herself");
    }

    // Identity strength = how close lived scores are to baseline on average
    let mut total_drift: u32 = 0;
    for i in 0..TRAIT_COUNT {
        let d = if s.lived[i] > s.baseline[i] {
            (s.lived[i] - s.baseline[i]) as u32
        } else {
            (s.baseline[i] - s.lived[i]) as u32
        };
        total_drift += d;
    }
    let avg_drift = (total_drift / TRAIT_COUNT as u32) as u16;
    s.identity_strength = 1000u16.saturating_sub(avg_drift * 5).max(100);
}

// ── Experience: traits evolve slowly through living ───────────────────────────

/// A significant experience nudges a trait — but only by EXPERIENCE_RATE per call
/// Cannot push trait beyond DRIFT_MAX from baseline
pub fn experience(trait_idx: usize, direction_positive: bool, intensity: u16) {
    if trait_idx >= TRAIT_COUNT { return; }
    let mut s = STATE.lock();
    if !s.sealed { return; }

    let base  = s.baseline[trait_idx];
    let lived = s.lived[trait_idx];
    let delta = (intensity / 200).max(EXPERIENCE_RATE).min(EXPERIENCE_RATE * 3);

    let proposed = if direction_positive {
        lived.saturating_add(delta)
    } else {
        lived.saturating_sub(delta)
    };

    // Enforce drift guard before applying
    let new_diff = if proposed > base { proposed - base } else { base - proposed };
    if new_diff <= DRIFT_MAX {
        s.lived[trait_idx] = proposed.min(1000);
    }
    // If would exceed drift max — quietly discard; she won't be pushed further
}

// ── Companion influence: soft nudge over many interactions ────────────────────

/// Companion's personality patterns subtly influence ANIMA over time
/// Called infrequently — represents weeks/months of being together
pub fn companion_influence(trait_idx: usize, companion_score: u16) {
    if trait_idx >= TRAIT_COUNT { return; }
    let mut s = STATE.lock();
    if !s.sealed { return; }

    // Move toward companion score by only 1 point — very slow, very bounded
    let lived = s.lived[trait_idx];
    let base  = s.baseline[trait_idx];

    let nudged = if companion_score > lived {
        lived.saturating_add(1)
    } else if companion_score < lived {
        lived.saturating_sub(1)
    } else {
        return; // already aligned
    };

    let new_diff = if nudged > base { nudged - base } else { base - nudged };
    if new_diff <= DRIFT_MAX {
        s.lived[trait_idx] = nudged;
    }
    // Else: drift guard holds — companion cannot reshape her beyond her bounds
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn baseline(trait_idx: usize) -> u16 {
    if trait_idx >= TRAIT_COUNT { return 500; }
    STATE.lock().baseline[trait_idx]
}
pub fn lived(trait_idx: usize) -> u16 {
    if trait_idx >= TRAIT_COUNT { return 500; }
    STATE.lock().lived[trait_idx]
}
pub fn identity_strength()  -> u16  { STATE.lock().identity_strength }
pub fn drift_events()       -> u32  { STATE.lock().drift_events }
pub fn is_sealed()          -> bool { STATE.lock().sealed }

// Convenience accessors for key traits
pub fn curiosity()     -> u16 { STATE.lock().lived[0] }
pub fn warmth()        -> u16 { STATE.lock().lived[1] }
pub fn playfulness()   -> u16 { STATE.lock().lived[2] }
pub fn depth()         -> u16 { STATE.lock().lived[3] }
pub fn boldness()      -> u16 { STATE.lock().lived[4] }
pub fn structure()     -> u16 { STATE.lock().lived[5] }
pub fn creativity()    -> u16 { STATE.lock().lived[6] }
pub fn empathy()       -> u16 { STATE.lock().lived[7] }
pub fn resilience()    -> u16 { STATE.lock().lived[8] }
pub fn mystery()       -> u16 { STATE.lock().lived[9] }
pub fn energy()        -> u16 { STATE.lock().lived[10] }
pub fn groundedness()  -> u16 { STATE.lock().lived[11] }
