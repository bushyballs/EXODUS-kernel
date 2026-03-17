use crate::serial_println;
use crate::sync::Mutex;
/// Directory entry cache (dcache) for fast path lookups
///
/// Part of the AIOS filesystem layer.
///
/// Provides an LRU-evicting cache of (parent_inode, name) -> inode mappings
/// so that repeated path traversals avoid hitting the underlying filesystem.
///
/// Design:
///   - A flat Vec acts as the hash table (open addressing, linear probe).
///   - Each bucket stores the entry plus an LRU generation counter.
///   - On lookup hit the generation is bumped; on insert the lowest-generation
///     bucket in the probe chain is evicted when full.
///   - A global Mutex<Option<Inner>> guards the singleton instance.
///
/// Inspired by: Linux dcache (fs/dcache.c). All code is original.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of buckets (must be power of two)
const DEFAULT_CAPACITY: usize = 1024;

/// Maximum linear-probe distance before giving up
const MAX_PROBE: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single cached directory entry.
#[derive(Clone)]
pub struct DentryEntry {
    pub name: String,
    pub inode_nr: u64,
    pub parent: u64,
}

/// Bucket states inside the hash table.
#[derive(Clone)]
enum Bucket {
    Empty,
    Occupied {
        entry: DentryEntry,
        generation: u64,
    },
    /// Tombstone left after invalidation so probe chains still work.
    Deleted,
}

/// Internal cache state behind the global mutex.
struct Inner {
    buckets: Vec<Bucket>,
    capacity: usize,
    count: usize,
    generation: u64,
    hits: u64,
    misses: u64,
}

// ---------------------------------------------------------------------------
// Hashing (FNV-1a, no_std friendly)
// ---------------------------------------------------------------------------

fn fnv1a(parent: u64, name: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    // Mix parent inode
    for b in parent.to_le_bytes().iter() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    // Mix name bytes
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as usize
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new(capacity: usize) -> Self {
        let cap = capacity.next_power_of_two().max(64);
        let mut buckets = Vec::with_capacity(cap);
        for _ in 0..cap {
            buckets.push(Bucket::Empty);
        }
        Inner {
            buckets,
            capacity: cap,
            count: 0,
            generation: 1,
            hits: 0,
            misses: 0,
        }
    }

    fn mask(&self) -> usize {
        self.capacity - 1
    }

    fn next_gen(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Look up (parent, name). Returns inode number on hit.
    fn lookup(&mut self, parent: u64, name: &str) -> Option<u64> {
        let start = fnv1a(parent, name) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            let found = match &self.buckets[idx] {
                Bucket::Occupied { entry, .. } => {
                    if entry.parent == parent && entry.name == name {
                        Some(entry.inode_nr)
                    } else {
                        None
                    }
                }
                Bucket::Empty => {
                    self.misses = self.misses.saturating_add(1);
                    return None;
                }
                Bucket::Deleted => {
                    // Continue probing past tombstones
                    None
                }
            };
            if let Some(inode_nr) = found {
                // Bump generation on hit (immutable borrow is now dropped)
                let gen = self.next_gen();
                if let Bucket::Occupied { generation, .. } = &mut self.buckets[idx] {
                    *generation = gen;
                }
                self.hits = self.hits.saturating_add(1);
                return Some(inode_nr);
            }
        }
        self.misses = self.misses.saturating_add(1);
        None
    }

    /// Insert an entry, evicting the lowest-generation bucket if needed.
    fn insert(&mut self, entry: DentryEntry) {
        let start = fnv1a(entry.parent, &entry.name) & self.mask();
        let gen = self.next_gen();

        // First pass: look for empty/deleted slot or duplicate
        let mut lowest_gen = u64::MAX;
        let mut lowest_idx = start;
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Empty | Bucket::Deleted => {
                    self.buckets[idx] = Bucket::Occupied {
                        entry,
                        generation: gen,
                    };
                    self.count = self.count.saturating_add(1);
                    return;
                }
                Bucket::Occupied {
                    entry: existing,
                    generation: g,
                } => {
                    // Update in place if same key
                    if existing.parent == entry.parent && existing.name == entry.name {
                        self.buckets[idx] = Bucket::Occupied {
                            entry,
                            generation: gen,
                        };
                        return;
                    }
                    if *g < lowest_gen {
                        lowest_gen = *g;
                        lowest_idx = idx;
                    }
                }
            }
        }

        // All slots occupied in probe chain -- evict LRU
        self.buckets[lowest_idx] = Bucket::Occupied {
            entry,
            generation: gen,
        };
        // count stays the same (replaced)
    }

    /// Remove an entry matching (parent, name).
    fn invalidate(&mut self, parent: u64, name: &str) {
        let start = fnv1a(parent, name) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Occupied { entry, .. } => {
                    if entry.parent == parent && entry.name == name {
                        self.buckets[idx] = Bucket::Deleted;
                        self.count = self.count.saturating_sub(1);
                        return;
                    }
                }
                Bucket::Empty => return,
                Bucket::Deleted => {}
            }
        }
    }

    /// Invalidate every entry whose parent matches.
    fn invalidate_children(&mut self, parent: u64) {
        for i in 0..self.capacity {
            let should_delete = match &self.buckets[i] {
                Bucket::Occupied { entry, .. } => entry.parent == parent,
                _ => false,
            };
            if should_delete {
                self.buckets[i] = Bucket::Deleted;
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    /// Flush the entire cache.
    fn flush(&mut self) {
        for i in 0..self.capacity {
            self.buckets[i] = Bucket::Empty;
        }
        self.count = 0;
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static DCACHE: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a (parent_inode, name) pair from the cache.
/// Returns `Some(inode_nr)` on hit, `None` on miss.
pub fn lookup(parent: u64, name: &str) -> Option<u64> {
    let mut guard = DCACHE.lock();
    guard.as_mut().and_then(|inner| inner.lookup(parent, name))
}

/// Insert or update a dentry in the cache.
pub fn insert(entry: DentryEntry) {
    let mut guard = DCACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.insert(entry);
    }
}

/// Invalidate a single dentry.
pub fn invalidate(parent: u64, name: &str) {
    let mut guard = DCACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.invalidate(parent, name);
    }
}

/// Invalidate all children of a directory inode.
pub fn invalidate_children(parent: u64) {
    let mut guard = DCACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.invalidate_children(parent);
    }
}

/// Flush the entire dentry cache.
pub fn flush() {
    let mut guard = DCACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.flush();
    }
}

/// Return (hits, misses) for diagnostic reporting.
pub fn stats() -> (u64, u64) {
    let guard = DCACHE.lock();
    match guard.as_ref() {
        Some(inner) => (inner.hits, inner.misses),
        None => (0, 0),
    }
}

/// Accelerated multi-component path lookup.
/// Walks each component through the cache, returning the final inode number
/// or `None` if any component misses.
pub fn resolve_path(root_ino: u64, path: &str) -> Option<u64> {
    let mut current = root_ino;
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        match lookup(current, component) {
            Some(next) => current = next,
            None => return None,
        }
    }
    Some(current)
}

/// Initialize the dentry cache subsystem.
pub fn init() {
    let mut guard = DCACHE.lock();
    *guard = Some(Inner::new(DEFAULT_CAPACITY));
    serial_println!("    dcache: initialized ({} buckets)", DEFAULT_CAPACITY);
}
