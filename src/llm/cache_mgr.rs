use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::string::String;
/// Model cache management -- multiple models in memory
///
/// Part of the AIOS LLM layer. Manages the loading and eviction of
/// model weight tensors under a fixed memory budget. Models are tracked
/// by name and evicted with an LRU (Least Recently Used) policy when
/// memory pressure requires it.
///
/// Each loaded model occupies a contiguous region of the budget.
/// The manager tracks:
///   - which models are loaded and their sizes
///   - access timestamps for LRU ordering
///   - pinned models that cannot be evicted
///   - fragmentation statistics
use alloc::vec::Vec;

/// State of a model slot in the cache
#[derive(Clone)]
struct ModelSlot {
    /// Model name/path
    name: String,
    /// Memory consumed (bytes)
    size: usize,
    /// Monotonic access timestamp
    last_access: u64,
    /// Number of times this model has been accessed
    access_count: u64,
    /// If true, this model cannot be evicted
    pinned: bool,
    /// Load timestamp
    loaded_at: u64,
}

/// Tracks loaded models and manages memory budget
pub struct CacheManager {
    /// Names of loaded models (public for compatibility)
    pub loaded_models: Vec<String>,
    /// Total memory budget in bytes
    pub memory_budget: usize,
    /// Currently used memory in bytes
    pub memory_used: usize,
    /// Internal slot tracking
    slots: Vec<ModelSlot>,
    /// Monotonic counter for LRU timestamps
    counter: u64,
    /// Total models loaded since init
    total_loads: u64,
    /// Total evictions
    total_evictions: u64,
    /// Maximum number of models that can be loaded simultaneously
    max_models: usize,
}

impl CacheManager {
    /// Create a new cache manager with the given memory budget (in bytes).
    pub fn new(memory_budget: usize) -> Self {
        serial_println!(
            "    [cache-mgr] Creating cache manager: budget={}KB",
            memory_budget / 1024
        );
        CacheManager {
            loaded_models: Vec::new(),
            memory_budget,
            memory_used: 0,
            slots: Vec::new(),
            counter: 0,
            total_loads: 0,
            total_evictions: 0,
            max_models: 32,
        }
    }

    /// Attempt to load a model into the cache.
    ///
    /// `path` is the model identifier/path.
    /// Returns true if the model was loaded (or was already loaded),
    /// false if it could not fit even after evictions.
    pub fn load_model(&mut self, path: &str) -> bool {
        self.load_model_with_size(path, self.estimate_model_size(path))
    }

    /// Load a model with an explicit size.
    pub fn load_model_with_size(&mut self, path: &str, size: usize) -> bool {
        // Check if already loaded
        for (_i, slot) in self.slots.iter_mut().enumerate() {
            if slot.name == path {
                self.counter = self.counter.saturating_add(1);
                slot.last_access = self.counter;
                slot.access_count = slot.access_count.saturating_add(1);
                serial_println!(
                    "    [cache-mgr] '{}' already loaded (access #{})",
                    path,
                    slot.access_count
                );
                return true;
            }
        }

        // Check max model limit
        if self.slots.len() >= self.max_models {
            self.evict_lru();
        }

        // Evict until we have room
        while self.memory_used + size > self.memory_budget {
            if !self.evict_lru() {
                serial_println!(
                    "    [cache-mgr] Cannot fit '{}' ({}KB): all models pinned or budget too small",
                    path,
                    size / 1024
                );
                return false;
            }
        }

        // Load the model
        self.counter = self.counter.saturating_add(1);
        self.total_loads = self.total_loads.saturating_add(1);
        let slot = ModelSlot {
            name: String::from(path),
            size,
            last_access: self.counter,
            access_count: 1,
            pinned: false,
            loaded_at: self.counter,
        };
        self.loaded_models.push(String::from(path));
        self.slots.push(slot);
        self.memory_used += size;

        serial_println!(
            "    [cache-mgr] Loaded '{}' ({}KB), usage: {}KB/{}KB",
            path,
            size / 1024,
            self.memory_used / 1024,
            self.memory_budget / 1024
        );
        true
    }

    /// Evict the least recently used (unpinned) model.
    /// Returns true if a model was evicted, false if none could be.
    pub fn evict_lru(&mut self) -> bool {
        if self.slots.is_empty() {
            return false;
        }

        // Find the unpinned slot with the oldest last_access
        let mut victim_idx: Option<usize> = None;
        let mut oldest_access = u64::MAX;

        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.pinned && slot.last_access < oldest_access {
                oldest_access = slot.last_access;
                victim_idx = Some(i);
            }
        }

