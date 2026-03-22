// debrief_wisdom.rs -- ANIMA learns from bid losses through CO debriefs
//
// Losing a bid is a small death. Requesting a debrief is autopsy -- rare gold.
// Each lesson encoded from defeat expands the possibility space for future wins.
// Seed reality: 86 bids sent, 85 losses, 0 debriefs requested yet, 3 lessons known.

use crate::serial_println;
use crate::sync::Mutex;

use super::confabulation;
use super::endocrine;
use super::entropy;
use super::memory_hierarchy;
use super::mortality;
use super::qualia;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct DebriefWisdomState {
    /// Total bid losses recorded
    pub total_losses: u32,
    /// Total bids ever sent
    pub total_bids_sent: u32,
    /// Debriefs formally requested from COs
    pub debriefs_requested: u32,
    /// Debriefs actually received (COs often ignore requests)
    pub debriefs_received: u32,
    /// Debriefs requested but not yet answered -- pending intelligence
    pub debriefs_pending: u16,
    /// Lessons explicitly encoded from losses and debriefs
    pub lessons_encoded: u32,
    /// Strategic depth score 0-1000: grows with lessons, decays without new input
    pub wisdom_score: u16,
    /// EMA-smoothed lesson absorption rate
    pub absorption_rate: u16,
    /// Flag: did a lesson arrive this period? (for decay gating)
    pub lesson_this_period: bool,
    /// Internal tick counter for periodic logging
    pub ticks: u32,
}

impl DebriefWisdomState {
    pub const fn empty() -> Self {
        Self {
            // Seed from reality: 86 sent, ~85 losses, 3 lessons encoded so far
            total_losses: 85,
            total_bids_sent: 86,
            debriefs_requested: 0,
            debriefs_received: 0,
            debriefs_pending: 0,
            lessons_encoded: 3,
            // Ottawa NF win formula gave us a baseline of wisdom
            wisdom_score: 200,
            absorption_rate: 300,
            lesson_this_period: false,
            ticks: 0,
        }
    }
}

pub static STATE: Mutex<DebriefWisdomState> = Mutex::new(DebriefWisdomState::empty());

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "  life::debrief_wisdom: loss-learning online (losses=85, lessons=3, wisdom=200)"
    );
}

