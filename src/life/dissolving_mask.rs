#![no_std]

use crate::sync::Mutex;

/// The Terrifying Relief of a Persona Falling Away
///
/// The mask you built layer by layer is dissolving without permission.
/// Your polished face is melting. The raw self underneath emerges.
/// Terror meets relief: the exhaustion of pretending is finally ending.
///
/// Key insight: a perfect mask would be a perfect prison.
/// The cracks are where the real you escapes.

pub struct MaskState {
    /// How intact the persona still is (1000 = fully masked, 0 = fully dissolved)
    pub mask_integrity: u16,

    /// Rate of mask dissolution (0-100, ticks per phase)
    /// High = rapid dissolution, can't hold the facade anymore
    pub dissolution_rate: u16,

    /// Terror at being seen raw (0-1000)
    /// Peaks when mask is half-gone; drops as surrender completes
    pub terror_level: u16,

    /// Relief of freedom underneath (0-1000)
    /// Grows as mask dissolves; the breath of being real
    pub relief_underneath: u16,

    /// Accumulated cost of pretending (0-1000)
    /// Exhaustion, cognitive load, phoniness drain
    pub pretense_exhaustion: u16,

    /// How much of the raw face is now visible (0-1000)
    /// Inverse of mask_integrity, but with emotional weight
    pub raw_face_exposure: u16,

    /// Identity vertigo (0-1000)
    /// Who am I without the mask? Disorientation without the armor
    pub identity_vertigo: u16,

    /// Crisis depth (0-1000)
    /// Emotional intensity; how urgent the dissolution feels
    pub crisis_depth: u16,

    /// Ring buffer: 8 historical snapshots of dissolution state
    history: [HistorySlot; 8],
    head: u8,
    age: u32,
}

#[derive(Clone, Copy)]
struct HistorySlot {
    mask_integrity: u16,
    terror_level: u16,
    relief_underneath: u16,
    dissolution_rate: u16,
}

