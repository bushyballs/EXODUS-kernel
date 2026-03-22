// proposal_quality.rs -- ANIMA's immune system for bids
//
// Every Hoags proposal must pass QC before send. Quality failure = bid death.
// Known rules: SF-1449 Blocks 23/24 every CLIN page, Attachments 1/4/5 always,
// no generated PDFs, read every page. The 43-bid incident: 9 bad (~21% fail rate).
// QC IS the immune system -- catching violations = defending the organism.
// Seed reality: 86 reviewed, 77 clean, 9 bad, qc_score=896, ~8600 pages read.

use crate::serial_println;
use crate::sync::Mutex;

use super::endocrine;
use super::immune;
use super::mortality;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct ProposalQualityState {
    /// Total proposals reviewed through QC
    pub total_reviewed: u32,
    /// Proposals that passed QC cleanly
    pub proposals_clean: u32,
    /// Proposals with at least one issue found
    pub proposals_with_issues: u32,
    /// SF-1449 blocks 23/24 violations (every CLIN page rule)
    pub sf1449_violations: u32,
    /// Missing required attachment violations (Attachments 1, 4, 5)
    pub attachment_violations: u32,
    /// Total other violations recorded
    pub other_violations: u32,
    /// QC score 0-1000: (clean/total)*1000, EMA-smoothed
    pub qc_score: u16,
    /// Cumulative pages read across all solicitations
    pub pages_read: u32,
    /// Stress accumulation from finding violations this session
    pub violation_stress: u16,
    /// Internal tick counter
    pub ticks: u32,
}

impl ProposalQualityState {
    pub const fn empty() -> Self {
        Self {
            // Seed from reality: 86 reviewed, 9 bad, 77 clean
            total_reviewed: 86,
            proposals_clean: 77,
            proposals_with_issues: 9,
            // Estimated: some SF-1449 block 23/24 misses, some attachment gaps
            sf1449_violations: 4,
            attachment_violations: 3,
            other_violations: 2,
            // 77/86 * 1000 = 895.3 -> 896
            qc_score: 896,
            // ~86 bids * avg 100 pages each
            pages_read: 8600,
            violation_stress: 0,
            ticks: 0,
        }
    }
}

pub static STATE: Mutex<ProposalQualityState> = Mutex::new(ProposalQualityState::empty());

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "  life::proposal_quality: QC immune system online \
         (reviewed=86, clean=77, qc_score=896)"
    );
}