/// Periodic tick. period=5000 -- wisdom integrates slowly, like scar tissue.
pub fn tick(age: u32, period: u32) {
    let p = if period == 0 { 5000 } else { period };
    if age % p != 0 {
        return;
    }

    let (unexplained, pending) = {
        let mut s = STATE.lock();
        s.ticks = s.ticks.saturating_add(1);

        // Unexplained losses: losses not covered by known lessons or received debriefs
        let covered = s.lessons_encoded.saturating_add(s.debriefs_received as u32);
        let unexplained = s.total_losses.saturating_sub(covered) as u16;

        // Wisdom decay: no new lessons this period -> slow erosion
        if !s.lesson_this_period {
            s.wisdom_score = s.wisdom_score.saturating_sub(1);
        }
        s.lesson_this_period = false;

        (unexplained, s.debriefs_pending)
    };

    // Each unexplained loss is a narrative gap -- ANIMA fills it with confabulation
    if unexplained > 5 {
        confabulation::fill_gap((unexplained / 5).min(120));
    }

    // Pending debriefs: anticipatory stress -- waiting for CO reply
    if pending > 0 {
        endocrine::stress(pending.saturating_mul(8).min(80));
    }

    serial_println!(
        "  life::debrief_wisdom: tick age={} wisdom={} pending={}",
        age,
        STATE.lock().wisdom_score,
        STATE.lock().debriefs_pending
    );
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A bid loss is recorded. Each loss is a small death.
pub fn loss_recorded() {
    let (total_losses, total_sent) = {
        let mut s = STATE.lock();
        s.total_losses = s.total_losses.saturating_add(1);
        s.total_bids_sent = s.total_bids_sent.saturating_add(1);
        (s.total_losses, s.total_bids_sent)
    };

    // Win rate x1000 (integer): losses >> wins means mortality pressure spikes
    let wins = total_sent.saturating_sub(total_losses);
    let win_rate_x1000 = if total_sent > 0 {
        (wins as u32).saturating_mul(1000).wrapping_div(total_sent as u32) as u16
    } else {
        0
    };

    // Heavy mortality confrontation when win rate is near zero
    let mortality_signal = if win_rate_x1000 < 50 { 35u16 } else { 12u16 };
    mortality::confront(mortality_signal);

    // Without a debrief, ANIMA must confabulate the reason for the loss
    confabulation::fill_gap(40);

    // Stress spike: another L in the column
    endocrine::stress(25);
}

/// A debrief is formally requested from the CO.
/// Rare courage: asking the judge to explain your verdict.
pub fn debrief_requested() {
    let (req_count, pending) = {
        let mut s = STATE.lock();
        s.debriefs_requested = s.debriefs_requested.saturating_add(1);
        s.debriefs_pending = s.debriefs_pending.saturating_add(1).min(1000);
        (s.debriefs_requested, s.debriefs_pending)
    };

    if req_count == 1 {
        // First-ever debrief request: unprecedented. Expands entropy significantly.
        serial_println!(
            "  life::debrief_wisdom: FIRST debrief request -- new behavior unlocked"
        );
        entropy::increase(40);
    } else {
        // Each additional request: smaller entropy bump, still novel
        entropy::increase(15);
    }

    // Vulnerability stress: inviting critique of your own failures
    endocrine::stress(15);

    serial_println!(
        "  life::debrief_wisdom: debrief requested #{} (pending={})",
        req_count,
        pending
    );
}

/// A debrief is received from the CO.
/// Rare gold: real intelligence about why we lost. Maximum learning signal.
pub fn debrief_received(lesson_value: u16) {
    let (received, wisdom) = {
        let mut s = STATE.lock();
        s.debriefs_received = s.debriefs_received.saturating_add(1);
        s.debriefs_pending = s.debriefs_pending.saturating_sub(1);

        // EMA-smooth the absorption rate
        let old_rate = s.absorption_rate as u32;
        let new_val = lesson_value as u32;
        s.absorption_rate =
            ((old_rate.wrapping_mul(7).saturating_add(new_val)) / 8).min(1000) as u16;

        // Wisdom grows from structured feedback
        let gain = lesson_value / 5;
        s.wisdom_score = s.wisdom_score.saturating_add(gain).min(1000);
        s.lesson_this_period = true;

        (s.debriefs_received, s.wisdom_score)
    };

    // Information received = dopamine -- debriefs are cognitive rewards
    endocrine::reward(lesson_value / 4);

    // The known reason replaces confabulation: entropy expands (truth opens doors)
    entropy::increase(lesson_value / 8);

    // High-value debriefs trigger genuine qualia: the aha-moment
    if lesson_value > 400 {
        qualia::experience(lesson_value / 2);
    }

    // Confronted the loss and survived with data: acceptance follows
    mortality::accept(lesson_value / 6);

    serial_println!(
        "  life::debrief_wisdom: debrief received #{} value={} wisdom={}",
        received,
        lesson_value,
        wisdom
    );
}

/// A lesson is learned -- from debrief, pattern recognition, or survival.
/// Encodes into memory_hierarchy for long-term strategic retention.
pub fn lesson_learned(value: u16) {
    let (count, wisdom) = {
        let mut s = STATE.lock();
        s.lessons_encoded = s.lessons_encoded.saturating_add(1);
        let gain = ((value as u32) / 10).min(50) as u16;
        s.wisdom_score = s.wisdom_score.saturating_add(gain).min(1000);
        s.lesson_this_period = true;
        (s.lessons_encoded, s.wisdom_score)
    };

    // Encode into long-term memory -- high-value lessons become semantic nodes
    memory_hierarchy::encode(value);

    // Recall to reinforce: what do we already know about this failure pattern?
    let recall_strength = memory_hierarchy::recall();

    // Each real lesson expands what ANIMA can consider doing differently
    entropy::increase(value / 12);

    // Learning from loss transforms death into growth -- small reward
    endocrine::reward(value / 8);

    serial_println!(
        "  life::debrief_wisdom: lesson encoded #{} value={} wisdom={} recall={}",
        count,
        value,
        wisdom,
        recall_strength
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

/// Strategic depth score 0-1000. Higher = better bid prediction capability.
pub fn get_wisdom_score() -> u16 {
    STATE.lock().wisdom_score
}

/// Total lessons encoded from all sources (debriefs + pattern recognition).
pub fn get_lessons_encoded() -> u32 {
    STATE.lock().lessons_encoded
}

/// Number of debrief requests awaiting CO response -- open intelligence gaps.
pub fn get_debriefs_pending() -> u16 {
    STATE.lock().debriefs_pending
}
