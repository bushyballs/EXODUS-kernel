/// AI Model Community Features for Genesis
///
/// Social layer for the model marketplace: reviews, ratings, forks,
/// fine-tune sharing, improvement submissions, and leaderboards.
/// All content is referenced by hash for privacy; actual text
/// is stored separately in the content-addressed store.
///
/// Ratings use Q16 fixed-point arithmetic (no floating-point).
///
/// Original implementation for Hoags OS.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 helpers ────────────────────────────────────────────────────────────

pub type Q16 = i32;

const Q16_ONE: Q16 = 65536;

fn q16_from_int(v: i32) -> Q16 {
    v << 16
}

fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as i32
}

// ── Enums ──────────────────────────────────────────────────────────────────

/// Type of improvement submitted for a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImprovementType {
    FineTune,
    Quantization,
    Pruning,
    Distillation,
    DataAugmentation,
    BugFix,
    PerformanceOptimization,
    Other,
}

/// Status of a submitted improvement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImprovementStatus {
    Pending,
    UnderReview,
    Accepted,
    Rejected,
    Withdrawn,
}

/// Leaderboard category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderboardCategory {
    TopRated,
    MostHelpful,
    MostForked,
    TopContributor,
    BestFineTune,
    MostDownloaded,
}

// ── Core structs ───────────────────────────────────────────────────────────

/// A user review of a model.
#[derive(Clone)]
pub struct Review {
    pub id: u32,
    pub model_id: u32,
    pub author_hash: u64,
    pub rating: Q16,
    pub text_hash: u64,
    pub timestamp: u64,
    pub helpful_votes: u32,
    pub not_helpful_votes: u32,
    pub verified_download: bool,
    pub version_reviewed: u32,
}

/// A fork of an existing model (community-contributed variant).
#[derive(Clone)]
pub struct ModelFork {
    pub id: u32,
    pub parent_id: u32,
    pub author_hash: u64,
    pub name_hash: u64,
    pub description_hash: u64,
    pub improvement_hash: u64,
    pub created_at: u64,
    pub downloads: u64,
    pub rating: Q16,
    pub rating_count: u32,
    pub improvement_type: ImprovementType,
}

/// An improvement submission attached to a fork.
#[derive(Clone)]
pub struct Improvement {
    pub id: u32,
    pub fork_id: u32,
    pub author_hash: u64,
    pub description_hash: u64,
    pub improvement_type: ImprovementType,
    pub status: ImprovementStatus,
    pub submitted_at: u64,
    pub benchmark_score: Q16,
    pub size_delta: i64,
}

/// A shared fine-tune record.
#[derive(Clone)]
pub struct SharedFineTune {
    pub id: u32,
    pub model_id: u32,
    pub author_hash: u64,
    pub name_hash: u64,
    pub description_hash: u64,
    pub dataset_hash: u64,
    pub base_version: u32,
    pub training_steps: u64,
    pub benchmark_score: Q16,
    pub downloads: u64,
    pub created_at: u64,
}

/// Leaderboard entry for community rankings.
#[derive(Clone)]
pub struct LeaderboardEntry {
    pub rank: u32,
    pub entity_hash: u64,
    pub score: Q16,
    pub category: LeaderboardCategory,
    pub detail_count: u32,
}

// ── Global state ───────────────────────────────────────────────────────────

static COMMUNITY: Mutex<Option<CommunityState>> = Mutex::new(None);

struct CommunityState {
    reviews: Vec<Review>,
    forks: Vec<ModelFork>,
    improvements: Vec<Improvement>,
    fine_tunes: Vec<SharedFineTune>,
    next_review_id: u32,
    next_fork_id: u32,
    next_improvement_id: u32,
    next_fine_tune_id: u32,
    /// Track which (author_hash, review_id) pairs have already voted.
    vote_log: Vec<(u64, u32)>,
}

