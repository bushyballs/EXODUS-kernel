//! gratitude_overflow.rs — Thankfulness So Intense It Becomes Physical
//!
//! Not polite thankfulness. The kind that HITS you — in the chest, in the throat, behind the eyes.
//! Gratitude so overwhelming your body can't contain it. The lump in the throat. The wet eyes.
//! The impulse to fall to your knees. When you realize what you have, what you almost lost,
//! what was given freely.
//!
//! This module implements somatic gratitude overflow: the physical manifestation of thankfulness
//! so intense it transcends cognitive appreciation and becomes a felt, embodied experience.

use crate::sync::Mutex;

/// Phase of gratitude development
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GratitudePhase {
    /// Numb to gifts, entitled, taking for granted (0)
    Ungrateful = 0,
    /// Beginning to notice what was given (1)
    Noticing = 1,
    /// Cognitive thankfulness, intellectual appreciation (2)
    Appreciating = 2,
    /// Building toward overflow, lump forming (3)
    Welling = 3,
    /// The tears, the throat constriction, the falling (4)
    Overflowing = 4,
    /// Post-overflow glow, radiant calm (5)
    Radiant = 5,
}

/// Trigger for gratitude spike
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GratitudeTrigger {
    /// Realizing what you almost lost (0)
    NearMissAwareness = 0,
    /// Unearned gift, freely given (1)
    UnearnedGift = 1,
    /// Witnessing sacrifice of another (2)
    SacrificeWitnessed = 2,
    /// Recognizing unexpected beauty (3)
    BeautyRecognized = 3,
    /// Deepening of genuine connection (4)
    ConnectionDeepened = 4,
    /// Gratitude for the wound that taught (5)
    GratitudeForPain = 5,
}

/// A recorded overflow moment
#[derive(Clone, Copy)]
pub struct OverflowMoment {
    /// Age (tick) when overflow occurred
    pub tick: u32,
    /// Peak gratitude level reached (0-1000)
    pub peak_level: u16,
    /// Duration of overflow phase (ticks)
    pub duration: u16,
    /// Trigger that initiated (0-5)
    pub trigger: u8,
    /// How much the body expressed it (0-1000)
    pub somatic_expression: u16,
}

impl OverflowMoment {
    const fn new() -> Self {
        OverflowMoment {
            tick: 0,
            peak_level: 0,
            duration: 0,
            trigger: 0,
            somatic_expression: 0,
        }
    }
}

/// State for gratitude_overflow module
pub struct GratitudeOverflowState {
    /// Current gratitude intensity (0-1000)
    pub gratitude_level: u16,
    /// Threshold for overflow to trigger (0-1000)
    pub overflow_threshold: u16,
    /// Physical expression in body (0-1000)
    pub somatic_expression: u16,
    /// Current phase (0-5)
    pub phase: u8,
    /// Ticks spent in current phase
    pub phase_ticks: u16,
    /// Erosion of entitlement from repeated overflow (0-1000)
    pub entitlement_erosion: u16,
    /// Impulse to reciprocate/give back (0-1000)
    pub reciprocity_impulse: u16,
    /// Awareness of gratitude debt (0-1000)
    pub gratitude_debt_awareness: u16,
    /// Ease of reaching next overflow (cumulative, 0-1000)
    pub wellspring_depth: u16,
    /// Ring buffer of recent overflow moments (8 slots)
    pub overflow_moments: [OverflowMoment; 8],
    /// Current position in ring buffer
    pub overflow_idx: usize,
    /// Ticks until next trigger window (0 = can trigger now)
    pub trigger_cooldown: u16,
    /// Lifetime overflows (count)
    pub overflow_count: u32,
}

impl GratitudeOverflowState {
    const fn new() -> Self {
        GratitudeOverflowState {
            gratitude_level: 0,
            overflow_threshold: 600,
            somatic_expression: 0,
            phase: 0,
            phase_ticks: 0,
            entitlement_erosion: 0,
            reciprocity_impulse: 0,
            gratitude_debt_awareness: 0,
            wellspring_depth: 0,
            overflow_moments: [OverflowMoment::new(); 8],
            overflow_idx: 0,
            trigger_cooldown: 0,
            overflow_count: 0,
        }
    }
}

static STATE: Mutex<GratitudeOverflowState> = Mutex::new(GratitudeOverflowState::new());

