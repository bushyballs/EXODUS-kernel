/// App reviews and ratings for Genesis store
///
/// Star ratings, review text, helpfulness voting,
/// moderation queue, average rating calculation,
/// review sorting and filtering.
///
/// Original implementation for Hoags OS.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers (i32 with 16 fractional bits, NO floats)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT; // 65536

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

fn q16_to_int(v: i32) -> i32 {
    v >> Q16_SHIFT
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ModerationStatus {
    Pending,
    Approved,
    Rejected,
    Flagged,
    AutoApproved,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ReviewSortOrder {
    MostRecent,
    MostHelpful,
    HighestRated,
    LowestRated,
    MostCritical,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FlagReason {
    Spam,
    Inappropriate,
    FakeReview,
    Harassment,
    OffTopic,
    Other,
}

struct Review {
    id: u32,
    listing_id: u32,
    user_hash: u64,
    star_rating: u8,          // 1-5
    title: [u8; 64],
    title_len: usize,
    body: [u8; 512],
    body_len: usize,
    timestamp: u64,
    updated_at: u64,
    helpful_yes: u32,
    helpful_no: u32,
    moderation: ModerationStatus,
    flag_reason: Option<FlagReason>,
    verified_purchase: bool,
    version_major: u8,
    version_minor: u8,
    developer_replied: bool,
    reply_timestamp: u64,
}

struct HelpfulnessVote {
    review_id: u32,
    user_hash: u64,
    helpful: bool,
    timestamp: u64,
}

struct RatingSnapshot {
    listing_id: u32,
    one_star: u32,
    two_star: u32,
    three_star: u32,
    four_star: u32,
    five_star: u32,
    total_reviews: u32,
    average_q16: i32,         // Q16 fixed-point average
    last_computed: u64,
}

struct ReviewEngine {
    reviews: Vec<Review>,
    votes: Vec<HelpfulnessVote>,
    snapshots: Vec<RatingSnapshot>,
    next_review_id: u32,
    moderation_queue_count: u32,
    total_approved: u32,
    total_rejected: u32,
    auto_approve_threshold: u32,   // user reputation score needed
}

static REVIEWS: Mutex<Option<ReviewEngine>> = Mutex::new(None);

impl ReviewEngine {
    fn new() -> Self {
        ReviewEngine {
            reviews: Vec::new(),
            votes: Vec::new(),
            snapshots: Vec::new(),
            next_review_id: 1,
            moderation_queue_count: 0,
            total_approved: 0,
            total_rejected: 0,
            auto_approve_threshold: 50,
        }
    }

    fn submit_review(
        &mut self,
        listing_id: u32,
        user_hash: u64,
        stars: u8,
        title: &[u8],
        body: &[u8],
        timestamp: u64,
        verified: bool,
        ver_major: u8,
        ver_minor: u8,
    ) -> u32 {
        // Clamp stars 1-5
        let clamped = if stars < 1 { 1 } else if stars > 5 { 5 } else { stars };

        // Check if user already reviewed this app
        if self.reviews.iter().any(|r| r.listing_id == listing_id && r.user_hash == user_hash) {
            return 0; // duplicate, caller should use edit
        }

        let id = self.next_review_id;
        self.next_review_id = self.next_review_id.saturating_add(1);

        let mut t = [0u8; 64];
        let tlen = title.len().min(64);
        t[..tlen].copy_from_slice(&title[..tlen]);

        let mut b = [0u8; 512];
        let blen = body.len().min(512);
        b[..blen].copy_from_slice(&body[..blen]);

        let moderation = if verified {
            ModerationStatus::AutoApproved
        } else {
            ModerationStatus::Pending
        };

        if moderation == ModerationStatus::Pending {
            self.moderation_queue_count = self.moderation_queue_count.saturating_add(1);
        } else {
            self.total_approved = self.total_approved.saturating_add(1);
        }

        self.reviews.push(Review {
            id,
            listing_id,
            user_hash,
            star_rating: clamped,
            title: t,
            title_len: tlen,
            body: b,
            body_len: blen,
            timestamp,
            updated_at: timestamp,
            helpful_yes: 0,
            helpful_no: 0,
            moderation,
            flag_reason: None,
            verified_purchase: verified,
            version_major: ver_major,
            version_minor: ver_minor,
            developer_replied: false,
            reply_timestamp: 0,
        });

        // Recalculate rating snapshot
        self.recompute_snapshot(listing_id, timestamp);
        id
    }

    fn edit_review(&mut self, review_id: u32, user_hash: u64, stars: u8, body: &[u8], timestamp: u64) -> bool {
        if let Some(rev) = self.reviews.iter_mut().find(|r| r.id == review_id && r.user_hash == user_hash) {
            let clamped = if stars < 1 { 1 } else if stars > 5 { 5 } else { stars };
            rev.star_rating = clamped;
            let blen = body.len().min(512);
            rev.body = [0u8; 512];
            rev.body[..blen].copy_from_slice(&body[..blen]);
            rev.body_len = blen;
            rev.updated_at = timestamp;
            rev.moderation = ModerationStatus::Pending;
            self.moderation_queue_count = self.moderation_queue_count.saturating_add(1);
            let lid = rev.listing_id;
            self.recompute_snapshot(lid, timestamp);
            return true;
        }
        false
    }

    fn vote_helpful(&mut self, review_id: u32, user_hash: u64, helpful: bool, timestamp: u64) -> bool {
        // Prevent double-voting
        if self.votes.iter().any(|v| v.review_id == review_id && v.user_hash == user_hash) {
            return false;
        }

        self.votes.push(HelpfulnessVote {
            review_id,
            user_hash,
            helpful,
            timestamp,
        });

        if let Some(rev) = self.reviews.iter_mut().find(|r| r.id == review_id) {
            if helpful {
                rev.helpful_yes = rev.helpful_yes.saturating_add(1);
            } else {
                rev.helpful_no = rev.helpful_no.saturating_add(1);
            }
        }
        true
    }

    fn flag_review(&mut self, review_id: u32, reason: FlagReason) -> bool {
        if let Some(rev) = self.reviews.iter_mut().find(|r| r.id == review_id) {
            rev.moderation = ModerationStatus::Flagged;
            rev.flag_reason = Some(reason);
            self.moderation_queue_count = self.moderation_queue_count.saturating_add(1);
            return true;
        }
        false
    }

    fn moderate(&mut self, review_id: u32, approve: bool) -> bool {
        if let Some(rev) = self.reviews.iter_mut().find(|r| r.id == review_id) {
            if approve {
                rev.moderation = ModerationStatus::Approved;
                self.total_approved = self.total_approved.saturating_add(1);
            } else {
                rev.moderation = ModerationStatus::Rejected;
                self.total_rejected = self.total_rejected.saturating_add(1);
            }
            if self.moderation_queue_count > 0 {
                self.moderation_queue_count -= 1;
            }
            return true;
        }
        false
    }

    fn get_reviews_for_app(&self, listing_id: u32, sort: ReviewSortOrder, limit: usize) -> Vec<u32> {
        let mut visible: Vec<&Review> = self.reviews.iter()
            .filter(|r| {
                r.listing_id == listing_id
                    && (r.moderation == ModerationStatus::Approved
                        || r.moderation == ModerationStatus::AutoApproved)
            })
            .collect();

        match sort {
            ReviewSortOrder::MostRecent => visible.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)),
            ReviewSortOrder::MostHelpful => visible.sort_by(|a, b| b.helpful_yes.cmp(&a.helpful_yes)),
            ReviewSortOrder::HighestRated => visible.sort_by(|a, b| b.star_rating.cmp(&a.star_rating)),
            ReviewSortOrder::LowestRated => visible.sort_by(|a, b| a.star_rating.cmp(&b.star_rating)),
            ReviewSortOrder::MostCritical => {
                visible.sort_by(|a, b| {
                    let a_score = a.helpful_yes.saturating_mul(5 - a.star_rating as u32);
                    let b_score = b.helpful_yes.saturating_mul(5 - b.star_rating as u32);
                    b_score.cmp(&a_score)
                });
            }
        }

        visible.iter().take(limit).map(|r| r.id).collect()
    }

    fn recompute_snapshot(&mut self, listing_id: u32, timestamp: u64) {
        let mut one = 0u32;
        let mut two = 0u32;
        let mut three = 0u32;
        let mut four = 0u32;
        let mut five = 0u32;
        let mut total = 0u32;

        for r in &self.reviews {
            if r.listing_id != listing_id { continue; }
            if r.moderation == ModerationStatus::Rejected { continue; }
            match r.star_rating {
                1 => one += 1,
                2 => two += 1,
                3 => three += 1,
                4 => four += 1,
                5 => five += 1,
                _ => {}
            }
            total += 1;
        }

        let weighted_sum = one + two * 2 + three * 3 + four * 4 + five * 5;
        let avg = if total > 0 {
            q16_div(weighted_sum as i32, total as i32)
        } else {
            0
        };

        let snap = RatingSnapshot {
            listing_id,
            one_star: one,
            two_star: two,
            three_star: three,
            four_star: four,
            five_star: five,
            total_reviews: total,
            average_q16: avg,
            last_computed: timestamp,
        };

        if let Some(existing) = self.snapshots.iter_mut().find(|s| s.listing_id == listing_id) {
            *existing = snap;
        } else {
            self.snapshots.push(snap);
        }
    }

    fn get_average_rating_q16(&self, listing_id: u32) -> i32 {
        self.snapshots.iter()
            .find(|s| s.listing_id == listing_id)
            .map(|s| s.average_q16)
            .unwrap_or(0)
    }

    fn get_review_count(&self, listing_id: u32) -> u32 {
        self.snapshots.iter()
            .find(|s| s.listing_id == listing_id)
            .map(|s| s.total_reviews)
            .unwrap_or(0)
    }

    fn pending_moderation(&self) -> Vec<u32> {
        self.reviews.iter()
            .filter(|r| r.moderation == ModerationStatus::Pending || r.moderation == ModerationStatus::Flagged)
            .map(|r| r.id)
            .collect()
    }

    fn helpfulness_score_q16(&self, review_id: u32) -> i32 {
        if let Some(rev) = self.reviews.iter().find(|r| r.id == review_id) {
            let total = rev.helpful_yes + rev.helpful_no;
            if total == 0 { return 0; }
            q16_div(rev.helpful_yes as i32, total as i32)
        } else {
            0
        }
    }
}

pub fn init() {
    let mut engine = REVIEWS.lock();
    *engine = Some(ReviewEngine::new());
    serial_println!("    App store: reviews and ratings engine ready");
}