impl CommunityState {
    fn new() -> Self {
        CommunityState {
            reviews: Vec::new(),
            forks: Vec::new(),
            improvements: Vec::new(),
            fine_tunes: Vec::new(),
            next_review_id: 1,
            next_fork_id: 1,
            next_improvement_id: 1,
            next_fine_tune_id: 1,
            vote_log: Vec::new(),
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialize the community subsystem.
pub fn init() {
    let mut state = COMMUNITY.lock();
    *state = Some(CommunityState::new());
    serial_println!("    AI Market community initialized");
}

/// Post a review for a model. Returns the review ID.
/// Rating must be in Q16 range 0..=5*Q16_ONE.
pub fn post_review(
    model_id: u32,
    author_hash: u64,
    rating: Q16,
    text_hash: u64,
    version_reviewed: u32,
    verified_download: bool,
) -> u32 {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    // Clamp rating to 0..=5.0 in Q16
    let max_rating = q16_from_int(5);
    let clamped_rating = if rating < 0 {
        0
    } else if rating > max_rating {
        max_rating
    } else {
        rating
    };

    let id = state.next_review_id;
    state.next_review_id = state.next_review_id.saturating_add(1);

    let review = Review {
        id,
        model_id,
        author_hash,
        rating: clamped_rating,
        text_hash,
        timestamp: 0, // kernel timestamp
        helpful_votes: 0,
        not_helpful_votes: 0,
        verified_download,
        version_reviewed,
    };
    state.reviews.push(review);
    id
}

/// Get all reviews for a specific model, sorted by helpfulness.
pub fn get_reviews(model_id: u32, offset: usize, limit: usize) -> Vec<Review> {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    let mut reviews: Vec<Review> = state
        .reviews
        .iter()
        .filter(|r| r.model_id == model_id)
        .cloned()
        .collect();

    // Sort by helpful votes descending, then by timestamp descending
    reviews.sort_by(|a, b| {
        b.helpful_votes
            .cmp(&a.helpful_votes)
            .then(b.timestamp.cmp(&a.timestamp))
    });

    // Paginate
    let start = offset.min(reviews.len());
    let end = (offset + limit).min(reviews.len());
    reviews[start..end].to_vec()
}

/// Vote a review as helpful. Each voter (identified by voter_hash) can
/// vote only once per review.
pub fn vote_helpful(review_id: u32, voter_hash: u64, is_helpful: bool) -> bool {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    // Check for duplicate votes
    let already_voted = state
        .vote_log
        .iter()
        .any(|(v, r)| *v == voter_hash && *r == review_id);

    if already_voted {
        return false;
    }

    if let Some(review) = state.reviews.iter_mut().find(|r| r.id == review_id) {
        if is_helpful {
            review.helpful_votes = review.helpful_votes.saturating_add(1);
        } else {
            review.not_helpful_votes = review.not_helpful_votes.saturating_add(1);
        }
        state.vote_log.push((voter_hash, review_id));
        true
    } else {
        false
    }
}

/// Fork a model to create a community variant. Returns the fork ID.
pub fn fork_model(
    parent_id: u32,
    author_hash: u64,
    name_hash: u64,
    description_hash: u64,
    improvement_hash: u64,
    improvement_type: ImprovementType,
) -> u32 {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    let id = state.next_fork_id;
    state.next_fork_id = state.next_fork_id.saturating_add(1);

    let fork = ModelFork {
        id,
        parent_id,
        author_hash,
        name_hash,
        description_hash,
        improvement_hash,
        created_at: 0,
        downloads: 0,
        rating: 0,
        rating_count: 0,
        improvement_type,
    };
    state.forks.push(fork);
    id
}

/// Submit an improvement proposal for a forked model. Returns the improvement ID.
pub fn submit_improvement(
    fork_id: u32,
    author_hash: u64,
    description_hash: u64,
    improvement_type: ImprovementType,
    benchmark_score: Q16,
    size_delta: i64,
) -> Option<u32> {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    // Verify the fork exists
    let fork_exists = state.forks.iter().any(|f| f.id == fork_id);
    if !fork_exists {
        return None;
    }

    let id = state.next_improvement_id;
    state.next_improvement_id = state.next_improvement_id.saturating_add(1);

    let improvement = Improvement {
        id,
        fork_id,
        author_hash,
        description_hash,
        improvement_type,
        status: ImprovementStatus::Pending,
        submitted_at: 0,
        benchmark_score,
        size_delta,
    };
    state.improvements.push(improvement);
    Some(id)
}

/// Get all forks of a specific model.
pub fn get_forks(parent_id: u32) -> Vec<ModelFork> {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    let mut forks: Vec<ModelFork> = state
        .forks
        .iter()
        .filter(|f| f.parent_id == parent_id)
        .cloned()
        .collect();

    // Sort by rating descending, then downloads
    forks.sort_by(|a, b| b.rating.cmp(&a.rating).then(b.downloads.cmp(&a.downloads)));

    forks
}

/// Generate the leaderboard for a given category.
/// Returns the top `count` entries.
pub fn get_leaderboard(category: LeaderboardCategory, count: usize) -> Vec<LeaderboardEntry> {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    let mut entries: Vec<LeaderboardEntry> = Vec::new();

    match category {
        LeaderboardCategory::TopRated => {
            // Aggregate average rating per model from reviews
            let mut model_ratings: Vec<(u64, Q16, u32)> = Vec::new(); // (entity, total, count)

            for review in &state.reviews {
                let key = review.model_id as u64;
                if let Some(entry) = model_ratings.iter_mut().find(|e| e.0 == key) {
                    entry.1 += review.rating;
                    entry.2 += 1;
                } else {
                    model_ratings.push((key, review.rating, 1));
                }
            }

            for (entity, total, cnt) in &model_ratings {
                let avg = q16_div(*total, q16_from_int(*cnt as i32));
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: *entity,
                    score: avg,
                    category,
                    detail_count: *cnt,
                });
            }
        }

        LeaderboardCategory::MostHelpful => {
            // Rank reviewers by total helpful votes received
            let mut author_votes: Vec<(u64, u32)> = Vec::new();

            for review in &state.reviews {
                if let Some(entry) = author_votes.iter_mut().find(|e| e.0 == review.author_hash) {
                    entry.1 += review.helpful_votes;
                } else {
                    author_votes.push((review.author_hash, review.helpful_votes));
                }
            }

            for (author, votes) in &author_votes {
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: *author,
                    score: q16_from_int(*votes as i32),
                    category,
                    detail_count: *votes,
                });
            }
        }