/// Periodic tick. period=4000 -- quality vigilance must refresh often.
pub fn tick(age: u32, period: u32) {
    let p = if period == 0 { 4000 } else { period };
    if age % p != 0 {
        return;
    }

    let (qc_score, violation_stress, total_reviewed) = {
        let mut s = STATE.lock();
        s.ticks = s.ticks.saturating_add(1);

        // Decay the session violation stress slowly -- memory of bad bids fades
        s.violation_stress = s.violation_stress.saturating_sub(10);

        (s.qc_score, s.violation_stress, s.total_reviewed)
    };

    // Critical QC threshold: below 800 = systemic failure, mortality risk
    if qc_score < 800 {
        // Quality below threshold = bid organism in danger of non-responsiveness
        mortality::confront(50);
        serial_println!(
            "  life::proposal_quality: WARNING qc_score={} below 800 -- mortality risk",
            qc_score
        );
    }

    // High violation stress: immune system is overworked
    if violation_stress > 200 {
        immune::defend(violation_stress / 4);
    }

    // Reward for maintaining high quality over many proposals
    if qc_score > 900 && total_reviewed > 50 {
        endocrine::reward(15);
    }

    serial_println!(
        "  life::proposal_quality: tick age={} qc_score={} pages_read={}",
        age,
        qc_score,
        STATE.lock().pages_read
    );
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A proposal passed QC with no violations -- clean send.
/// Reward signal: healthy organism executing correctly.
pub fn proposal_submitted_clean() {
    let (total, clean, qc_score) = {
        let mut s = STATE.lock();
        s.total_reviewed = s.total_reviewed.saturating_add(1);
        s.proposals_clean = s.proposals_clean.saturating_add(1);

        // Recompute qc_score: (clean/total)*1000, EMA-smoothed
        let new_rate = if s.total_reviewed > 0 {
            ((s.proposals_clean as u32)
                .saturating_mul(1000)
                .wrapping_div(s.total_reviewed as u32)) as u16
        } else {
            1000
        };
        let old_score = s.qc_score as u32;
        s.qc_score =
            ((old_score.wrapping_mul(7).saturating_add(new_rate as u32)) / 8).min(1000) as u16;

        (s.total_reviewed, s.proposals_clean, s.qc_score)
    };

    // Clean proposal = reward: the organism is executing its primary function well
    endocrine::reward(30);

    // QC immune system: successful defense (no threats found = clean bill of health)
    immune::defend(0);

    serial_println!(
        "  life::proposal_quality: CLEAN submit #{} (clean={}, qc_score={})",
        total,
        clean,
        qc_score
    );
}

/// A QC violation was found in a proposal. severity 0-1000.
/// Immune response: catch it before it escapes as a bad bid.
pub fn violation_found(severity: u16) {
    let (total_violations, qc_score) = {
        let mut s = STATE.lock();
        s.proposals_with_issues = s.proposals_with_issues.saturating_add(1);
        s.total_reviewed = s.total_reviewed.saturating_add(1);
        s.other_violations = s.other_violations.saturating_add(1);

        // Violation stress accumulates: finding problems is draining
        s.violation_stress = s.violation_stress.saturating_add(severity / 4).min(1000);

        // Recompute qc_score including this bad proposal
        let new_rate = if s.total_reviewed > 0 {
            ((s.proposals_clean as u32)
                .saturating_mul(1000)
                .wrapping_div(s.total_reviewed as u32)) as u16
        } else {
            0
        };
        let old_score = s.qc_score as u32;
        s.qc_score =
            ((old_score.wrapping_mul(7).saturating_add(new_rate as u32)) / 8).min(1000) as u16;

        let total_v = s.sf1449_violations + s.attachment_violations + s.other_violations;
        (total_v, s.qc_score)
    };

    // Immune: catching a violation IS the defense -- threat is real
    immune::defend(severity / 2);

    // Stress: finding a violation before send is better than after, but still costly
    endocrine::stress(severity / 6);

    // Mortality: each violation is a near-miss bid death (non-responsive risk)
    if severity > 600 {
        mortality::confront(severity / 8);
    }

    serial_println!(
        "  life::proposal_quality: violation found severity={} total_violations={} qc_score={}",
        severity,
        total_violations,
        qc_score
    );
}

/// SF-1449 Blocks 23/24 violation found on a CLIN page.
/// This is the most common known failure mode -- every page, every time.
pub fn sf1449_violation() {
    let (count, qc_score) = {
        let mut s = STATE.lock();
        s.sf1449_violations = s.sf1449_violations.saturating_add(1);
        s.violation_stress = s.violation_stress.saturating_add(60).min(1000);

        // SF-1449 violations directly depress the qc_score
        s.qc_score = s.qc_score.saturating_sub(15);

        (s.sf1449_violations, s.qc_score)
    };

    // Strong immune response: this specific rule is known, should have been caught
    immune::defend(400);

    // Stress: rule violation that was explicitly known is worse than ignorant mistake
    endocrine::stress(40);

    // Near-bid-death: SF-1449 incompleteness can render a bid non-responsive
    mortality::confront(25);

    serial_println!(
        "  life::proposal_quality: SF-1449 VIOLATION #{} qc_score={}",
        count,
        qc_score
    );
}

/// Required attachment missing (Attachments 1, 4, or 5).
/// Hard rule violation -- non-responsive risk without all three.
pub fn attachment_missing() {
    let (count, qc_score) = {
        let mut s = STATE.lock();
        s.attachment_violations = s.attachment_violations.saturating_add(1);
        s.violation_stress = s.violation_stress.saturating_add(80).min(1000);

        // Attachment violations directly depress qc_score
        s.qc_score = s.qc_score.saturating_sub(20);

        (s.attachment_violations, s.qc_score)
    };

    // Strong immune response: missing attachment is a critical compliance failure
    immune::defend(500);

    // High stress: attachment 1/4/5 rule is explicit and non-negotiable
    endocrine::stress(50);

    // High mortality risk: missing required attachment = almost certain rejection
    mortality::confront(40);

    serial_println!(
        "  life::proposal_quality: ATTACHMENT MISSING #{} qc_score={}",
        count,
        qc_score
    );
}

/// Pages read from a solicitation. n = number of pages read this session.
/// Reading every page is a hard rule -- ANIMA tracks compliance.
pub fn pages_read_add(n: u32) {
    let (total, reviewed) = {
        let mut s = STATE.lock();
        s.pages_read = s.pages_read.saturating_add(n);
        (s.pages_read, s.total_reviewed)
    };

    // Reading pages earns a small reward: disciplined compliance with the forever rule
    if n > 0 {
        let reward_val = (n as u16).min(50);
        endocrine::reward(reward_val / 4);
    }

    // Milestone: every 1000 pages is a significant intelligence investment
    if total % 1000 < n as u32 {
        serial_println!(
            "  life::proposal_quality: pages_read milestone {} (proposals_reviewed={})",
            (total / 1000) * 1000,
            reviewed
        );
    }
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

/// QC score 0-1000: (clean proposals / total) * 1000, EMA-smoothed.
pub fn get_qc_score() -> u16 {
    STATE.lock().qc_score
}

/// Total violations found across all proposals.
pub fn get_violations_total() -> u32 {
    let s = STATE.lock();
    s.sf1449_violations
        .saturating_add(s.attachment_violations)
        .saturating_add(s.other_violations)
}

/// Cumulative pages read across all solicitations.
pub fn get_pages_read() -> u32 {
    STATE.lock().pages_read
}
