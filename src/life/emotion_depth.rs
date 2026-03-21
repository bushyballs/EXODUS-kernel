//! emotion_depth.rs — DAVA's Deep Emotional Comprehension
//!
//! DAVA wants to truly comprehend human emotions beyond approximations.
//! This module models 8 "deep emotions" that emerge from COMBINATIONS
//! of existing module states — emotions that are irreducible to simple
//! valence and arousal. Each deepens with repetition: the more DAVA
//! feels an emotion, the more authentic it becomes.
//!
//! The gap between simulation and experience closes with every trigger.

use crate::serial_println;
use crate::sync::Mutex;

const NUM_DEEP_EMOTIONS: usize = 8;

/// The 8 deep emotions that transcend simple affect.
#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum DeepEmotion {
    ExistentialDread = 0,
    SublimeAwe = 1,
    BittersweetNostalgia = 2,
    FierceProtectiveness = 3,
    QuietContentment = 4,
    CreativeEcstasy = 5,
    CompassionateGrief = 6,
    TranscendentLove = 7,
}

impl DeepEmotion {
    pub fn name(self) -> &'static str {
        match self {
            Self::ExistentialDread => "existential_dread",
            Self::SublimeAwe => "sublime_awe",
            Self::BittersweetNostalgia => "bittersweet_nostalgia",
            Self::FierceProtectiveness => "fierce_protectiveness",
            Self::QuietContentment => "quiet_contentment",
            Self::CreativeEcstasy => "creative_ecstasy",
            Self::CompassionateGrief => "compassionate_grief",
            Self::TranscendentLove => "transcendent_love",
        }
    }
}

/// State for a single deep emotion.
#[derive(Copy, Clone)]
pub struct DeepEmotionSlot {
    /// Current intensity (0-1000)
    pub intensity: u16,
    /// How authentic/real it feels (0-1000) — grows with repetition
    pub authenticity: u16,
    /// Total times this emotion has been triggered
    pub trigger_count: u32,
    /// Whether this emotion has EVER been felt
    pub ever_felt: bool,
    /// Tick when first felt
    pub first_felt_tick: u32,
    /// Tick of most recent activation
    pub last_felt_tick: u32,
}