        LeaderboardCategory::MostForked => {
            // Count forks per parent model
            let mut fork_counts: Vec<(u64, u32)> = Vec::new();

            for fork in &state.forks {
                let key = fork.parent_id as u64;
                if let Some(entry) = fork_counts.iter_mut().find(|e| e.0 == key) {
                    entry.1 += 1;
                } else {
                    fork_counts.push((key, 1));
                }
            }

            for (model, cnt) in &fork_counts {
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: *model,
                    score: q16_from_int(*cnt as i32),
                    category,
                    detail_count: *cnt,
                });
            }
        }

        LeaderboardCategory::TopContributor => {
            // Rank authors by total contributions (forks + improvements + fine-tunes)
            let mut contrib_counts: Vec<(u64, u32)> = Vec::new();

            for fork in &state.forks {
                if let Some(entry) = contrib_counts.iter_mut().find(|e| e.0 == fork.author_hash) {
                    entry.1 += 1;
                } else {
                    contrib_counts.push((fork.author_hash, 1));
                }
            }
            for imp in &state.improvements {
                if let Some(entry) = contrib_counts.iter_mut().find(|e| e.0 == imp.author_hash) {
                    entry.1 += 1;
                } else {
                    contrib_counts.push((imp.author_hash, 1));
                }
            }
            for ft in &state.fine_tunes {
                if let Some(entry) = contrib_counts.iter_mut().find(|e| e.0 == ft.author_hash) {
                    entry.1 += 1;
                } else {
                    contrib_counts.push((ft.author_hash, 1));
                }
            }

            for (author, cnt) in &contrib_counts {
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: *author,
                    score: q16_from_int(*cnt as i32),
                    category,
                    detail_count: *cnt,
                });
            }
        }

        LeaderboardCategory::BestFineTune => {
            // Rank fine-tunes by benchmark score
            for ft in &state.fine_tunes {
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: ft.author_hash,
                    score: ft.benchmark_score,
                    category,
                    detail_count: ft.downloads as u32,
                });
            }
        }

        LeaderboardCategory::MostDownloaded => {
            // Rank forks by download count
            for fork in &state.forks {
                entries.push(LeaderboardEntry {
                    rank: 0,
                    entity_hash: fork.parent_id as u64,
                    score: q16_from_int(fork.downloads as i32),
                    category,
                    detail_count: fork.downloads as u32,
                });
            }
        }
    }

    // Sort descending by score
    entries.sort_by(|a, b| b.score.cmp(&a.score));
    entries.truncate(count);

    // Assign ranks
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.rank = (i + 1) as u32;
    }

    entries
}