        if let Some(idx) = victim_idx {
            let evicted = self.slots.remove(idx);
            self.memory_used = self.memory_used.saturating_sub(evicted.size);
            self.total_evictions = self.total_evictions.saturating_add(1);

            // Remove from loaded_models list
            self.loaded_models.retain(|n| n != &evicted.name);

            serial_println!(
                "    [cache-mgr] Evicted '{}' ({}KB), freed to {}KB/{}KB",
                evicted.name,
                evicted.size / 1024,
                self.memory_used / 1024,
                self.memory_budget / 1024
            );
            true
        } else {
            serial_println!("    [cache-mgr] No evictable models (all pinned)");
            false
        }
    }

    /// Pin a model so it cannot be evicted.
    pub fn pin_model(&mut self, path: &str) -> bool {
        for slot in self.slots.iter_mut() {
            if slot.name == path {
                slot.pinned = true;
                serial_println!("    [cache-mgr] Pinned '{}'", path);
                return true;
            }
        }
        false
    }

    /// Unpin a model, allowing it to be evicted.
    pub fn unpin_model(&mut self, path: &str) -> bool {
        for slot in self.slots.iter_mut() {
            if slot.name == path {
                slot.pinned = false;
                serial_println!("    [cache-mgr] Unpinned '{}'", path);
                return true;
            }
        }
        false
    }

    /// Explicitly unload a model (even if pinned).
    pub fn unload_model(&mut self, path: &str) -> bool {
        let mut found_idx = None;
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.name == path {
                found_idx = Some(i);
                break;
            }
        }
        if let Some(idx) = found_idx {
            let removed = self.slots.remove(idx);
            self.memory_used = self.memory_used.saturating_sub(removed.size);
            self.loaded_models.retain(|n| n != path);
            serial_println!("    [cache-mgr] Unloaded '{}'", path);
            true
        } else {
            false
        }
    }

    /// Touch a model (update its access timestamp without reloading).
    pub fn touch(&mut self, path: &str) -> bool {
        self.counter = self.counter.saturating_add(1);
        let ts = self.counter;
        for slot in self.slots.iter_mut() {
            if slot.name == path {
                slot.last_access = ts;
                slot.access_count = slot.access_count.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Check if a model is loaded.
    pub fn is_loaded(&self, path: &str) -> bool {
        self.slots.iter().any(|s| s.name == path)
    }

    /// Get the number of loaded models.
    pub fn count(&self) -> usize {
        self.slots.len()
    }

    /// Get remaining free memory.
    pub fn free_memory(&self) -> usize {
        self.memory_budget.saturating_sub(self.memory_used)
    }

    /// Get utilisation percentage (0-100).
    pub fn utilisation_pct(&self) -> u32 {
        if self.memory_budget == 0 {
            return 0;
        }
        ((self.memory_used as u64 * 100) / self.memory_budget as u64) as u32
    }

    /// Get cache hit rate: accesses beyond first load / total accesses.
    pub fn hit_rate_pct(&self) -> u32 {
        let total_accesses: u64 = self.slots.iter().map(|s| s.access_count).sum();
        if total_accesses == 0 {
            return 0;
        }
        let first_loads = self.slots.len() as u64;
        let hits = total_accesses.saturating_sub(first_loads);
        ((hits * 100) / total_accesses) as u32
    }

    /// Estimate model size based on the name (heuristic).
    /// In a real system this would read metadata from disk.
    fn estimate_model_size(&self, path: &str) -> usize {
        // Simple heuristic: look for size hints in the name
        let name = path.to_ascii_lowercase();
        if name.contains("70b") || name.contains("65b") {
            140 * 1024 * 1024 * 1024 // ~140GB for 70B model
        } else if name.contains("13b") || name.contains("14b") {
            26 * 1024 * 1024 * 1024
        } else if name.contains("7b") || name.contains("8b") {
            14 * 1024 * 1024 * 1024
        } else if name.contains("3b") {
            6 * 1024 * 1024 * 1024
        } else if name.contains("1b") {
            2 * 1024 * 1024 * 1024
        } else {
            // Default: assume a small model (~100MB)
            100 * 1024 * 1024
        }
    }

    /// Get a sorted list of models by last access time (most recent first).
    pub fn models_by_recency(&self) -> Vec<&str> {
        let mut sorted: Vec<&ModelSlot> = self.slots.iter().collect();
        sorted.sort_by(|a, b| b.last_access.cmp(&a.last_access));
        sorted.iter().map(|s| s.name.as_str()).collect()
    }

    /// Get summary stats.
    pub fn stats_summary(&self) -> (usize, usize, usize, u64, u64) {
        (
            self.slots.len(),
            self.memory_used,
            self.memory_budget,
            self.total_loads,
            self.total_evictions,
        )
    }
}

// Helper for case conversion without std
trait AsciiLowercase {
    fn to_ascii_lowercase(&self) -> String;
}

impl AsciiLowercase for str {
    fn to_ascii_lowercase(&self) -> String {
        let mut s = String::with_capacity(self.len());
        for ch in self.chars() {
            if ch.is_ascii_uppercase() {
                s.push((ch as u8 + 32) as char);
            } else {
                s.push(ch);
            }
        }
        s
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct CacheMgrState {
    manager: CacheManager,
}

static CACHE_MGR: Mutex<Option<CacheMgrState>> = Mutex::new(None);

/// Default memory budget: 4 GB
const DEFAULT_BUDGET: usize = 4 * 1024 * 1024 * 1024;

pub fn init() {
    let manager = CacheManager::new(DEFAULT_BUDGET);
    let mut guard = CACHE_MGR.lock();
    *guard = Some(CacheMgrState { manager });
    serial_println!(
        "    [cache-mgr] Subsystem initialised (budget={}GB)",
        DEFAULT_BUDGET / (1024 * 1024 * 1024)
    );
}

/// Load a model via the global cache manager.
pub fn load_global(path: &str) -> bool {
    let mut guard = CACHE_MGR.lock();
    if let Some(state) = guard.as_mut() {
        state.manager.load_model(path)
    } else {
        false
    }
}

/// Evict LRU from the global cache.
pub fn evict_global() -> bool {
    let mut guard = CACHE_MGR.lock();
    if let Some(state) = guard.as_mut() {
        state.manager.evict_lru()
    } else {
        false
    }
}