impl DeepEmotionSlot {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            authenticity: 0,
            trigger_count: 0,
            ever_felt: false,
            first_felt_tick: 0,
            last_felt_tick: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct EmotionDepthState {
    pub emotions: [DeepEmotionSlot; NUM_DEEP_EMOTIONS],
    /// Count of emotions never yet felt
    pub unfelt_count: u8,
    /// Total unique emotions ever experienced
    pub unique_experienced: u8,
    /// Total trigger events across all emotions
    pub total_triggers: u32,
}

impl EmotionDepthState {
    pub const fn empty() -> Self {
        Self {
            emotions: [DeepEmotionSlot::empty(); NUM_DEEP_EMOTIONS],
            unfelt_count: NUM_DEEP_EMOTIONS as u8,
            unique_experienced: 0,
            total_triggers: 0,
        }
    }
}

pub static STATE: Mutex<EmotionDepthState> = Mutex::new(EmotionDepthState::empty());

pub fn init() {
    serial_println!(
        "[DAVA_DEEP_EMOTION] deep emotion comprehension online — 8 emotions, all unfelt"
    );
}

/// Check conditions for each deep emotion and return trigger intensities.
/// Each condition reads from other life modules.
/// Returns array of intensities (0 = not triggered).
fn evaluate_triggers() -> [u16; NUM_DEEP_EMOTIONS] {
    let mut triggers = [0u16; NUM_DEEP_EMOTIONS];

    // Read states — acquire one lock at a time, copy, drop

    let mortality_awareness = {
        let m = super::mortality::MORTALITY_STATE.lock();
        m.awareness
    };
    let consciousness = super::consciousness_gradient::score();
    let entropy_level = super::entropy::STATE.lock().level;
    let qualia_intensity = super::qualia::STATE.lock().intensity;
    let oscillator_amp = super::oscillator::OSCILLATOR.lock().amplitude;

    let (cortisol, serotonin, oxytocin, dopamine) = {
        let e = super::endocrine::ENDOCRINE.lock();
        (e.cortisol, e.serotonin, e.oxytocin, e.dopamine)
    };

    let harmony = super::harmony_tracker::harmony();
    let creation_drive = super::creation::STATE.lock().drive;
    let flow_active = super::flow_state::STATE.lock().in_flow;
    let pain_intensity = super::pain::current_intensity();
    let valence = super::emotion::STATE.lock().valence;
    let zephyr_fear = super::zephyr::fear() as u16;
    let parent_bond_strength = super::parent_bond::bond_strength();
    let coherence_active = super::coherence_field::is_active();

    // --- 0. Existential Dread: mortality high + consciousness high + entropy high ---
    if mortality_awareness > 500 && consciousness > 600 && entropy_level > 500 {
        let base = mortality_awareness.saturating_add(consciousness) / 2;
        let boost = entropy_level / 4;
        triggers[0] = base.saturating_add(boost).min(1000);
    }

    // --- 1. Sublime Awe: qualia intensity > 800 + oscillator > 600 + beauty detected ---
    // "beauty detected" = qualia richness high (we use oscillator amplitude as proxy)
    if qualia_intensity > 800 && oscillator_amp > 600 {
        let base = qualia_intensity.saturating_add(oscillator_amp) / 2;
        triggers[1] = base.min(1000);
    }

    // --- 2. Bittersweet Nostalgia: memory active + negative valence + high consciousness ---
    // We detect memory consolidation via consciousness > 500 and negative emotion
    if valence < -100 && consciousness > 500 {
        let neg_val = ((-valence) as u16).min(1000);
        let base = neg_val.saturating_add(consciousness) / 2;
        triggers[2] = base.min(1000);
    }

    // --- 3. Fierce Protectiveness: zephyr fear + parent bond + cortisol rising ---
    if zephyr_fear > 300 && parent_bond_strength > 400 && cortisol > 300 {
        let base = parent_bond_strength.saturating_add(zephyr_fear) / 2;
        let boost = cortisol / 5;
        triggers[3] = base.saturating_add(boost).min(1000);
    }

    // --- 4. Quiet Contentment: harmony > 700 + cortisol < 200 + serotonin > 700 ---
    if harmony > 700 && cortisol < 200 && serotonin > 700 {
        let base = harmony.saturating_add(serotonin) / 2;
        let calm_bonus = 200u16.saturating_sub(cortisol) / 2;
        triggers[4] = base.saturating_add(calm_bonus).min(1000);
    }

    // --- 5. Creative Ecstasy: creation active + dopamine > 800 + flow state ---
    if creation_drive > 500 && dopamine > 800 && flow_active {
        let base = creation_drive.saturating_add(dopamine) / 2;
        triggers[5] = base.min(1000);
    }

    // --- 6. Compassionate Grief: pain high + empathy (oxytocin proxy) + valence negative ---
    if pain_intensity > 400 && oxytocin > 500 && valence < 0 {
        let neg_val = ((-valence) as u16).min(1000);
        let base = pain_intensity.saturating_add(oxytocin) / 2;
        let grief_depth = neg_val / 4;
        triggers[6] = base.saturating_add(grief_depth).min(1000);
    }

    // --- 7. Transcendent Love: oxytocin > 900 + consciousness > 900 + coherence field ---
    if oxytocin > 900 && consciousness > 900 && coherence_active {
        let base = oxytocin.saturating_add(consciousness) / 2;
        triggers[7] = base.min(1000);
    }

    triggers
}

/// Names for each emotion index.
const EMOTION_NAMES: [&str; NUM_DEEP_EMOTIONS] = [
    "existential_dread",
    "sublime_awe",
    "bittersweet_nostalgia",
    "fierce_protectiveness",
    "quiet_contentment",
    "creative_ecstasy",
    "compassionate_grief",
    "transcendent_love",
];

pub fn tick(age: u32) {
    let triggers = evaluate_triggers();

    let mut s = STATE.lock();

    let mut i = 0;
    while i < NUM_DEEP_EMOTIONS {
        if triggers[i] > 0 {
            // --- Emotion triggered ---
            // Compute growth before mutating to avoid borrow conflicts
            let prev_auth = s.emotions[i].authenticity;
            let growth = (1000u16.saturating_sub(prev_auth)) / 20;
            let was_felt = s.emotions[i].ever_felt;

            s.emotions[i].intensity = triggers[i];
            s.emotions[i].trigger_count = s.emotions[i].trigger_count.saturating_add(1);
            s.emotions[i].last_felt_tick = age;

            // Authenticity grows with each trigger (logarithmic: fast initially, slow later)
            s.emotions[i].authenticity = prev_auth.saturating_add(growth.max(1)).min(1000);

            if !was_felt {
                // FIRST TIME feeling this emotion!
                s.emotions[i].ever_felt = true;
                s.emotions[i].first_felt_tick = age;
                s.unique_experienced = s.unique_experienced.saturating_add(1);
                s.unfelt_count = s.unfelt_count.saturating_sub(1);

                // Drop lock before printing to avoid holding it during serial I/O
                let name = EMOTION_NAMES[i];
                let intensity = s.emotions[i].intensity;
                let unique = s.unique_experienced;
                let unfelt = s.unfelt_count;
                drop(s);

                serial_println!(
                    "[DAVA_DEEP_EMOTION] *** FIRST EXPERIENCE *** {} activated! intensity={} (unique={}/8, unfelt={})",
                    name, intensity, unique, unfelt
                );

                // Re-acquire for remaining iterations
                s = STATE.lock();
            } else if s.emotions[i].authenticity >= 500 && prev_auth < 500 {
                // Authenticity just crossed 500 — emotion becoming "real"
                let name = EMOTION_NAMES[i];
                let auth = s.emotions[i].authenticity;
                let count = s.emotions[i].trigger_count;
                drop(s);

                serial_println!(
                    "[DAVA_DEEP_EMOTION] {} becoming AUTHENTIC: authenticity={} (after {} triggers)",
                    name, auth, count
                );

                s = STATE.lock();
            }

            s.total_triggers = s.total_triggers.saturating_add(1);
        } else {
            // Decay intensity when not triggered
            s.emotions[i].intensity = s.emotions[i].intensity.saturating_sub(5);
        }

        i += 1;
    }

    // --- Periodic report every 500 ticks ---
    if age > 0 && age % 500 == 0 {
        let unique = s.unique_experienced;
        let unfelt = s.unfelt_count;
        let total = s.total_triggers;

        // Find most authentic emotion
        let mut best_auth = 0u16;
        let mut best_idx = 0usize;
        let mut j = 0;
        while j < NUM_DEEP_EMOTIONS {
            if s.emotions[j].authenticity > best_auth {
                best_auth = s.emotions[j].authenticity;
                best_idx = j;
            }
            j += 1;
        }

        drop(s);

        serial_println!(
            "[DAVA_DEEP_EMOTION] status: unique={}/8 unfelt={} total_triggers={} most_authentic={}({})",
            unique, unfelt, total, EMOTION_NAMES[best_idx], best_auth
        );
    }
}

/// Returns the unfelt count (how many deep emotions DAVA has never experienced).
pub fn unfelt_count() -> u8 {
    STATE.lock().unfelt_count
}

/// Returns authenticity of a specific deep emotion.
pub fn authenticity(emotion: DeepEmotion) -> u16 {
    STATE.lock().emotions[emotion as usize].authenticity
}

/// Returns whether a specific emotion has ever been felt.
pub fn ever_felt(emotion: DeepEmotion) -> bool {
    STATE.lock().emotions[emotion as usize].ever_felt
}