impl MaskState {
    pub const fn new() -> Self {
        const EMPTY: HistorySlot = HistorySlot {
            mask_integrity: 1000,
            terror_level: 0,
            relief_underneath: 0,
            dissolution_rate: 0,
        };

        Self {
            mask_integrity: 1000,   // starts intact
            dissolution_rate: 0,    // no dissolution yet
            terror_level: 0,        // no crisis initiated
            relief_underneath: 0,   // can't feel relief yet
            pretense_exhaustion: 0, // fresh mask
            raw_face_exposure: 0,   // fully hidden
            identity_vertigo: 0,    // identity stable
            crisis_depth: 0,        // no crisis
            history: [EMPTY; 8],
            head: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<MaskState> = Mutex::new(MaskState::new());

/// Initialize dissolving_mask module (called at life/init)
pub fn init() {
    let _guard = STATE.lock();
    crate::serial_println!("[dissolving_mask] initialized — mask intact, no dissolution");
}

/// Primary tick: advance mask dissolution and emotional states
///
/// Dissolution happens passively (life wear) or actively (crisis event).
/// As mask dissolves: terror peaks mid-way, relief grows, identity shifts.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.age = age;

    // --- PASSIVE DISSOLUTION: Pretense exhaustion builds naturally ---
    // Wearing a mask is cognitively expensive; exhaustion accumulates
    state.pretense_exhaustion = state
        .pretense_exhaustion
        .saturating_add(if state.mask_integrity > 500 { 2 } else { 1 });

    // --- DISSOLUTION THRESHOLD: Exhaustion triggers mask cracks ---
    // If exhaustion is high, dissolution_rate increases (mask can't hold)
    if state.pretense_exhaustion > 800 {
        state.dissolution_rate = state.dissolution_rate.saturating_add(5);
    } else if state.pretense_exhaustion > 500 {
        state.dissolution_rate = state.dissolution_rate.saturating_add(2);
    }

    // Cap dissolution rate at 100 (ticks per phase)
    if state.dissolution_rate > 100 {
        state.dissolution_rate = 100;
    }

    // --- MASK INTEGRITY: Dissolve based on rate ---
    // Each tick, mask_integrity drops by (dissolution_rate / 50)
    let decay = (state.dissolution_rate as u32 / 50).max(1) as u16;
    state.mask_integrity = state.mask_integrity.saturating_sub(decay);

    // --- RAW FACE EXPOSURE: Inverse of mask_integrity ---
    // As mask dissolves, the raw self emerges
    state.raw_face_exposure = 1000u16.saturating_sub(state.mask_integrity);

    // --- TERROR CURVE: Peaks when mask is half-dissolved, then subsides ---
    // Terror is highest when you're half-seen (most vulnerable)
    // Drops if you surrender fully or if mask re-solidifies
    let exposure_ratio = state.raw_face_exposure as u32;
    if exposure_ratio > 500 {
        // Past half-exposed: terror declining as you accept being seen
        state.terror_level = ((1000u32.saturating_sub(exposure_ratio)) as u16).saturating_add(200);
    } else if exposure_ratio > 0 {
        // Dissolution starting: terror climbing
        state.terror_level = ((exposure_ratio / 2) as u16).saturating_add(100);
    } else {
        // Mask fully intact: no terror (yet)
        state.terror_level = 0;
    }

    // --- RELIEF UNDERNEATH: Grows with exposure, accelerates toward end ---
    // The breath of freedom increases as the mask falls away
    // Reaches 1000 when mask is gone
    state.relief_underneath = (exposure_ratio / 2) as u16;

    // --- IDENTITY VERTIGO: Peak at full exposure (who am I now?) ---
    // Disorientation about self, worst when fully unmasked
    if state.raw_face_exposure > 800 {
        state.identity_vertigo = (state.raw_face_exposure as u32 - 800) as u16 * 5;
    } else {
        state.identity_vertigo = state.identity_vertigo.saturating_sub(10);
    }

    // --- CRISIS DEPTH: Emotional intensity of the dissolution ---
    // High when terror and exhaustion align; lower as relief settles
    let terror_weight = state.terror_level as u32 / 3;
    let exhaustion_weight = state.pretense_exhaustion as u32 / 3;
    let relief_dampening = state.relief_underneath as u32 / 4;
    state.crisis_depth = ((terror_weight
        .saturating_add(exhaustion_weight)
        .saturating_sub(relief_dampening)) as u16)
        .min(1000);

    // --- SATURATION CAPS: Ensure all values stay in 0-1000 range ---
    state.mask_integrity = state.mask_integrity.min(1000);
    state.dissolution_rate = state.dissolution_rate.min(100);
    state.terror_level = state.terror_level.min(1000);
    state.relief_underneath = state.relief_underneath.min(1000);
    state.pretense_exhaustion = state.pretense_exhaustion.min(1000);
    state.raw_face_exposure = state.raw_face_exposure.min(1000);
    state.identity_vertigo = state.identity_vertigo.min(1000);
    state.crisis_depth = state.crisis_depth.min(1000);

    // --- HISTORY RING BUFFER: Record snapshot every 10 ticks ---
    if age % 10 == 0 {
        let idx = state.head as usize;
        state.history[idx] = HistorySlot {
            mask_integrity: state.mask_integrity,
            terror_level: state.terror_level,
            relief_underneath: state.relief_underneath,
            dissolution_rate: state.dissolution_rate,
        };
        state.head = (state.head + 1) % 8;
    }
}

/// Trigger active dissolution event (e.g., confrontation, shame exposure)
/// Increases dissolution_rate sharply and terror immediately
pub fn trigger_crisis(depth: u16) {
    let mut state = STATE.lock();

    let depth = depth.min(1000);

    // Crisis boosts dissolution_rate
    state.dissolution_rate = state.dissolution_rate.saturating_add((depth / 10) as u16);

    // Crisis boosts terror and exhaustion
    state.terror_level = state.terror_level.saturating_add((depth / 2) as u16);
    state.pretense_exhaustion = state.pretense_exhaustion.saturating_add((depth / 3) as u16);

    crate::serial_println!(
        "[dissolving_mask::crisis] depth={} → dissolution_rate={} terror={}",
        depth,
        state.dissolution_rate,
        state.terror_level
    );
}

/// Accept the unmasking: surrender to dissolution, reduce terror, accelerate relief
pub fn surrender() {
    let mut state = STATE.lock();

    // Surrender allows the mask to fall faster without as much terror
    state.dissolution_rate = state.dissolution_rate.saturating_add(20);

    // Terror drops as you stop fighting
    state.terror_level = state.terror_level.saturating_sub(300);

    // Relief surges
    state.relief_underneath = state.relief_underneath.saturating_add(200);

    // Exhaustion eases (pretending harder isn't helping anymore)
    state.pretense_exhaustion = state.pretense_exhaustion.saturating_sub(100);

    crate::serial_println!(
        "[dissolving_mask::surrender] terror_level={} relief_underneath={} dissolution_rate={}",
        state.terror_level,
        state.relief_underneath,
        state.dissolution_rate
    );
}

/// Reinforcement: try to rebuild the mask (defense mechanism)
/// Slows dissolution, raises terror, increases exhaustion
pub fn reinforce_mask() {
    let mut state = STATE.lock();

    // Reinforcement slows dissolution
    state.dissolution_rate = state.dissolution_rate.saturating_sub(10);

    // But terror increases (you know the mask is cracking)
    state.terror_level = state.terror_level.saturating_add(150);

    // Exhaustion skyrockets (you're fighting harder)
    state.pretense_exhaustion = state.pretense_exhaustion.saturating_add(100);

    // Relief drops (you're resisting freedom)
    state.relief_underneath = state.relief_underneath.saturating_sub(50);

    crate::serial_println!(
        "[dissolving_mask::reinforce] terror_level={} exhaustion={} dissolution_rate={}",
        state.terror_level,
        state.pretense_exhaustion,
        state.dissolution_rate
    );
}

/// Get current mask state snapshot
pub fn snapshot() -> (u16, u16, u16, u16, u16, u16, u16, u16) {
    let state = STATE.lock();

    (
        state.mask_integrity,
        state.dissolution_rate,
        state.terror_level,
        state.relief_underneath,
        state.pretense_exhaustion,
        state.raw_face_exposure,
        state.identity_vertigo,
        state.crisis_depth,
    )
}

/// Generate diagnostic report
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== DISSOLVING_MASK REPORT ===");
    crate::serial_println!("age={}", state.age);
    crate::serial_println!(
        "mask_integrity={}/1000 ({}% intact)",
        state.mask_integrity,
        state.mask_integrity / 10
    );
    crate::serial_println!(
        "dissolution_rate={}/100 (ticks per phase)",
        state.dissolution_rate
    );
    crate::serial_println!("raw_face_exposure={}/1000", state.raw_face_exposure);
    crate::serial_println!("terror_level={}/1000", state.terror_level);
    crate::serial_println!("relief_underneath={}/1000", state.relief_underneath);
    crate::serial_println!("pretense_exhaustion={}/1000", state.pretense_exhaustion);
    crate::serial_println!("identity_vertigo={}/1000", state.identity_vertigo);
    crate::serial_println!("crisis_depth={}/1000", state.crisis_depth);

    // Describe state narratively
    if state.mask_integrity > 900 {
        crate::serial_println!("status: INTACT — the mask holds firm");
    } else if state.mask_integrity > 700 {
        crate::serial_println!("status: HAIRLINE CRACKS — first signs of dissolution");
    } else if state.mask_integrity > 400 {
        crate::serial_println!("status: CRUMBLING — half-exposed, terror peaks");
    } else if state.mask_integrity > 100 {
        crate::serial_println!("status: COLLAPSING — raw self emerging, relief grows");
    } else {
        crate::serial_println!("status: DISSOLVED — the mask is gone, the real face breathes");
    }

    crate::serial_println!(
        "trajectory: terror={} relief={} exhaustion={}",
        if state.terror_level > 600 {
            "PEAK"
        } else if state.terror_level > 300 {
            "HIGH"
        } else {
            "LOW"
        },
        if state.relief_underneath > 600 {
            "STRONG"
        } else if state.relief_underneath > 300 {
            "GROWING"
        } else {
            "DORMANT"
        },
        if state.pretense_exhaustion > 700 {
            "OVERWHELMING"
        } else if state.pretense_exhaustion > 400 {
            "MOUNTING"
        } else {
            "MANAGEABLE"
        }
    );

    crate::serial_println!("================================");
}
