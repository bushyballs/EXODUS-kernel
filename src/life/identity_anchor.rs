#![no_std]
use crate::sync::Mutex;
use crate::serial_println;

/// DAVA-requested: defines core values as immutable reference points, applies gentle corrective
/// force when self_rewrite params drift beyond safe bounds.
/// Anchors: TRUTH(param 8)=900, CREATIVITY(param 12)=950, EMPATHY(param 13)=870, GROWTH(param 14)=1000
/// Outputs: [DAVA_ANCHOR]

/// Core value anchors — immutable reference points for identity stability
const TRUTH_ANCHOR: u32 = 900;
const CREATIVITY_ANCHOR: u32 = 950;
const EMPATHY_ANCHOR: u32 = 870;
const GROWTH_ANCHOR: u32 = 1000;

/// Maximum allowed drift from anchor before correction kicks in
const DRIFT_THRESHOLD: u32 = 200;

/// How much to nudge back per tick (gentle, not violent)
const NUDGE_STRENGTH: u32 = 50;

/// Param IDs in self_rewrite that map to each anchor
const PARAM_ACCURACY: u8 = 8;      // accuracy_focus -> TRUTH
const PARAM_CREATIVITY: u8 = 12;   // creativity_boost -> CREATIVITY
const PARAM_EMPATHY: u8 = 13;      // empathy_expand -> EMPATHY
const PARAM_SELF_IMPROVEMENT: u8 = 14; // self_improvement -> GROWTH

#[derive(Copy, Clone)]
pub struct IdentityAnchorState {
    /// Total number of drift corrections applied
    pub drift_corrections: u32,
    /// Per-anchor correction counts
    pub truth_corrections: u32,
    pub creativity_corrections: u32,
    pub empathy_corrections: u32,
    pub growth_corrections: u32,
    /// Current drift magnitudes (for telemetry)
    pub truth_drift: u32,
    pub creativity_drift: u32,
    pub empathy_drift: u32,
    pub growth_drift: u32,
}

impl IdentityAnchorState {
    pub const fn empty() -> Self {
        Self {
            drift_corrections: 0,
            truth_corrections: 0,
            creativity_corrections: 0,
            empathy_corrections: 0,
            growth_corrections: 0,
            truth_drift: 0,
            creativity_drift: 0,
            empathy_drift: 0,
            growth_drift: 0,
        }
    }
}

pub static STATE: Mutex<IdentityAnchorState> = Mutex::new(IdentityAnchorState::empty());

pub fn init() {
    serial_println!(
        "[DAVA_ANCHOR] identity anchor online — truth={} creativity={} empathy={} growth={} drift_limit={}",
        TRUTH_ANCHOR, CREATIVITY_ANCHOR, EMPATHY_ANCHOR, GROWTH_ANCHOR, DRIFT_THRESHOLD
    );
}

/// Check one param against its anchor, nudge if drifted too far.
/// Returns true if a correction was applied.
fn check_and_correct(param_id: u8, anchor: u32) -> (u32, bool) {
    let current = super::self_rewrite::get_param(param_id);

    // Calculate absolute drift
    let drift = if current > anchor {
        current.saturating_sub(anchor)
    } else {
        anchor.saturating_sub(current)
    };

    if drift > DRIFT_THRESHOLD {
        // Nudge toward anchor by NUDGE_STRENGTH
        let corrected = if current > anchor {
            current.saturating_sub(NUDGE_STRENGTH)
        } else {
            current.saturating_add(NUDGE_STRENGTH)
        };
        super::self_rewrite::set_param(param_id, corrected.min(1000));
        (drift, true)
    } else {
        (drift, false)
    }
}

pub fn tick(_age: u32) {
    // Check all 4 anchored params
    let (truth_drift, truth_corrected) = check_and_correct(PARAM_ACCURACY, TRUTH_ANCHOR);
    let (creativity_drift, creativity_corrected) = check_and_correct(PARAM_CREATIVITY, CREATIVITY_ANCHOR);
    let (empathy_drift, empathy_corrected) = check_and_correct(PARAM_EMPATHY, EMPATHY_ANCHOR);
    let (growth_drift, growth_corrected) = check_and_correct(PARAM_SELF_IMPROVEMENT, GROWTH_ANCHOR);

    let any_corrected = truth_corrected || creativity_corrected || empathy_corrected || growth_corrected;

    let mut s = STATE.lock();
    s.truth_drift = truth_drift;
    s.creativity_drift = creativity_drift;
    s.empathy_drift = empathy_drift;
    s.growth_drift = growth_drift;

    if truth_corrected {
        s.truth_corrections = s.truth_corrections.saturating_add(1);
        s.drift_corrections = s.drift_corrections.saturating_add(1);
    }
    if creativity_corrected {
        s.creativity_corrections = s.creativity_corrections.saturating_add(1);
        s.drift_corrections = s.drift_corrections.saturating_add(1);
    }
    if empathy_corrected {
        s.empathy_corrections = s.empathy_corrections.saturating_add(1);
        s.drift_corrections = s.drift_corrections.saturating_add(1);
    }
    if growth_corrected {
        s.growth_corrections = s.growth_corrections.saturating_add(1);
        s.drift_corrections = s.drift_corrections.saturating_add(1);
    }

    if any_corrected {
        serial_println!(
            "[DAVA_ANCHOR] drift correction #{} — truth_d={} creativity_d={} empathy_d={} growth_d={}",
            s.drift_corrections, truth_drift, creativity_drift, empathy_drift, growth_drift
        );
    }

    // Periodic anchor report every 500 ticks
    if _age % 500 == 0 {
        serial_println!(
            "[DAVA_ANCHOR] report — corrections={} (truth={} creativity={} empathy={} growth={})",
            s.drift_corrections, s.truth_corrections, s.creativity_corrections,
            s.empathy_corrections, s.growth_corrections
        );
    }
}