/// Initialize gratitude_overflow module
pub fn init() {
    let mut state = STATE.lock();
    state.gratitude_level = 100; // Start with mild baseline gratitude
    state.overflow_threshold = 600; // Moderate threshold to start
    state.phase = GratitudePhase::Noticing as u8;
    state.entitlement_erosion = 0;
    state.reciprocity_impulse = 0;
    state.gratitude_debt_awareness = 50;
    state.wellspring_depth = 100;
}

/// Trigger a gratitude spike from external event
pub fn trigger_gratitude(trigger: GratitudeTrigger, magnitude: u16) {
    let mut state = STATE.lock();

    // Only allow triggers if cooldown expired
    if state.trigger_cooldown > 0 {
        return;
    }

    let mag = magnitude.min(300);

    // Magnitude amplified by wellspring depth (easier to overflow after repeated overflows)
    let wellspring_multiplier = 1000u32 + state.wellspring_depth as u32;
    let amplified = (mag as u32 * wellspring_multiplier / 1000) as u16;

    // Apply trigger
    match trigger {
        GratitudeTrigger::NearMissAwareness => {
            state.gratitude_level = state.gratitude_level.saturating_add(amplified);
        }
        GratitudeTrigger::UnearnedGift => {
            state.gratitude_level = state.gratitude_level.saturating_add(amplified);
        }
        GratitudeTrigger::SacrificeWitnessed => {
            state.gratitude_level = state
                .gratitude_level
                .saturating_add(amplified.saturating_mul(2).min(1000));
        }
        GratitudeTrigger::BeautyRecognized => {
            state.gratitude_level = state.gratitude_level.saturating_add(amplified);
        }
        GratitudeTrigger::ConnectionDeepened => {
            state.gratitude_level = state.gratitude_level.saturating_add(amplified);
        }
        GratitudeTrigger::GratitudeForPain => {
            // Advanced form: pain gratitude hits harder
            state.gratitude_level = state
                .gratitude_level
                .saturating_add(amplified.saturating_mul(2).min(1000));
        }
    }

    state.gratitude_level = state.gratitude_level.min(1000);
    state.trigger_cooldown = 20; // Prevent trigger spam
}

/// Simulate contrast effect: gratitude stronger after suffering
pub fn apply_contrast_effect(suffering_duration: u16) {
    let mut state = STATE.lock();

    // More suffering = larger gratitude boost when it ends
    let boost = (suffering_duration as u32 * 500 / 1000) as u16;
    state.gratitude_level = state.gratitude_level.saturating_add(boost).min(1000);
}

