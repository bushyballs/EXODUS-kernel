use crate::serial_println;
use crate::sync::Mutex;
/// Inode cache (icache) for fast inode lookups with dirty tracking
///
/// Part of the AIOS filesystem layer.
///
/// Caches in-memory representations of on-disk inodes so that repeated
/// access to the same inode avoids re-reading from the block device.
///
/// Design:
///   - Hash table (open addressing) keyed by (device_id, inode_number).
///   - Each entry carries metadata (size, mode, links, uid, gid, timestamps)
///     and a dirty flag.
///   - LRU eviction based on a generation counter.
///   - `flush_dirty()` collects all dirty entries for writeback.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux inode cache (fs/inode.c). All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_CAPACITY: usize = 512;
const MAX_PROBE: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A cached inode with full metadata.
#[derive(Clone)]
pub struct CachedInode {
    pub dev: u32,
    pub ino: u64,
    pub size: u64,
    pub mode: u32,
    pub nlinks: u32,
    pub uid: u32,
    pub gid: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub blocks: u64,
    pub dirty: bool,
    pub ref_count: u32,
}

impl CachedInode {
    /// Create a new cached inode with default timestamps.
    pub fn new(dev: u32, ino: u64, size: u64, mode: u32) -> Self {
        CachedInode {
            dev,
            ino,
            size,
            mode,
            nlinks: 1,
            uid: 0,
            gid: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            blocks: (size + 4095) / 4096,
            dirty: false,
            ref_count: 1,
        }
    }

    /// Mark this inode as dirty (needs writeback).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Bucket in the hash table.
#[derive(Clone)]
enum Bucket {
    Empty,
    Occupied { inode: CachedInode, generation: u64 },
    Deleted,
}

/// Inner cache state.
struct Inner {
    buckets: Vec<Bucket>,
    capacity: usize,
    count: usize,
    generation: u64,
    dirty_count: usize,
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

fn hash_key(dev: u32, ino: u64) -> usize {
    let mut h: u64 = 0x517cc1b727220a95;
    for b in dev.to_le_bytes().iter() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    for b in ino.to_le_bytes().iter() {
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
            dirty_count: 0,
        }
    }

    fn mask(&self) -> usize {
        self.capacity - 1
    }

    fn next_gen(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Look up an inode by (dev, ino). Returns a clone on hit.
    fn get(&mut self, dev: u32, ino: u64) -> Option<CachedInode> {
        let start = hash_key(dev, ino) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            let found = match &self.buckets[idx] {
                Bucket::Occupied { inode, .. } => {
                    if inode.dev == dev && inode.ino == ino {
                        Some(inode.clone())
                    } else {
                        None
                    }
                }
                Bucket::Empty => return None,
                Bucket::Deleted => None,
            };
            if let Some(result) = found {
                // Immutable borrow is now dropped, safe to mutate
                let gen = self.next_gen();
                if let Bucket::Occupied { generation, .. } = &mut self.buckets[idx] {
                    *generation = gen;
                }
                return Some(result);
            }
        }
        None
    }

    /// Insert or update an inode in the cache.
    fn insert(&mut self, inode: CachedInode) {
        let dev = inode.dev;
        let ino = inode.ino;
        let is_dirty = inode.dirty;
        let start = hash_key(dev, ino) & self.mask();
        let gen = self.next_gen();

        let mut lowest_gen = u64::MAX;
        let mut lowest_idx = start;

        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Empty | Bucket::Deleted => {
                    self.buckets[idx] = Bucket::Occupied {
                        inode,
                        generation: gen,
                    };
                    self.count = self.count.saturating_add(1);
                    if is_dirty {
                        self.dirty_count = self.dirty_count.saturating_add(1);
                    }
                    return;
                }
                Bucket::Occupied {
                    inode: existing,
                    generation: g,
                } => {
                    if existing.dev == dev && existing.ino == ino {
                        // Update in place
                        let was_dirty = existing.dirty;
                        self.buckets[idx] = Bucket::Occupied {
                            inode,
                            generation: gen,
                        };
                        if is_dirty && !was_dirty {
                            self.dirty_count = self.dirty_count.saturating_add(1);
                        } else if !is_dirty && was_dirty {
                            self.dirty_count = self.dirty_count.saturating_sub(1);
                        }
                        return;
                    }
                    if *g < lowest_gen {
                        lowest_gen = *g;
                        lowest_idx = idx;
                    }
                }
            }
        }

