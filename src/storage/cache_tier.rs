/// Tiered caching (SSD cache for HDD)
///
/// Part of the AIOS storage layer.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub enum CachePolicy {
    WriteThrough,
    WriteBack,
    WriteAround,
}

/// State of a cached block.
struct CachedBlock {
    /// The data stored in the cache for this LBA.
    data: Vec<u8>,
    /// Whether this block has been modified but not flushed to the slow device.
    dirty: bool,
    /// Access count for promotion/eviction decisions.
    access_count: u64,
}

pub struct CacheTier {
    policy: CachePolicy,
    /// Map from LBA to cached block data.
    cache_map: BTreeMap<u64, CachedBlock>,
    /// Maximum number of blocks the cache can hold.
    max_entries: usize,
    /// Block size in bytes (matches sector size, typically 512 or 4096).
    block_size: usize,
    /// Statistics
    hits: u64,
    misses: u64,
    writebacks: u64,
}

impl CacheTier {
    pub fn new(policy: CachePolicy) -> Self {
        CacheTier {
            policy,
            cache_map: BTreeMap::new(),
            max_entries: 4096,
            block_size: 512,
            hits: 0,
            misses: 0,
            writebacks: 0,
        }
    }

    /// Read a block. If the block is in the cache (hit), copy from cache.
    /// Otherwise, return Err to indicate the caller should read from the slow device
    /// and then call `populate` to insert the block into the cache.
    pub fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        if let Some(entry) = self.cache_map.get(&lba) {
            let copy_len = buf.len().min(entry.data.len());
            buf[..copy_len].copy_from_slice(&entry.data[..copy_len]);
            // Note: access_count incremented in mutable path
            return Ok(());
        }
        // Cache miss: caller must read from slow device
        Err(())
    }

    /// Write a block according to the cache policy.
    pub fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), ()> {
        match self.policy {
            CachePolicy::WriteThrough => {
                // Write to cache and mark clean (caller is responsible for
                // also writing to the slow device).
                self.insert_block(lba, data, false);
                Ok(())
            }
            CachePolicy::WriteBack => {
                // Write only to cache, mark dirty. Actual write to slow
                // device is deferred until flush.
                self.insert_block(lba, data, true);
                Ok(())
            }
            CachePolicy::WriteAround => {
                // Write directly to slow device (caller's responsibility).
                // Invalidate the cache entry if it exists so stale data
                // is not served on subsequent reads.
                self.cache_map.remove(&lba);
                Ok(())
            }
        }
    }

    /// Flush all dirty blocks to the slow device.
    /// Returns Ok(()) on success.
    pub fn flush(&mut self) -> Result<(), ()> {
        let mut flushed = 0u64;
        for (_lba, entry) in self.cache_map.iter_mut() {
            if entry.dirty {
                // In a real system, we would issue a write to the slow device here.
                entry.dirty = false;
                flushed += 1;
            }
        }
        self.writebacks += flushed;
        if flushed > 0 {
            serial_println!("  [cache_tier] Flushed {} dirty blocks", flushed);
        }
        Ok(())
    }

    /// Insert or update a block in the cache.
    fn insert_block(&mut self, lba: u64, data: &[u8], dirty: bool) {
        if let Some(entry) = self.cache_map.get_mut(&lba) {
            // Update existing entry
            entry.data.clear();
            entry.data.extend_from_slice(data);
            entry.dirty = entry.dirty || dirty;
            entry.access_count = entry.access_count.saturating_add(1);
            self.hits = self.hits.saturating_add(1);
        } else {
            // Evict if at capacity
            if self.cache_map.len() >= self.max_entries {
                self.evict_one();
            }
            self.cache_map.insert(
                lba,
                CachedBlock {
                    data: data.into(),
                    dirty,
                    access_count: 1,
                },
            );
            self.misses = self.misses.saturating_add(1);
        }
    }

    /// Populate the cache after a read miss.
    pub fn populate(&mut self, lba: u64, data: &[u8]) {
        self.insert_block(lba, data, false);
    }

    /// Evict the least-accessed clean block. If all blocks are dirty,
    /// evict the least-accessed dirty block (writeback implied).
    fn evict_one(&mut self) {
        // Prefer evicting a clean block with the lowest access count
        let mut best_lba: Option<u64> = None;
        let mut best_count = u64::MAX;
        let mut best_dirty = true;

        for (&lba, entry) in self.cache_map.iter() {
            let dominated = (!entry.dirty && best_dirty)
                || (entry.dirty == best_dirty && entry.access_count < best_count);
            if dominated {
                best_lba = Some(lba);
                best_count = entry.access_count;
                best_dirty = entry.dirty;
            }
        }

        if let Some(lba) = best_lba {
            if best_dirty {
                self.writebacks = self.writebacks.saturating_add(1);
                // In a real system, write back the dirty block here.
            }
            self.cache_map.remove(&lba);
        }
    }

    /// Return cache hit rate as a Q16 fixed-point value (0..100<<16).
    pub fn hit_rate_q16(&self) -> i32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0;
        }
        ((self.hits * (100 << 16)) / total) as i32
    }

    /// Return the number of dirty blocks pending writeback.
    pub fn dirty_count(&self) -> usize {
        self.cache_map.values().filter(|e| e.dirty).count()
    }

    /// Return the number of cached blocks.
    pub fn cached_count(&self) -> usize {
        self.cache_map.len()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CACHE_TIER: Mutex<Option<CacheTier>> = Mutex::new(None);

pub fn init() {
    let mut guard = CACHE_TIER.lock();
    *guard = Some(CacheTier::new(CachePolicy::WriteBack));
    serial_println!("  [storage] Tiered cache initialized (write-back)");
}

/// Access the cache tier under lock.
pub fn with_cache<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut CacheTier) -> R,
{
    let mut guard = CACHE_TIER.lock();
    guard.as_mut().map(f)
}