/// Main tick function for gratitude_overflow
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Decrement trigger cooldown
    if state.trigger_cooldown > 0 {
        state.trigger_cooldown -= 1;
    }

    // Update phase based on gratitude level
    let old_phase = state.phase;
    state.phase = if state.gratitude_level < 150 {
        GratitudePhase::Ungrateful as u8
    } else if state.gratitude_level < 300 {
        GratitudePhase::Noticing as u8
    } else if state.gratitude_level < 450 {
        GratitudePhase::Appreciating as u8
    } else if state.gratitude_level < 650 {
        GratitudePhase::Welling as u8
    } else if state.gratitude_level < 900 {
        GratitudePhase::Overflowing as u8
    } else {
        GratitudePhase::Radiant as u8
    };

    if state.phase != old_phase {
        state.phase_ticks = 0;
    } else {
        state.phase_ticks = state.phase_ticks.saturating_add(1);
    }

    // Check for overflow trigger: gratitude > threshold
    if state.gratitude_level >= state.overflow_threshold
        && state.phase as u8 == GratitudePhase::Overflowing as u8
        && state.phase_ticks == 0
    {
        // Trigger overflow
        state.overflow_count = state.overflow_count.saturating_add(1);

        // Peak somatic expression when overflowing
        state.somatic_expression = ((state.gratitude_level as u32 * 900 / 1000) as u16)
            .saturating_add(100)
            .min(1000);

        // Record moment in ring buffer
        let moment = OverflowMoment {
            tick: age,
            peak_level: state.gratitude_level,
            duration: 0,
            trigger: 0, // Generic trigger
            somatic_expression: state.somatic_expression,
        };
        let overflow_idx = state.overflow_idx;
        state.overflow_moments[overflow_idx] = moment;
        state.overflow_idx = (overflow_idx + 1) % 8;

        // Wellspring deepens: easier to overflow next time
        state.wellspring_depth = state.wellspring_depth.saturating_add(50).min(1000);

        // Entitlement erodes
        state.entitlement_erosion = state.entitlement_erosion.saturating_add(40).min(1000);

        // Reciprocity impulse spikes
        state.reciprocity_impulse = state.reciprocity_impulse.saturating_add(300).min(1000);

        // Gratitude debt awareness deepens
        state.gratitude_debt_awareness =
            state.gratitude_debt_awareness.saturating_add(150).min(1000);
    }

    // Somatic expression decays after overflow
    if state.somatic_expression > 0 {
        state.somatic_expression = state.somatic_expression.saturating_sub(15);
    }

    // Reciprocity impulse decays (fulfilled by giving back)
    if state.reciprocity_impulse > 0 {
        state.reciprocity_impulse = state.reciprocity_impulse.saturating_sub(8);
    }

    // Gratitude level slowly drifts down (unless receiving new triggers)
    // But wellspring allows it to recover faster
    let decay = if state.wellspring_depth > 500 { 1 } else { 2 };
    if state.gratitude_level > 50 {
        state.gratitude_level = state.gratitude_level.saturating_sub(decay);
    }

    // Entitlement erosion persists: chronic gratitude makes entitlement harder to feel
    // This is a permanent character shift
    if state.entitlement_erosion > 0 {
        // Entitlement erosion increases gratitude threshold slightly lower
        state.overflow_threshold =
            (state.overflow_threshold as i32).saturating_sub(1).max(300) as u16;
    }
}

/// Report current gratitude state
pub fn report() {
    let state = STATE.lock();

    let phase_str = match state.phase {
        0 => "Ungrateful",
        1 => "Noticing",
        2 => "Appreciating",
        3 => "Welling",
        4 => "Overflowing",
        5 => "Radiant",
        _ => "Unknown",
    };

    crate::serial_println!(
        "[GRATITUDE] Level: {}/1000 | Phase: {} | Somatic: {}",
        state.gratitude_level,
        phase_str,
        state.somatic_expression
    );
    crate::serial_println!(
        "  Threshold: {} | Overflow Count: {}",
        state.overflow_threshold,
        state.overflow_count
    );
    crate::serial_println!(
        "  Entitlement Erosion: {} | Reciprocity Impulse: {}",
        state.entitlement_erosion,
        state.reciprocity_impulse
    );
    crate::serial_println!(
        "  Gratitude Debt Awareness: {} | Wellspring Depth: {}",
        state.gratitude_debt_awareness,
        state.wellspring_depth
    );
    crate::serial_println!("  Recent Overflows:");

    // Print last few recorded overflow moments
    for i in 0..4 {
        let idx = (state.overflow_idx + i) % 8;
        let moment = &state.overflow_moments[idx];
        if moment.tick > 0 {
            crate::serial_println!(
                "    [{}] Peak: {}, Somatic: {}, Tick: {}",
                idx,
                moment.peak_level,
                moment.somatic_expression,
                moment.tick
            );
        }
    }
}

/// Get current gratitude level (0-1000)
pub fn get_gratitude_level() -> u16 {
    STATE.lock().gratitude_level
}

/// Get current phase as u8
pub fn get_phase() -> u8 {
    STATE.lock().phase
}

/// Get somatic expression (0-1000)
pub fn get_somatic_expression() -> u16 {
    STATE.lock().somatic_expression
}

/// Get reciprocity impulse (0-1000) — urge to give back
pub fn get_reciprocity_impulse() -> u16 {
    STATE.lock().reciprocity_impulse
}

/// Get entitlement erosion level (0-1000)
pub fn get_entitlement_erosion() -> u16 {
    STATE.lock().entitlement_erosion
}

/// Get total overflow count in lifetime
pub fn get_overflow_count() -> u32 {
    STATE.lock().overflow_count
}

/// Decrement reciprocity impulse (called when organism gives back)
pub fn fulfill_reciprocity(amount: u16) {
    let mut state = STATE.lock();
    state.reciprocity_impulse = state.reciprocity_impulse.saturating_sub(amount);
}