        // Evict lowest-generation entry
        if let Bucket::Occupied { inode: old, .. } = &self.buckets[lowest_idx] {
            if old.dirty {
                self.dirty_count = self.dirty_count.saturating_sub(1);
            }
        }
        self.buckets[lowest_idx] = Bucket::Occupied {
            inode,
            generation: gen,
        };
        if is_dirty {
            self.dirty_count = self.dirty_count.saturating_add(1);
        }
    }

    /// Remove an inode from the cache.
    fn evict(&mut self, dev: u32, ino: u64) {
        let start = hash_key(dev, ino) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            let should_delete = match &self.buckets[idx] {
                Bucket::Occupied { inode, .. } => inode.dev == dev && inode.ino == ino,
                _ => false,
            };
            if should_delete {
                if let Bucket::Occupied { inode, .. } = &self.buckets[idx] {
                    if inode.dirty {
                        self.dirty_count = self.dirty_count.saturating_sub(1);
                    }
                }
                self.buckets[idx] = Bucket::Deleted;
                self.count = self.count.saturating_sub(1);
                return;
            }
            if let Bucket::Empty = &self.buckets[idx] {
                return;
            }
        }
    }

    /// Collect all dirty inodes for writeback, clearing their dirty flags.
    fn flush_dirty(&mut self) -> Vec<CachedInode> {
        let mut dirty = Vec::new();
        for bucket in self.buckets.iter_mut() {
            if let Bucket::Occupied { inode, .. } = bucket {
                if inode.dirty {
                    dirty.push(inode.clone());
                    inode.dirty = false;
                }
            }
        }
        self.dirty_count = 0;
        dirty
    }

    /// Mark a specific cached inode as dirty.
    fn mark_dirty(&mut self, dev: u32, ino: u64) -> bool {
        let start = hash_key(dev, ino) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &mut self.buckets[idx] {
                Bucket::Occupied { inode, .. } => {
                    if inode.dev == dev && inode.ino == ino {
                        if !inode.dirty {
                            inode.dirty = true;
                            self.dirty_count = self.dirty_count.saturating_add(1);
                        }
                        return true;
                    }
                }
                Bucket::Empty => return false,
                Bucket::Deleted => {}
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static ICACHE: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Look up a cached inode by (device, inode number).
pub fn get(dev: u32, ino: u64) -> Option<CachedInode> {
    let mut guard = ICACHE.lock();
    guard.as_mut().and_then(|inner| inner.get(dev, ino))
}

/// Insert or update an inode in the cache.
pub fn insert(inode: CachedInode) {
    let mut guard = ICACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.insert(inode);
    }
}

/// Evict an inode from the cache.
pub fn evict(dev: u32, ino: u64) {
    let mut guard = ICACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.evict(dev, ino);
    }
}

/// Mark a cached inode as dirty.
pub fn mark_dirty(dev: u32, ino: u64) -> bool {
    let mut guard = ICACHE.lock();
    guard
        .as_mut()
        .map_or(false, |inner| inner.mark_dirty(dev, ino))
}

/// Flush all dirty inodes, returning them for writeback.
pub fn flush_dirty() -> Vec<CachedInode> {
    let mut guard = ICACHE.lock();
    guard
        .as_mut()
        .map_or_else(Vec::new, |inner| inner.flush_dirty())
}

/// Return the number of dirty inodes pending writeback.
pub fn dirty_count() -> usize {
    let guard = ICACHE.lock();
    guard.as_ref().map_or(0, |inner| inner.dirty_count)
}

/// Return (cached_count, capacity) for diagnostics.
pub fn stats() -> (usize, usize) {
    let guard = ICACHE.lock();
    guard
        .as_ref()
        .map_or((0, 0), |inner| (inner.count, inner.capacity))
}

/// Initialize the inode cache subsystem.
pub fn init() {
    let mut guard = ICACHE.lock();
    *guard = Some(Inner::new(DEFAULT_CAPACITY));
    serial_println!("    icache: initialized ({} buckets)", DEFAULT_CAPACITY);
}
