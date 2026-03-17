use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// App data storage API
///
/// Part of the Genesis app framework. Provides sandboxed
/// key-value and blob storage scoped to each application.
/// Enforces per-app storage quotas and supports listing keys.
use alloc::string::String;
use alloc::vec::Vec;

/// A stored key-value entry
struct DataEntry {
    key: String,
    value: Vec<u8>,
    created_at: u64,
    modified_at: u64,
}

impl DataEntry {
    fn size(&self) -> usize {
        self.key.len() + self.value.len()
    }
}

/// Monotonic counter for timestamps
static DATA_TICK: Mutex<u64> = Mutex::new(0);

fn tick() -> u64 {
    let mut t = DATA_TICK.lock();
    *t = t.saturating_add(1);
    *t
}

/// Per-app data store
pub struct AppDataStore {
    pub app_id: u64,
    pub quota_bytes: usize,
    pub used_bytes: usize,
    entries: BTreeMap<String, DataEntry>,
    access_count: u64,
}

impl AppDataStore {
    pub fn new(app_id: u64, quota: usize) -> Self {
        let effective_quota = if quota == 0 { 1024 * 1024 } else { quota }; // default 1MB
        crate::serial_println!(
            "[app::data] store created for app {}, quota {} bytes",
            app_id,
            effective_quota
        );
        Self {
            app_id,
            quota_bytes: effective_quota,
            used_bytes: 0,
            entries: BTreeMap::new(),
            access_count: 0,
        }
    }

    /// Store a key-value pair
    pub fn put(&mut self, key: &str, value: &[u8]) -> Result<(), ()> {
        self.access_count = self.access_count.saturating_add(1);

        // Validate key
        if key.is_empty() || key.len() > 256 {
            crate::serial_println!(
                "[app::data] app {}: invalid key length {}",
                self.app_id,
                key.len()
            );
            return Err(());
        }

        // Calculate size delta
        let new_entry_size = key.len() + value.len();
        let existing_size = self.entries.get(key).map(|e| e.size()).unwrap_or(0);
        let size_delta = new_entry_size as isize - existing_size as isize;

        // Check quota
        let projected = self.used_bytes as isize + size_delta;
        if projected > self.quota_bytes as isize {
            crate::serial_println!(
                "[app::data] app {}: quota exceeded ({} + {} > {})",
                self.app_id,
                self.used_bytes,
                size_delta,
                self.quota_bytes
            );
            return Err(());
        }

        let now = tick();
        let mut k = String::new();
        for c in key.chars() {
            k.push(c);
        }

        let mut v = Vec::with_capacity(value.len());
        for b in value {
            v.push(*b);
        }

        let existing = self.entries.contains_key(key);
        let entry = DataEntry {
            key: k.clone(),
            value: v,
            created_at: if existing {
                self.entries.get(key).map(|e| e.created_at).unwrap_or(now)
            } else {
                now
            },
            modified_at: now,
        };

        self.entries.insert(k, entry);
        self.used_bytes = (self.used_bytes as isize + size_delta) as usize;

        crate::serial_println!(
            "[app::data] app {}: put '{}' ({} bytes), used {}/{}",
            self.app_id,
            key,
            value.len(),
            self.used_bytes,
            self.quota_bytes
        );
        Ok(())
    }

    /// Retrieve a value by key
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        match self.entries.get(key) {
            Some(entry) => {
                let mut result = Vec::with_capacity(entry.value.len());
                for b in &entry.value {
                    result.push(*b);
                }
                Some(result)
            }
            None => None,
        }
    }

    /// Delete a key and reclaim space
    pub fn delete(&mut self, key: &str) -> bool {
        self.access_count = self.access_count.saturating_add(1);
        match self.entries.remove(key) {
            Some(entry) => {
                let freed = entry.size();
                self.used_bytes = self.used_bytes.saturating_sub(freed);
                crate::serial_println!(
                    "[app::data] app {}: deleted '{}', freed {} bytes",
                    self.app_id,
                    key,
                    freed
                );
                true
            }
            None => false,
        }
    }

    /// List all keys in this store
    pub fn keys(&self) -> Vec<&str> {
        let mut result = Vec::new();
        for (k, _) in &self.entries {
            result.push(k.as_str());
        }
        result
    }

    /// Check if a key exists
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Get the number of stored entries
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get remaining quota in bytes
    pub fn remaining_quota(&self) -> usize {
        self.quota_bytes.saturating_sub(self.used_bytes)
    }

    /// Clear all data in this store
    pub fn clear(&mut self) {
        self.entries.clear();
        self.used_bytes = 0;
        crate::serial_println!("[app::data] app {}: store cleared", self.app_id);
    }

    /// Get storage usage as a percentage (0..100)
    pub fn usage_pct(&self) -> u8 {
        if self.quota_bytes == 0 {
            return 0;
        }
        ((self.used_bytes as u64 * 100) / self.quota_bytes as u64) as u8
    }
}

/// Global registry of app data stores
static DATA_REGISTRY: Mutex<Option<BTreeMap<u64, AppDataStore>>> = Mutex::new(None);

pub fn init() {
    let mut reg = DATA_REGISTRY.lock();
    *reg = Some(BTreeMap::new());
    crate::serial_println!("[app::data] app data subsystem initialized");
}

/// Get or create a data store for an app
pub fn get_store(app_id: u64) -> Option<u64> {
    let mut reg = DATA_REGISTRY.lock();
    if let Some(ref mut map) = *reg {
        if !map.contains_key(&app_id) {
            let store = AppDataStore::new(app_id, 1024 * 1024);
            map.insert(app_id, store);
        }
        Some(app_id)
    } else {
        None
    }
}
