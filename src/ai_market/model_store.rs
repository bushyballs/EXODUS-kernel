/// AI Model Marketplace Store for Genesis
///
/// Provides a browsable, searchable catalog of AI models available for
/// on-device inference. Models are indexed by category, rating, popularity,
/// and compatibility. All ratings use Q16 fixed-point arithmetic.
///
/// Original implementation for Hoags OS.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 fixed-point helpers ────────────────────────────────────────────────

/// Q16 fixed-point: 16 bits integer, 16 bits fraction.
/// 1.0 = 65536, 0.5 = 32768, 5.0 = 327680
pub type Q16 = i32;

const Q16_ONE: Q16 = 65536;
const Q16_HALF: Q16 = 32768;

/// Multiply two Q16 values.
fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Divide two Q16 values.
fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Convert integer to Q16.
fn q16_from_int(v: i32) -> Q16 {
    v << 16
}

// ── Enums ──────────────────────────────────────────────────────────────────

/// Category of an AI model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCategory {
    Chat,
    Code,
    Vision,
    Audio,
    Translation,
    Embedding,
    Classification,
    Generation,
    Custom,
}

/// Report reason for flagging a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportReason {
    Malicious,
    Copyright,
    Inaccurate,
    Inappropriate,
    Spam,
    Other,
}

/// Sort order for listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Newest,
    HighestRated,
    MostDownloaded,
    Trending,
    Alphabetical,
}

// ── Core structs ───────────────────────────────────────────────────────────

/// A single model listing in the marketplace.
#[derive(Clone)]
pub struct ModelListing {
    pub id: u32,
    pub name_hash: u64,
    pub author_hash: u64,
    pub description_hash: u64,
    pub category: ModelCategory,
    pub size_bytes: u64,
    pub params: u64,
    pub quant_mode: u8,
    pub rating: Q16,
    pub downloads: u64,
    pub version: u32,
    pub license_hash: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub reported: bool,
    pub featured: bool,
    pub compatible_hw: Vec<u64>,
    pub tags: Vec<u64>,
}

/// A report filed against a model.
#[derive(Clone)]
pub struct ModelReport {
    pub model_id: u32,
    pub reporter_hash: u64,
    pub reason: ReportReason,
    pub detail_hash: u64,
    pub timestamp: u64,
}

/// Trending score entry.
#[derive(Clone)]
pub struct TrendingEntry {
    pub model_id: u32,
    pub score: Q16,
    pub recent_downloads: u64,
    pub recent_ratings: u32,
}

/// Recommendation entry.
#[derive(Clone)]
pub struct Recommendation {
    pub model_id: u32,
    pub relevance: Q16,
    pub reason_hash: u64,
}

// ── Global state ───────────────────────────────────────────────────────────

static STORE: Mutex<Option<MarketStore>> = Mutex::new(None);

struct MarketStore {
    listings: Vec<ModelListing>,
    reports: Vec<ModelReport>,
    next_id: u32,
    trending_cache: Vec<TrendingEntry>,
    trending_epoch: u64,
}

