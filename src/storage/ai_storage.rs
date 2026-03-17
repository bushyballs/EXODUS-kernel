/// AI-powered storage for Genesis
///
/// Smart cleanup, storage optimization, content indexing,
/// usage prediction, intelligent caching, auto-archive.
///
/// Inspired by: Android Smart Storage, iOS Storage Optimization. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Storage cleanup recommendation
pub struct CleanupRecommendation {
    pub category: CleanupCategory,
    pub description: String,
    pub reclaimable_bytes: u64,
    pub impact: CleanupImpact,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupCategory {
    AppCache,
    DownloadedFiles,
    DuplicatePhotos,
    LargeFiles,
    OldScreenshots,
    UnusedApps,
    TempFiles,
    OldBackups,
    EmptyFolders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupImpact {
    None,   // Safe to delete, no user impact
    Low,    // Slight inconvenience
    Medium, // Some data may need re-download
    High,   // User data at risk
}

/// Cache optimization strategy
pub struct CacheStrategy {
    pub app_name: String,
    pub current_size_mb: u32,
    pub recommended_size_mb: u32,
    pub priority: CachePriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePriority {
    Critical, // Keep always (maps, offline content)
    High,     // Keep if space available
    Normal,   // Standard LRU eviction
    Low,      // Evict first under pressure
}

/// AI storage engine
pub struct AiStorageEngine {
    pub enabled: bool,
    pub total_space_mb: u64,
    pub used_space_mb: u64,
    pub app_sizes: BTreeMap<String, u64>,
    pub cache_sizes: BTreeMap<String, u64>,
    pub access_history: BTreeMap<String, Vec<u64>>,
    pub cleanup_history: Vec<(u64, u64)>, // (timestamp, bytes_freed)
    pub auto_cleanup_enabled: bool,
    pub low_space_threshold_mb: u64,
    pub total_cleaned_bytes: u64,
}

impl AiStorageEngine {
    const fn new() -> Self {
        AiStorageEngine {
            enabled: true,
            total_space_mb: 128_000,
            used_space_mb: 0,
            app_sizes: BTreeMap::new(),
            cache_sizes: BTreeMap::new(),
            access_history: BTreeMap::new(),
            cleanup_history: Vec::new(),
            auto_cleanup_enabled: true,
            low_space_threshold_mb: 2048,
            total_cleaned_bytes: 0,
        }
    }

    /// Get cleanup recommendations
    pub fn get_recommendations(&self) -> Vec<CleanupRecommendation> {
        let mut recs = Vec::new();

        // Large caches
        for (app, size) in &self.cache_sizes {
            if *size > 100 {
                recs.push(CleanupRecommendation {
                    category: CleanupCategory::AppCache,
                    description: alloc::format!("{} cache: {} MB", app, size),
                    reclaimable_bytes: *size * 1024 * 1024,
                    impact: CleanupImpact::Low,
                    confidence: 0.9,
                });
            }
        }

        // Unused apps (not accessed in 30 days)
        let now = crate::time::clock::unix_time();
        for (app, history) in &self.access_history {
            if let Some(last) = history.last() {
                if now - last > 30 * 86400 {
                    let size = self.app_sizes.get(app).copied().unwrap_or(50);
                    recs.push(CleanupRecommendation {
                        category: CleanupCategory::UnusedApps,
                        description: alloc::format!("{} not used in 30+ days ({} MB)", app, size),
                        reclaimable_bytes: size * 1024 * 1024,
                        impact: CleanupImpact::Medium,
                        confidence: 0.8,
                    });
                }
            }
        }

        recs.sort_by(|a, b| b.reclaimable_bytes.cmp(&a.reclaimable_bytes));
        recs
    }

    /// Get intelligent cache strategies
    pub fn optimize_caches(&self) -> Vec<CacheStrategy> {
        let now = crate::time::clock::unix_time();
        self.cache_sizes
            .iter()
            .map(|(app, size)| {
                let recent_access = self
                    .access_history
                    .get(app)
                    .and_then(|h| h.last())
                    .map(|t| now - t < 86400)
                    .unwrap_or(false);

                let access_freq = self
                    .access_history
                    .get(app)
                    .map(|h| h.len() as u32)
                    .unwrap_or(0);

                let priority = if recent_access && access_freq > 10 {
                    CachePriority::High
                } else if recent_access {
                    CachePriority::Normal
                } else {
                    CachePriority::Low
                };

                let recommended = match priority {
                    CachePriority::Critical => *size as u32,
                    CachePriority::High => (*size as u32 * 80) / 100,
                    CachePriority::Normal => (*size as u32 * 50) / 100,
                    CachePriority::Low => (*size as u32 * 20) / 100,
                };

                CacheStrategy {
                    app_name: app.clone(),
                    current_size_mb: *size as u32,
                    recommended_size_mb: recommended.max(10),
                    priority,
                }
            })
            .collect()
    }

    /// Record app storage usage
    pub fn record_usage(&mut self, app: &str, size_mb: u64, cache_mb: u64) {
        self.app_sizes.insert(String::from(app), size_mb);
        self.cache_sizes.insert(String::from(app), cache_mb);
        let now = crate::time::clock::unix_time();
        self.access_history
            .entry(String::from(app))
            .or_insert_with(Vec::new)
            .push(now);
    }

    /// Estimate when storage will be full
    pub fn predict_full_date(&self) -> Option<u64> {
        if self.cleanup_history.len() < 2 {
            return None;
        }
        let free = self.total_space_mb.saturating_sub(self.used_space_mb);
        if free == 0 {
            return Some(crate::time::clock::unix_time());
        }

        // Simple growth rate estimation
        let growth_rate_mb_per_day = 100; // Placeholder
        let days_remaining = free / growth_rate_mb_per_day;
        Some(crate::time::clock::unix_time() + days_remaining * 86400)
    }

    /// Is storage critically low?
    pub fn is_low_space(&self) -> bool {
        self.total_space_mb.saturating_sub(self.used_space_mb) < self.low_space_threshold_mb
    }

    /// Get total reclaimable space from recommendations
    pub fn total_reclaimable(&self) -> u64 {
        self.get_recommendations()
            .iter()
            .map(|r| r.reclaimable_bytes)
            .sum()
    }
}

static AI_STORAGE: Mutex<AiStorageEngine> = Mutex::new(AiStorageEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-storage] AI storage initialized (cleanup, cache, optimization)"
    );
}

pub fn get_recommendations() -> Vec<CleanupRecommendation> {
    AI_STORAGE.lock().get_recommendations()
}

pub fn record_usage(app: &str, size_mb: u64, cache_mb: u64) {
    AI_STORAGE.lock().record_usage(app, size_mb, cache_mb);
}

pub fn is_low_space() -> bool {
    AI_STORAGE.lock().is_low_space()
}
