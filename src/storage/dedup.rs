/// Data deduplication engine
///
/// Part of the AIOS storage layer.
///
/// Uses content-addressable storage with 32-byte hashes to identify
/// duplicate blocks. Reference counting ensures blocks are freed
/// only when no references remain.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// An entry in the dedup hash table mapping a content hash to a block address.
struct DedupEntry {
    hash: [u8; 32],
    block: u64,
    ref_count: u32,
}

pub struct DedupTable {
    /// Hash-to-block index with reference counts.
    entries: Vec<DedupEntry>,
    /// Total number of insert requests (including duplicates).
    total_inserts: u64,
    /// Number of inserts that were deduplicated (hash already present).
    dedup_hits: u64,
}

impl DedupTable {
    pub fn new() -> Self {
        DedupTable {
            entries: Vec::new(),
            total_inserts: 0,
            dedup_hits: 0,
        }
    }

    /// Insert a hash-to-block mapping.
    /// Returns `true` if the hash already existed (block was deduplicated),
    /// `false` if this is a new unique block.
    pub fn insert(&mut self, hash: &[u8; 32], block: u64) -> bool {
        self.total_inserts = self.total_inserts.saturating_add(1);

        // Check if hash already exists
        for entry in self.entries.iter_mut() {
            if entry.hash == *hash {
                // Duplicate found: increment reference count
                entry.ref_count = entry.ref_count.saturating_add(1);
                self.dedup_hits = self.dedup_hits.saturating_add(1);
                return true;
            }
        }

        // New unique block
        self.entries.push(DedupEntry {
            hash: *hash,
            block,
            ref_count: 1,
        });
        false
    }

    /// Look up a hash and return the block address if found.
    pub fn lookup(&self, hash: &[u8; 32]) -> Option<u64> {
        for entry in &self.entries {
            if entry.hash == *hash {
                return Some(entry.block);
            }
        }
        None
    }

    /// Return the deduplication ratio as a floating-point value.
    /// A ratio of 2.0 means the effective data is 2x the physical storage.
    /// Returns 1.0 if no deduplication has occurred.
    pub fn dedup_ratio(&self) -> f64 {
        if self.total_inserts == 0 || self.entries.is_empty() {
            return 1.0;
        }
        self.total_inserts as f64 / self.entries.len() as f64
    }

    /// Remove a reference to a hash. If the reference count drops to zero,
    /// the entry is removed and the block can be freed.
    /// Returns `true` if the entry was fully removed (ref_count hit 0).
    pub fn remove_ref(&mut self, hash: &[u8; 32]) -> bool {
        let mut remove_idx = None;
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if entry.hash == *hash {
                entry.ref_count = entry.ref_count.saturating_sub(1);
                if entry.ref_count == 0 {
                    remove_idx = Some(i);
                }
                break;
            }
        }
        if let Some(idx) = remove_idx {
            self.entries.remove(idx);
            true
        } else {
            false
        }
    }

    /// Return the number of unique blocks stored.
    pub fn unique_blocks(&self) -> usize {
        self.entries.len()
    }

    /// Return the total number of logical references.
    pub fn total_refs(&self) -> u64 {
        self.entries.iter().map(|e| e.ref_count as u64).sum()
    }

    /// Return the dedup ratio as a Q16 fixed-point value (ratio * 65536).
    pub fn dedup_ratio_q16(&self) -> i32 {
        if self.entries.is_empty() {
            return 1 << 16;
        }
        let ratio_scaled = (self.total_inserts * (1 << 16)) / self.entries.len() as u64;
        ratio_scaled as i32
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DEDUP_TABLE: Mutex<Option<DedupTable>> = Mutex::new(None);

pub fn init() {
    let mut guard = DEDUP_TABLE.lock();
    *guard = Some(DedupTable::new());
    serial_println!("  [storage] Deduplication engine initialized");
}

/// Access the dedup table under lock.
pub fn with_dedup<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut DedupTable) -> R,
{
    let mut guard = DEDUP_TABLE.lock();
    guard.as_mut().map(f)
}
