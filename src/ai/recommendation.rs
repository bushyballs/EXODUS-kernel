use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
/// Recommendation engine
///
/// Part of the Hoags AI subsystem. Content-based recommendation engine
/// using item feature vectors, user preference profiles, and cosine
/// similarity scoring. Generates top-N recommendations with explanations.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A single recommendation with score and rationale
pub struct Recommendation {
    pub item_id: u64,
    pub score: f32,
    pub reason: String,
}

/// An item in the catalog with feature vector
#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub name: String,
    pub description: String,
    /// Feature vector (sparse: feature_name -> weight)
    pub features: BTreeMap<String, f32>,
    /// Category tags
    pub tags: Vec<String>,
    /// Popularity score (higher = more popular)
    pub popularity: f32,
    /// Whether this item is active/available
    pub active: bool,
}

impl Item {
    pub fn new(id: u64, name: &str, description: &str) -> Self {
        Item {
            id,
            name: String::from(name),
            description: String::from(description),
            features: BTreeMap::new(),
            tags: Vec::new(),
            popularity: 0.0,
            active: true,
        }
    }

    fn with_features(mut self, features: &[(&str, f32)]) -> Self {
        for &(name, weight) in features {
            self.features.insert(String::from(name), weight);
        }
        self
    }

    fn with_tags(mut self, tags: &[&str]) -> Self {
        for tag in tags {
            self.tags.push(String::from(*tag));
        }
        self
    }

    fn with_popularity(mut self, pop: f32) -> Self {
        self.popularity = pop;
        self
    }
}

/// User preference profile
#[derive(Clone)]
pub struct UserProfile {
    /// Feature preferences (feature_name -> weight, positive = likes, negative = dislikes)
    pub preferences: BTreeMap<String, f32>,
    /// Tags the user has shown interest in
    pub interested_tags: Vec<String>,
    /// Items the user has already interacted with (to filter from recommendations)
    pub seen_items: Vec<u64>,
    /// Interaction history: (item_id, rating)
    pub history: Vec<(u64, f32)>,
}