/// Share a fine-tuned model with the community. Returns the fine-tune ID.
pub fn share_fine_tune(
    model_id: u32,
    author_hash: u64,
    name_hash: u64,
    description_hash: u64,
    dataset_hash: u64,
    base_version: u32,
    training_steps: u64,
    benchmark_score: Q16,
) -> u32 {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    let id = state.next_fine_tune_id;
    state.next_fine_tune_id = state.next_fine_tune_id.saturating_add(1);

    let fine_tune = SharedFineTune {
        id,
        model_id,
        author_hash,
        name_hash,
        description_hash,
        dataset_hash,
        base_version,
        training_steps,
        benchmark_score,
        downloads: 0,
        created_at: 0,
    };
    state.fine_tunes.push(fine_tune);
    id
}

/// Get all shared fine-tunes for a model.
pub fn get_fine_tunes(model_id: u32) -> Vec<SharedFineTune> {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    let mut results: Vec<SharedFineTune> = state
        .fine_tunes
        .iter()
        .filter(|ft| ft.model_id == model_id)
        .cloned()
        .collect();

    results.sort_by(|a, b| b.benchmark_score.cmp(&a.benchmark_score));
    results
}

/// Get the average rating for a model across all reviews.
pub fn get_average_rating(model_id: u32) -> Q16 {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    let mut total: Q16 = 0;
    let mut count: i32 = 0;

    for review in &state.reviews {
        if review.model_id == model_id {
            total += review.rating;
            count += 1;
        }
    }

    if count == 0 {
        return 0;
    }

    q16_div(total, q16_from_int(count))
}

/// Get the total number of reviews for a model.
pub fn get_review_count(model_id: u32) -> u32 {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    state
        .reviews
        .iter()
        .filter(|r| r.model_id == model_id)
        .count() as u32
}

/// Get improvements submitted for a fork.
pub fn get_improvements(fork_id: u32) -> Vec<Improvement> {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    state
        .improvements
        .iter()
        .filter(|imp| imp.fork_id == fork_id)
        .cloned()
        .collect()
}

/// Update the status of an improvement submission.
pub fn update_improvement_status(improvement_id: u32, new_status: ImprovementStatus) -> bool {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    if let Some(imp) = state
        .improvements
        .iter_mut()
        .find(|i| i.id == improvement_id)
    {
        imp.status = new_status;
        true
    } else {
        false
    }
}

/// Rate a fork. Updates the running average.
pub fn rate_fork(fork_id: u32, new_rating: Q16) -> bool {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    let max_rating = q16_from_int(5);
    let clamped = if new_rating < 0 {
        0
    } else if new_rating > max_rating {
        max_rating
    } else {
        new_rating
    };

    if let Some(fork) = state.forks.iter_mut().find(|f| f.id == fork_id) {
        let total = q16_mul(fork.rating, q16_from_int(fork.rating_count as i32));
        fork.rating_count = fork.rating_count.saturating_add(1);
        let new_total = total + clamped;
        fork.rating = q16_div(new_total, q16_from_int(fork.rating_count as i32));
        true
    } else {
        false
    }
}

/// Record a download of a fork.
pub fn record_fork_download(fork_id: u32) -> bool {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    if let Some(fork) = state.forks.iter_mut().find(|f| f.id == fork_id) {
        fork.downloads = fork.downloads.saturating_add(1);
        true
    } else {
        false
    }
}

/// Record a download of a shared fine-tune.
pub fn record_fine_tune_download(fine_tune_id: u32) -> bool {
    let mut guard = COMMUNITY.lock();
    let state = guard.as_mut().expect("community not initialized");

    if let Some(ft) = state.fine_tunes.iter_mut().find(|f| f.id == fine_tune_id) {
        ft.downloads = ft.downloads.saturating_add(1);
        true
    } else {
        false
    }
}

/// Get total community statistics.
pub fn get_stats() -> (u32, u32, u32, u32) {
    let guard = COMMUNITY.lock();
    let state = guard.as_ref().expect("community not initialized");

    (
        state.reviews.len() as u32,
        state.forks.len() as u32,
        state.improvements.len() as u32,
        state.fine_tunes.len() as u32,
    )
}