impl MarketStore {
    fn new() -> Self {
        MarketStore {
            listings: Vec::new(),
            reports: Vec::new(),
            next_id: 1,
            trending_cache: Vec::new(),
            trending_epoch: 0,
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialize the model store subsystem.
pub fn init() {
    let mut store = STORE.lock();
    *store = Some(MarketStore::new());
    serial_println!("    AI Market store initialized");
}

/// Add a new model listing to the store. Returns the assigned listing ID.
pub fn add_listing(
    name_hash: u64,
    author_hash: u64,
    description_hash: u64,
    category: ModelCategory,
    size_bytes: u64,
    params: u64,
    quant_mode: u8,
    license_hash: u64,
    compatible_hw: Vec<u64>,
    tags: Vec<u64>,
) -> u32 {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");
    let id = store.next_id;
    store.next_id = store.next_id.saturating_add(1);

    let listing = ModelListing {
        id,
        name_hash,
        author_hash,
        description_hash,
        category,
        size_bytes,
        params,
        quant_mode,
        rating: 0,
        downloads: 0,
        version: 1,
        license_hash,
        created_at: 0, // kernel timestamp would go here
        updated_at: 0,
        reported: false,
        featured: false,
        compatible_hw,
        tags,
    };
    store.listings.push(listing);
    id
}

/// List all models, optionally sorted.
pub fn list_models(sort: SortOrder, offset: usize, limit: usize) -> Vec<ModelListing> {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");

    let mut results: Vec<ModelListing> = store.listings.clone();

    match sort {
        SortOrder::Newest => results.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        SortOrder::HighestRated => results.sort_by(|a, b| b.rating.cmp(&a.rating)),
        SortOrder::MostDownloaded => results.sort_by(|a, b| b.downloads.cmp(&a.downloads)),
        SortOrder::Trending => {
            // Sort by a combination of recent downloads and rating
            results.sort_by(|a, b| {
                let score_a = q16_mul(a.rating, q16_from_int(a.downloads as i32));
                let score_b = q16_mul(b.rating, q16_from_int(b.downloads as i32));
                score_b.cmp(&score_a)
            });
        }
        SortOrder::Alphabetical => results.sort_by(|a, b| a.name_hash.cmp(&b.name_hash)),
    }

    // Apply pagination
    let start = offset.min(results.len());
    let end = (offset + limit).min(results.len());
    results[start..end].to_vec()
}

/// Search models by name hash or tag hash. Returns matching listings.
pub fn search(query_hash: u64, category_filter: Option<ModelCategory>) -> Vec<ModelListing> {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");

    let mut results = Vec::new();
    for listing in &store.listings {
        let name_match = listing.name_hash == query_hash;
        let tag_match = listing.tags.iter().any(|t| *t == query_hash);
        let author_match = listing.author_hash == query_hash;
        let desc_match = listing.description_hash == query_hash;

        if name_match || tag_match || author_match || desc_match {
            if let Some(cat) = category_filter {
                if listing.category != cat {
                    continue;
                }
            }
            results.push(listing.clone());
        }
    }

    // Sort results by relevance: exact name match first, then by rating
    results.sort_by(|a, b| {
        let a_exact = if a.name_hash == query_hash { 1 } else { 0 };
        let b_exact = if b.name_hash == query_hash { 1 } else { 0 };
        b_exact.cmp(&a_exact).then(b.rating.cmp(&a.rating))
    });

    results
}

/// Get all models in a specific category.
pub fn get_by_category(category: ModelCategory, sort: SortOrder) -> Vec<ModelListing> {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");

    let mut results: Vec<ModelListing> = store
        .listings
        .iter()
        .filter(|l| l.category == category)
        .cloned()
        .collect();

    match sort {
        SortOrder::HighestRated => results.sort_by(|a, b| b.rating.cmp(&a.rating)),
        SortOrder::MostDownloaded => results.sort_by(|a, b| b.downloads.cmp(&a.downloads)),
        SortOrder::Newest => results.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        _ => {}
    }

    results
}

/// Get full details of a single model by ID.
pub fn get_details(model_id: u32) -> Option<ModelListing> {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");
    store.listings.iter().find(|l| l.id == model_id).cloned()
}

/// Rate a model. Rating is a Q16 value (0 to 5 * Q16_ONE).
/// Computes a running weighted average.
pub fn rate_model(model_id: u32, new_rating: Q16) -> bool {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");

    if let Some(listing) = store.listings.iter_mut().find(|l| l.id == model_id) {
        // Clamp rating to valid range: 0..=5.0 in Q16
        let max_rating = q16_from_int(5);
        let clamped = if new_rating < 0 {
            0
        } else if new_rating > max_rating {
            max_rating
        } else {
            new_rating
        };

        // Weighted running average: new_avg = (old * downloads + new) / (downloads + 1)
        let total = q16_mul(listing.rating, q16_from_int(listing.downloads as i32));
        listing.downloads = listing.downloads.saturating_add(1); // Using downloads as a proxy for rating count here
        let new_total = total + clamped;
        listing.rating = q16_div(new_total, q16_from_int(listing.downloads as i32));
        true
    } else {
        false
    }
}

/// Report a model for policy violation.
pub fn report_model(
    model_id: u32,
    reporter_hash: u64,
    reason: ReportReason,
    detail_hash: u64,
) -> bool {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");

    // Verify model exists
    let exists = store.listings.iter().any(|l| l.id == model_id);
    if !exists {
        return false;
    }

    // Mark model as reported
    if let Some(listing) = store.listings.iter_mut().find(|l| l.id == model_id) {
        listing.reported = true;
    }

    let report = ModelReport {
        model_id,
        reporter_hash,
        reason,
        detail_hash,
        timestamp: 0, // kernel timestamp
    };
    store.reports.push(report);
    true
}

/// Get trending models based on recent activity.
/// Computes a trending score from recent downloads, rating velocity, and recency.
pub fn get_trending(count: usize) -> Vec<TrendingEntry> {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");

    // Rebuild trending cache
    let mut entries: Vec<TrendingEntry> = Vec::new();

    for listing in &store.listings {
        if listing.reported {
            continue;
        }

        // Trending score: rating * log2(downloads + 1) weighted by recency
        let download_factor = if listing.downloads == 0 {
            Q16_ONE
        } else {
            // Approximate log2 via bit length
            let bits = 64 - listing.downloads.leading_zeros();
            q16_from_int(bits as i32)
        };

        let score = q16_mul(listing.rating, download_factor);

        entries.push(TrendingEntry {
            model_id: listing.id,
            score,
            recent_downloads: listing.downloads,
            recent_ratings: 0,
        });
    }

    // Sort by trending score descending
    entries.sort_by(|a, b| b.score.cmp(&a.score));
    entries.truncate(count);

    store.trending_cache = entries.clone();
    entries
}

/// Get personalized recommendations based on user history.
/// Takes a list of category preferences (hashes of previously used categories)
/// and returns models that match the user's interests.
pub fn get_recommended(
    user_categories: &[ModelCategory],
    hw_hash: u64,
    count: usize,
) -> Vec<Recommendation> {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");

    let mut scored: Vec<Recommendation> = Vec::new();

    for listing in &store.listings {
        if listing.reported {
            continue;
        }

        let mut relevance: Q16 = 0;

        // Category match bonus
        if user_categories.contains(&listing.category) {
            relevance += q16_from_int(3); // +3.0 for category match
        }

        // Hardware compatibility bonus
        if listing.compatible_hw.iter().any(|h| *h == hw_hash) {
            relevance += q16_from_int(2); // +2.0 for hw compat
        }

        // Rating bonus (normalized: rating / 5.0)
        let max_q16 = q16_from_int(5);
        if max_q16 != 0 {
            let norm_rating = q16_div(listing.rating, max_q16);
            relevance += norm_rating; // +0.0 to +1.0
        }

        // Popularity bonus: log2(downloads + 1) / 20 as Q16
        if listing.downloads > 0 {
            let bits = 64 - listing.downloads.leading_zeros();
            let pop_bonus = q16_div(q16_from_int(bits as i32), q16_from_int(20));
            relevance += pop_bonus;
        }

        // Featured bonus
        if listing.featured {
            relevance += Q16_HALF; // +0.5
        }

        if relevance > 0 {
            scored.push(Recommendation {
                model_id: listing.id,
                relevance,
                reason_hash: listing.category as u64,
            });
        }
    }

    // Sort by relevance descending
    scored.sort_by(|a, b| b.relevance.cmp(&a.relevance));
    scored.truncate(count);
    scored
}

/// Get total number of models in the store.
pub fn get_count() -> usize {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");
    store.listings.len()
}

/// Get total number of reports filed.
pub fn get_report_count() -> usize {
    let guard = STORE.lock();
    let store = guard.as_ref().expect("store not initialized");
    store.reports.len()
}

/// Toggle the featured flag on a model.
pub fn set_featured(model_id: u32, featured: bool) -> bool {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");
    if let Some(listing) = store.listings.iter_mut().find(|l| l.id == model_id) {
        listing.featured = featured;
        true
    } else {
        false
    }
}

/// Increment the download count for a model.
pub fn record_download(model_id: u32) -> bool {
    let mut guard = STORE.lock();
    let store = guard.as_mut().expect("store not initialized");
    if let Some(listing) = store.listings.iter_mut().find(|l| l.id == model_id) {
        listing.downloads = listing.downloads.saturating_add(1);
        listing.updated_at = listing.updated_at.saturating_add(1); // simplistic update marker
        true
    } else {
        false
    }
}
