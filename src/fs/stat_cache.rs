use crate::serial_println;
use crate::sync::Mutex;
/// Stat result caching with TTL expiry for hot paths
///
/// Part of the AIOS filesystem layer.
///
/// Caches `stat()` results keyed by path so that repeated stat calls
/// (very common in shell completion, build systems, etc.) avoid hitting
/// the underlying filesystem.
///
/// Design:
///   - Hash table (open addressing) keyed by full path string.
///   - Each entry carries a TTL (time-to-live) in ticks; expired entries
///     are treated as misses and evicted on next access.
///   - LRU eviction when the table is full.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux attribute cache (NFS), stat caching in FUSE.
/// All code is original.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_CAPACITY: usize = 256;
const MAX_PROBE: usize = 8;
/// Default TTL in ticks (roughly 5 seconds at 1000Hz)
const DEFAULT_TTL: u64 = 5000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Cached stat result mirroring POSIX struct stat fields.
#[derive(Clone)]
pub struct StatResult {
    pub dev: u32,
    pub ino: u64,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub blksize: u32,
    pub blocks: u64,
}

/// Internal bucket.
#[derive(Clone)]
enum Bucket {
    Empty,
    Occupied {
        path: String,
        stat: StatResult,
        expires_at: u64,
        generation: u64,
    },
    Deleted,
}

/// Internal cache state.
struct Inner {
    buckets: Vec<Bucket>,
    capacity: usize,
    count: usize,
    generation: u64,
    ttl: u64,
    hits: u64,
    misses: u64,
    expirations: u64,
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

fn hash_path(path: &str) -> usize {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h as usize
}

/// Read current tick count for TTL expiry.
fn current_ticks() -> u64 {
    // Use the kernel tick counter if available; fallback to 0.
    #[cfg(not(test))]
    {
        crate::time::clock::uptime_ms()
    }
    #[cfg(test)]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new(capacity: usize, ttl: u64) -> Self {
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
            ttl,
            hits: 0,
            misses: 0,
            expirations: 0,
        }
    }

    fn mask(&self) -> usize {
        self.capacity - 1
    }

    fn next_gen(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    fn lookup(&mut self, path: &str) -> Option<StatResult> {
        let now = current_ticks();
        let start = hash_path(path) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Occupied {
                    path: p,
                    stat,
                    expires_at,
                    ..
                } => {
                    if p == path {
                        // Check TTL
                        if now > *expires_at {
                            // Expired -- evict
                            self.buckets[idx] = Bucket::Deleted;
                            self.count = self.count.saturating_sub(1);
                            self.expirations = self.expirations.saturating_add(1);
                            self.misses = self.misses.saturating_add(1);
                            return None;
                        }
                        let result = stat.clone();
                        let gen = self.next_gen();
                        if let Bucket::Occupied { generation, .. } = &mut self.buckets[idx] {
                            *generation = gen;
                        }
                        self.hits = self.hits.saturating_add(1);
                        return Some(result);
                    }
                }
                Bucket::Empty => {
                    self.misses = self.misses.saturating_add(1);
                    return None;
                }
                Bucket::Deleted => {}
            }
        }
        self.misses = self.misses.saturating_add(1);
        None
    }

    fn insert(&mut self, path: String, stat: StatResult) {
        let now = current_ticks();
        let expires_at = now + self.ttl;
        let start = hash_path(&path) & self.mask();
        let gen = self.next_gen();

        let mut lowest_gen = u64::MAX;
        let mut lowest_idx = start;

        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Empty | Bucket::Deleted => {
                    self.buckets[idx] = Bucket::Occupied {
                        path,
                        stat,
                        expires_at,
                        generation: gen,
                    };
                    self.count = self.count.saturating_add(1);
                    return;
                }
                Bucket::Occupied {
                    path: p,
                    generation: g,
                    ..
                } => {
                    if *p == path {
                        self.buckets[idx] = Bucket::Occupied {
                            path,
                            stat,
                            expires_at,
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

        // Evict LRU
        self.buckets[lowest_idx] = Bucket::Occupied {
            path,
            stat,
            expires_at,
            generation: gen,
        };
    }

    fn invalidate(&mut self, path: &str) {
        let start = hash_path(path) & self.mask();
        for i in 0..MAX_PROBE {
            let idx = (start + i) & self.mask();
            match &self.buckets[idx] {
                Bucket::Occupied { path: p, .. } => {
                    if *p == path {
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

    /// Invalidate all entries whose path starts with the given prefix.
    fn invalidate_prefix(&mut self, prefix: &str) {
        for i in 0..self.capacity {
            let should_delete = match &self.buckets[i] {
                Bucket::Occupied { path, .. } => path.starts_with(prefix),
                _ => false,
            };
            if should_delete {
                self.buckets[i] = Bucket::Deleted;
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    /// Sweep expired entries.
    fn purge_expired(&mut self) {
        let now = current_ticks();
        for i in 0..self.capacity {
            let expired = match &self.buckets[i] {
                Bucket::Occupied { expires_at, .. } => now > *expires_at,
                _ => false,
            };
            if expired {
                self.buckets[i] = Bucket::Deleted;
                self.count = self.count.saturating_sub(1);
                self.expirations = self.expirations.saturating_add(1);
            }
        }
    }

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

static STAT_CACHE: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Look up a cached stat result by path. Returns `None` on miss or expiry.
pub fn lookup(path: &str) -> Option<StatResult> {
    let mut guard = STAT_CACHE.lock();
    guard.as_mut().and_then(|inner| inner.lookup(path))
}

/// Insert or update a stat result.
pub fn insert(path: String, stat: StatResult) {
    let mut guard = STAT_CACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.insert(path, stat);
    }
}

/// Invalidate a single path.
pub fn invalidate(path: &str) {
    let mut guard = STAT_CACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.invalidate(path);
    }
}

/// Invalidate all paths under a directory prefix.
pub fn invalidate_prefix(prefix: &str) {
    let mut guard = STAT_CACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.invalidate_prefix(prefix);
    }
}

/// Sweep expired entries (call periodically from a timer).
pub fn purge_expired() {
    let mut guard = STAT_CACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.purge_expired();
    }
}

/// Flush the entire cache.
pub fn flush() {
    let mut guard = STAT_CACHE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.flush();
    }
}

/// Return (hits, misses, expirations) for diagnostics.
pub fn stats() -> (u64, u64, u64) {
    let guard = STAT_CACHE.lock();
    match guard.as_ref() {
        Some(inner) => (inner.hits, inner.misses, inner.expirations),
        None => (0, 0, 0),
    }
}

/// Initialize the stat cache subsystem.
pub fn init() {
    let mut guard = STAT_CACHE.lock();
    *guard = Some(Inner::new(DEFAULT_CAPACITY, DEFAULT_TTL));
    serial_println!(
        "    stat_cache: initialized ({} buckets, TTL {}ms)",
        DEFAULT_CAPACITY,
        DEFAULT_TTL
    );
}