impl UserProfile {
    pub fn new() -> Self {
        UserProfile {
            preferences: BTreeMap::new(),
            interested_tags: Vec::new(),
            seen_items: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Record that the user interacted with an item and gave a rating
    pub fn record_interaction(&mut self, item_id: u64, rating: f32) {
        if !self.seen_items.contains(&item_id) {
            self.seen_items.push(item_id);
        }
        self.history.push((item_id, rating));
    }

    /// Update preferences based on an item's features and user rating
    pub fn update_from_item(&mut self, item: &Item, rating: f32, learning_rate: f32) {
        // Positive rating increases preference for item's features
        // Negative rating decreases them
        let signal = (rating - 0.5) * 2.0; // Map [0,1] to [-1, 1]
        for (feature, &weight) in &item.features {
            let current = self.preferences.get(feature).copied().unwrap_or(0.0);
            let update = current + learning_rate * signal * weight;
            self.preferences
                .insert(feature.clone(), update.max(-5.0).min(5.0));
        }

        // Update tag interests
        if rating > 0.5 {
            for tag in &item.tags {
                if !self.interested_tags.contains(tag) {
                    self.interested_tags.push(tag.clone());
                }
            }
        }

        self.record_interaction(item.id, rating);
    }
}

pub struct RecommendationEngine {
    pub user_profile: Vec<f32>,
    /// Item catalog
    items: Vec<Item>,
    /// Active user profiles (user_id -> profile)
    profiles: BTreeMap<u64, UserProfile>,
    /// Learning rate for preference updates
    learning_rate: f32,
    /// Weight for content-based similarity vs popularity
    content_weight: f32,
    /// Weight for popularity in scoring
    popularity_weight: f32,
    /// Weight for tag overlap
    tag_weight: f32,
    /// Diversity factor: penalize recommendations too similar to each other
    diversity_factor: f32,
    /// Next item ID
    next_item_id: u64,
}

impl RecommendationEngine {
    pub fn new() -> Self {
        RecommendationEngine {
            user_profile: Vec::new(),
            items: Vec::new(),
            profiles: BTreeMap::new(),
            learning_rate: 0.1,
            content_weight: 0.6,
            popularity_weight: 0.2,
            tag_weight: 0.2,
            diversity_factor: 0.3,
            next_item_id: 1,
        }
    }

    /// Add an item to the catalog
    pub fn add_item(&mut self, item: Item) {
        if item.id >= self.next_item_id {
            self.next_item_id = item.id + 1;
        }
        self.items.push(item);
    }

    /// Add an item with auto-generated ID
    pub fn add_item_auto(
        &mut self,
        name: &str,
        description: &str,
        features: &[(&str, f32)],
        tags: &[&str],
    ) -> u64 {
        let id = self.next_item_id;
        self.next_item_id = self.next_item_id.saturating_add(1);
        let item = Item::new(id, name, description)
            .with_features(features)
            .with_tags(tags);
        self.items.push(item);
        id
    }

    /// Get or create a user profile
    pub fn get_profile(&mut self, user_id: u64) -> &mut UserProfile {
        // entry API guarantees the key is present; eliminates the .unwrap() on get_mut.
        self.profiles
            .entry(user_id)
            .or_insert_with(UserProfile::new)
    }

    /// Record a user interaction (rating an item)
    pub fn record_rating(&mut self, user_id: u64, item_id: u64, rating: f32) {
        let item = self.items.iter().find(|i| i.id == item_id).cloned();
        let lr = self.learning_rate;
        let profile = self.get_profile(user_id);
        if let Some(item) = item {
            profile.update_from_item(&item, rating, lr);
        }
    }

    /// Generate top-N recommendations for a user
    pub fn recommend_for_user(&self, user_id: u64, top_n: usize) -> Vec<Recommendation> {
        let profile = match self.profiles.get(&user_id) {
            Some(p) => p,
            None => {
                // No profile: return most popular items
                return self.popular_items(top_n);
            }
        };

        self.recommend_with_profile(profile, top_n)
    }

    /// Generate recommendations based on context text (no user ID needed)
    pub fn recommend(&self, context: &str, top_n: usize) -> Vec<Recommendation> {
        if self.items.is_empty() {
            return Vec::new();
        }

        // Build a temporary profile from context keywords
        let context_features = extract_context_features(context);
        let temp_profile = UserProfile {
            preferences: context_features,
            interested_tags: extract_context_tags(context),
            seen_items: Vec::new(),
            history: Vec::new(),
        };

        self.recommend_with_profile(&temp_profile, top_n)
    }

    /// Internal: generate recommendations given a profile
    fn recommend_with_profile(&self, profile: &UserProfile, top_n: usize) -> Vec<Recommendation> {
        let mut scored: Vec<(u64, f32, String)> = Vec::new();

        for item in &self.items {
            if !item.active {
                continue;
            }
            // Skip already-seen items
            if profile.seen_items.contains(&item.id) {
                continue;
            }

            // Content-based similarity: cosine similarity between profile preferences and item features
            let content_score = cosine_similarity(&profile.preferences, &item.features);

            // Tag overlap score
            let tag_score = tag_overlap_score(&profile.interested_tags, &item.tags);

            // Popularity score (normalized to [0, 1])
            let max_pop = self
                .items
                .iter()
                .map(|i| i.popularity)
                .fold(0.0f32, |a, b| if b > a { b } else { a })
                .max(1.0);
            let pop_score = item.popularity / max_pop;

            // Composite score
            let total_score = self.content_weight * content_score
                + self.tag_weight * tag_score
                + self.popularity_weight * pop_score;

            // Generate explanation
            let reason = generate_reason(profile, item, content_score, tag_score, pop_score);

            scored.push((item.id, total_score, reason));
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

        // Apply diversity: penalize items too similar to already-selected recommendations
        let mut selected: Vec<Recommendation> = Vec::new();
        for (item_id, score, reason) in scored {
            if selected.len() >= top_n {
                break;
            }

            // Check diversity against already-selected items
            let mut diversity_penalty = 0.0f32;
            if self.diversity_factor > 0.0 {
                let item = match self.items.iter().find(|i| i.id == item_id) {
                    Some(i) => i,
                    None => continue,
                };
                for rec in &selected {
                    let other = match self.items.iter().find(|i| i.id == rec.item_id) {
                        Some(i) => i,
                        None => continue,
                    };
                    let sim = cosine_similarity(&item.features, &other.features);
                    diversity_penalty += sim * self.diversity_factor;
                }
            }

            let adjusted_score = (score - diversity_penalty).max(0.0);

            selected.push(Recommendation {
                item_id,
                score: adjusted_score,
                reason,
            });
        }

        // Re-sort after diversity adjustment
        selected.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        selected
    }

    /// Get most popular items as recommendations
    fn popular_items(&self, top_n: usize) -> Vec<Recommendation> {
        let mut items: Vec<&Item> = self.items.iter().filter(|i| i.active).collect();
        items.sort_by(|a, b| {
            b.popularity
                .partial_cmp(&a.popularity)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        items
            .iter()
            .take(top_n)
            .map(|item| Recommendation {
                item_id: item.id,
                score: item.popularity,
                reason: format!("Popular: {}", item.name),
            })
            .collect()
    }

    /// Number of items in catalog
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Number of user profiles
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }
}

// ---------------------------------------------------------------------------
// Math and helper functions
// ---------------------------------------------------------------------------

/// Cosine similarity between two sparse vectors (BTreeMap<String, f32>)
fn cosine_similarity(a: &BTreeMap<String, f32>, b: &BTreeMap<String, f32>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (key, &va) in a {
        norm_a += va * va;
        if let Some(&vb) = b.get(key) {
            dot += va * vb;
        }
    }
    for (_, &vb) in b {
        norm_b += vb * vb;
    }

    let denom = sqrt_f32(norm_a) * sqrt_f32(norm_b);
    if denom < 1e-10 {
        return 0.0;
    }
    (dot / denom).max(-1.0).min(1.0)
}

/// Calculate tag overlap as a fraction
fn tag_overlap_score(user_tags: &[String], item_tags: &[String]) -> f32 {
    if user_tags.is_empty() || item_tags.is_empty() {
        return 0.0;
    }
    let overlap = item_tags
        .iter()
        .filter(|t| user_tags.iter().any(|ut| ut == *t))
        .count();
    overlap as f32 / item_tags.len() as f32
}

/// Generate a human-readable reason for a recommendation
fn generate_reason(
    profile: &UserProfile,
    item: &Item,
    content_score: f32,
    tag_score: f32,
    pop_score: f32,
) -> String {
    let mut reasons = Vec::new();

    if content_score > 0.3 {
        // Find the top matching feature
        let mut best_feature = String::from("content");
        let mut best_match = 0.0f32;
        for (feature, &item_weight) in &item.features {
            if let Some(&pref_weight) = profile.preferences.get(feature) {
                let match_score = item_weight * pref_weight;
                if match_score > best_match {
                    best_match = match_score;
                    best_feature = feature.clone();
                }
            }
        }
        reasons.push(format!("Matches preference: {}", best_feature));
    }

    if tag_score > 0.0 {
        let matching_tags: Vec<&String> = item
            .tags
            .iter()
            .filter(|t| profile.interested_tags.iter().any(|ut| ut == *t))
            .collect();
        if let Some(tag) = matching_tags.first() {
            reasons.push(format!("Tags: {}", tag));
        }
    }

    if pop_score > 0.7 {
        reasons.push(String::from("Highly popular"));
    }

    if reasons.is_empty() {
        format!("Suggested: {}", item.name)
    } else {
        reasons.join("; ")
    }
}

/// Extract feature weights from context text
fn extract_context_features(context: &str) -> BTreeMap<String, f32> {
    let mut features = BTreeMap::new();
    let lower = context.to_lowercase();

    // Count word frequencies as feature weights
    for chunk in lower.split(|c: char| !c.is_alphanumeric()) {
        if chunk.len() >= 3 {
            let count = features.get(chunk).copied().unwrap_or(0.0);
            features.insert(String::from(chunk), count + 1.0);
        }
    }

    // Normalize
    let max = features
        .values()
        .copied()
        .fold(0.0f32, |a, b| if b > a { b } else { a })
        .max(1.0);
    for val in features.values_mut() {
        *val /= max;
    }

    features
}

/// Extract tag-like keywords from context
fn extract_context_tags(context: &str) -> Vec<String> {
    let lower = context.to_lowercase();
    let mut tags = Vec::new();
    for chunk in lower.split(|c: char| !c.is_alphanumeric()) {
        if chunk.len() >= 4 && !tags.contains(&String::from(chunk)) {
            tags.push(String::from(chunk));
        }
    }
    tags.truncate(20);
    tags
}

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x / 2.0;
    for _ in 0..32 {
        let next = 0.5 * (guess + x / guess);
        if (next - guess).abs() < 1e-7 {
            break;
        }
        guess = next;
    }
    guess
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ENGINE: Mutex<Option<RecommendationEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = RecommendationEngine::new();

    // Seed with some default items for the OS context
    engine.add_item(
        Item::new(
            1,
            "System Monitor",
            "Real-time system performance monitoring",
        )
        .with_features(&[("system", 0.9), ("monitoring", 0.8), ("performance", 0.7)])
        .with_tags(&["system", "tools", "monitoring"])
        .with_popularity(0.8),
    );
    engine.add_item(
        Item::new(2, "File Manager", "Browse and manage files and directories")
            .with_features(&[("files", 0.9), ("management", 0.7), ("browsing", 0.6)])
            .with_tags(&["files", "tools", "productivity"])
            .with_popularity(0.9),
    );
    engine.add_item(
        Item::new(3, "Text Editor", "Edit text files and code")
            .with_features(&[("editing", 0.9), ("code", 0.7), ("text", 0.8)])
            .with_tags(&["editing", "tools", "code", "productivity"])
            .with_popularity(0.85),
    );
    engine.add_item(
        Item::new(
            4,
            "Terminal",
            "Command-line interface for system operations",
        )
        .with_features(&[("command", 0.9), ("system", 0.7), ("shell", 0.8)])
        .with_tags(&["system", "tools", "command-line"])
        .with_popularity(0.75),
    );
    engine.add_item(
        Item::new(5, "Settings", "Configure system preferences and settings")
            .with_features(&[("settings", 0.9), ("configuration", 0.8), ("system", 0.5)])
            .with_tags(&["system", "settings", "configuration"])
            .with_popularity(0.6),
    );

    engine.next_item_id = 6;

    *ENGINE.lock() = Some(engine);
    crate::serial_println!("    [recommendation] Content-based recommendation engine ready (5 items, cosine similarity)");
}

/// Generate recommendations from context
pub fn recommend(context: &str, top_n: usize) -> Vec<Recommendation> {
    ENGINE
        .lock()
        .as_ref()
        .map(|e| e.recommend(context, top_n))
        .unwrap_or_else(Vec::new)
}
