/// Page cache — file-backed page caching for Genesis
///
/// Caches recently accessed file pages in memory to avoid repeated disk I/O.
/// Uses LRU (Least Recently Used) eviction when memory pressure is high.
///
/// Features:
///   - Page cache indexed by (inode, offset) -> physical frame
///   - LRU list for eviction (move to head on access)
///   - Read-ahead: prefetch next N pages on sequential access detection
///   - Dirty page tracking (mark on write, flush to backing store)
///   - Write-back scheduling (flush dirty pages older than threshold)
///   - Page cache pressure: evict clean pages first, then dirty
///   - Cache hit/miss statistics
///   - Sync/flush all dirty pages for an inode
///
/// Inspired by: Linux page cache (mm/filemap.c). All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;

/// Maximum pages in the cache
const MAX_CACHED_PAGES: usize = 4096;

/// Read-ahead window: max pages to prefetch
const READAHEAD_MAX: usize = 16;

/// Read-ahead minimum sequential accesses before triggering
const READAHEAD_THRESHOLD: u32 = 2;

/// Dirty page writeback age threshold (in ticks — pages older than this get flushed)
const WRITEBACK_AGE_THRESHOLD: u64 = 1000;

/// Maximum dirty pages before forced writeback
const MAX_DIRTY_PAGES: usize = 1024;

/// Maximum dirty ratio (per-mille of total cache) before forced writeback
const MAX_DIRTY_RATIO_PERMILLE: usize = 400; // 40%

/// Page cache entry
#[derive(Clone)]
pub struct CachedPage {
    /// Inode number (which file)
    pub ino: u64,
    /// Page offset within the file
    pub offset: u64,
    /// Physical address of the cached page
    pub phys_addr: usize,
    /// Dirty flag (needs writeback)
    pub dirty: bool,
    /// Access counter (for LRU approximation)
    pub access_count: u64,
    /// Last access tick
    pub last_access: u64,
    /// Tick when page was dirtied (0 if clean)
    pub dirty_since: u64,
    /// Reference count
    pub refcount: u32,
    /// Whether this page was obtained via read-ahead (not yet accessed by user)
    pub readahead: bool,
}

/// Per-inode sequential access tracker (for read-ahead)
#[derive(Clone, Copy)]
struct AccessTracker {
    /// Inode being tracked
    ino: u64,
    /// Last accessed offset
    last_offset: u64,
    /// Number of consecutive sequential accesses
    sequential_count: u32,
    /// Read-ahead window size (adaptive)
    readahead_size: u32,
    /// Active flag
    active: bool,
}

impl AccessTracker {
    const fn empty() -> Self {
        AccessTracker {
            ino: 0,
            last_offset: 0,
            sequential_count: 0,
            readahead_size: 4,
            active: false,
        }
    }
}

/// LRU list node (doubly-linked list embedded in a fixed array)
#[derive(Clone, Copy)]
struct LruNode {
    /// Key: (ino, offset)
    key: (u64, u64),
    /// Index of the next node (toward tail / LRU end)
    next: i32, // -1 = none
    /// Index of the previous node (toward head / MRU end)
    prev: i32, // -1 = none
    /// Whether this slot is active
    active: bool,
}

impl LruNode {
    const fn empty() -> Self {
        LruNode {
            key: (0, 0),
            next: -1,
            prev: -1,
            active: false,
        }
    }
}

/// LRU list manager
struct LruList {
    /// Fixed array of LRU nodes
    nodes: [LruNode; MAX_CACHED_PAGES],
    /// Index of the head (most recently used)
    head: i32,
    /// Index of the tail (least recently used)
    tail: i32,
    /// Number of active nodes
    count: usize,
}

impl LruList {
    const fn new() -> Self {
        const EMPTY_NODE: LruNode = LruNode::empty();
        LruList {
            nodes: [EMPTY_NODE; MAX_CACHED_PAGES],
            head: -1,
            tail: -1,
            count: 0,
        }
    }

    /// Find a free slot
    fn find_free_slot(&self) -> Option<usize> {
        for i in 0..MAX_CACHED_PAGES {
            if !self.nodes[i].active {
                return Some(i);
            }
        }
        None
    }

    /// Push a new entry to the head (most recently used)
    fn push_front(&mut self, key: (u64, u64)) -> Option<usize> {
        let slot = self.find_free_slot()?;
        self.nodes[slot].key = key;
        self.nodes[slot].active = true;
        self.nodes[slot].prev = -1;
        self.nodes[slot].next = self.head;

        if self.head >= 0 {
            self.nodes[self.head as usize].prev = slot as i32;
        }
        self.head = slot as i32;

        if self.tail < 0 {
            self.tail = slot as i32;
        }

        self.count += 1;
        Some(slot)
    }

    /// Move an existing node to the head (on access)
    fn move_to_front(&mut self, slot: usize) {
        if self.head == slot as i32 {
            return; // Already at head
        }

        // Unlink from current position
        let prev = self.nodes[slot].prev;
        let next = self.nodes[slot].next;

        if prev >= 0 {
            self.nodes[prev as usize].next = next;
        }
        if next >= 0 {
            self.nodes[next as usize].prev = prev;
        }
        if self.tail == slot as i32 {
            self.tail = prev;
        }

        // Insert at head
        self.nodes[slot].prev = -1;
        self.nodes[slot].next = self.head;
        if self.head >= 0 {
            self.nodes[self.head as usize].prev = slot as i32;
        }
        self.head = slot as i32;
    }

    /// Remove and return the tail entry (least recently used)
    fn pop_tail(&mut self) -> Option<(u64, u64)> {
        if self.tail < 0 {
            return None;
        }

        let slot = self.tail as usize;
        let key = self.nodes[slot].key;
        let prev = self.nodes[slot].prev;

        self.nodes[slot].active = false;
        self.nodes[slot].next = -1;
        self.nodes[slot].prev = -1;

        self.tail = prev;
        if prev >= 0 {
            self.nodes[prev as usize].next = -1;
        } else {
            self.head = -1;
        }

        self.count -= 1;
        Some(key)
    }

    /// Remove a specific entry by key
    fn remove(&mut self, key: (u64, u64)) -> bool {
        for i in 0..MAX_CACHED_PAGES {
            if self.nodes[i].active && self.nodes[i].key == key {
                let prev = self.nodes[i].prev;
                let next = self.nodes[i].next;

                if prev >= 0 {
                    self.nodes[prev as usize].next = next;
                } else {
                    self.head = next;
                }
                if next >= 0 {
                    self.nodes[next as usize].prev = prev;
                } else {
                    self.tail = prev;
                }

                self.nodes[i].active = false;
                self.nodes[i].next = -1;
                self.nodes[i].prev = -1;
                self.count -= 1;
                return true;
            }
        }
        false
    }

    /// Find the LRU slot index for a given key
    fn find_slot(&self, key: (u64, u64)) -> Option<usize> {
        for i in 0..MAX_CACHED_PAGES {
            if self.nodes[i].active && self.nodes[i].key == key {
                return Some(i);
            }
        }
        None
    }
}

/// Composite key: (inode, offset)
type PageKey = (u64, u64);

/// The page cache
pub struct PageCache {
    /// Map from (ino, offset) -> CachedPage
    pages: BTreeMap<PageKey, CachedPage>,
    /// LRU list for eviction ordering
    lru: LruList,
    /// Total cached pages
    count: usize,
    /// Global access counter
    access_tick: u64,
    /// Per-inode access trackers (for read-ahead)
    trackers: [AccessTracker; 64],
    /// Number of active trackers
    tracker_count: usize,
    /// Stats
    pub stats: PageCacheStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PageCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub writebacks: u64,
    pub dirty_pages: u64,
    pub readahead_issued: u64,
    pub readahead_hits: u64,
    pub forced_writebacks: u64,
}

impl PageCache {
    const fn new() -> Self {
        const EMPTY_TRACKER: AccessTracker = AccessTracker::empty();
        PageCache {
            pages: BTreeMap::new(),
            lru: LruList::new(),
            count: 0,
            access_tick: 0,
            trackers: [EMPTY_TRACKER; 64],
            tracker_count: 0,
            stats: PageCacheStats {
                hits: 0,
                misses: 0,
                insertions: 0,
                evictions: 0,
                writebacks: 0,
                dirty_pages: 0,
                readahead_issued: 0,
                readahead_hits: 0,
                forced_writebacks: 0,
            },
        }
    }

    /// Look up a page in the cache
    pub fn find(&mut self, ino: u64, offset: u64) -> Option<&CachedPage> {
        self.access_tick = self.access_tick.saturating_add(1);
        let key = (ino, offset);

        // Update access tracker for read-ahead
        self.update_tracker(ino, offset);

        if let Some(page) = self.pages.get_mut(&key) {
            page.access_count += 1;
            page.last_access = self.access_tick;

            // If this was a read-ahead page, mark it as accessed
            if page.readahead {
                page.readahead = false;
                self.stats.readahead_hits = self.stats.readahead_hits.saturating_add(1);
            }

            self.stats.hits = self.stats.hits.saturating_add(1);

            // Move to front of LRU
            if let Some(slot) = self.lru.find_slot(key) {
                self.lru.move_to_front(slot);
            }

            // Return immutable ref through re-borrow
            Some(&*page)
        } else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            None
        }
    }

    /// Update the sequential access tracker for an inode
    fn update_tracker(&mut self, ino: u64, offset: u64) {
        // Find existing tracker for this inode
        for i in 0..self.tracker_count {
            if self.trackers[i].active && self.trackers[i].ino == ino {
                let expected_next = self.trackers[i].last_offset + 1;
                if offset == expected_next {
                    self.trackers[i].sequential_count =
                        self.trackers[i].sequential_count.saturating_add(1);
                    // Grow read-ahead window (up to max)
                    if self.trackers[i].readahead_size < READAHEAD_MAX as u32 {
                        self.trackers[i].readahead_size += 1;
                    }
                } else {
                    // Non-sequential access — reset
                    self.trackers[i].sequential_count = 0;
                    self.trackers[i].readahead_size = 4;
                }
                self.trackers[i].last_offset = offset;
                return;
            }
        }

        // Create new tracker
        if self.tracker_count < 64 {
            let idx = self.tracker_count;
            self.trackers[idx] = AccessTracker {
                ino,
                last_offset: offset,
                sequential_count: 0,
                readahead_size: 4,
                active: true,
            };
            self.tracker_count += 1;
        }
    }

    /// Check if read-ahead should be triggered for an inode at a given offset.
    /// Returns the number of pages to prefetch (0 if no read-ahead needed).
    pub fn should_readahead(&self, ino: u64, offset: u64) -> usize {
        for i in 0..self.tracker_count {
            if self.trackers[i].active && self.trackers[i].ino == ino {
                if self.trackers[i].sequential_count >= READAHEAD_THRESHOLD
                    && self.trackers[i].last_offset == offset
                {
                    return self.trackers[i].readahead_size as usize;
                }
            }
        }
        0
    }

    /// Insert a page into the cache
    pub fn insert(&mut self, ino: u64, offset: u64, phys_addr: usize) {
        self.insert_flags(ino, offset, phys_addr, false);
    }

    /// Insert a page with optional readahead flag
    pub fn insert_flags(&mut self, ino: u64, offset: u64, phys_addr: usize, is_readahead: bool) {
        // Check dirty page limit — force writeback if too many dirty pages
        if self.stats.dirty_pages as usize > MAX_DIRTY_PAGES {
            self.writeback_aged();
        }

        // Evict if at capacity
        if self.count >= MAX_CACHED_PAGES {
            self.evict_lru();
        }

        let key = (ino, offset);
        self.access_tick = self.access_tick.saturating_add(1);

        let entry = CachedPage {
            ino,
            offset,
            phys_addr,
            dirty: false,
            access_count: if is_readahead { 0 } else { 1 },
            last_access: self.access_tick,
            dirty_since: 0,
            refcount: 1,
            readahead: is_readahead,
        };

        // Add to LRU
        self.lru.push_front(key);

        self.pages.insert(key, entry);
        self.count += 1;
        self.stats.insertions = self.stats.insertions.saturating_add(1);

        if is_readahead {
            self.stats.readahead_issued = self.stats.readahead_issued.saturating_add(1);
        }
    }

    /// Mark a cached page as dirty
    pub fn mark_dirty(&mut self, ino: u64, offset: u64) {
        if let Some(page) = self.pages.get_mut(&(ino, offset)) {
            if !page.dirty {
                page.dirty = true;
                page.dirty_since = self.access_tick;
                self.stats.dirty_pages = self.stats.dirty_pages.saturating_add(1);

                // Check dirty ratio — trigger writeback if too high
                let dirty_ratio = if self.count > 0 {
                    (self.stats.dirty_pages as usize * 1000) / self.count
                } else {
                    0
                };
                if dirty_ratio > MAX_DIRTY_RATIO_PERMILLE {
                    self.writeback_aged();
                }
            }
        }
    }

    /// Evict the least recently used clean page.
    /// If no clean pages, evict LRU dirty page (after writeback).
    fn evict_lru(&mut self) {
        // First pass: try to find the LRU clean page with refcount <= 1
        let mut best_clean_key: Option<PageKey> = None;
        let mut best_clean_access = u64::MAX;
        let mut best_dirty_key: Option<PageKey> = None;
        let mut best_dirty_access = u64::MAX;

        for (key, page) in &self.pages {
            if page.refcount > 1 {
                continue; // Pinned
            }
            if !page.dirty && page.last_access < best_clean_access {
                best_clean_access = page.last_access;
                best_clean_key = Some(*key);
            }
            if page.dirty && page.last_access < best_dirty_access {
                best_dirty_access = page.last_access;
                best_dirty_key = Some(*key);
            }
        }

        // Prefer evicting readahead pages that were never accessed
        for (key, page) in &self.pages {
            if page.readahead && page.access_count == 0 && page.refcount <= 1 {
                self.evict_page(*key);
                return;
            }
        }

        // Evict cleanest LRU page
        if let Some(key) = best_clean_key {
            self.evict_page(key);
        } else if let Some(key) = best_dirty_key {
            // Must writeback first
            if let Some(page) = self.pages.get_mut(&key) {
                page.dirty = false;
                self.stats.writebacks = self.stats.writebacks.saturating_add(1);
                if self.stats.dirty_pages > 0 {
                    self.stats.dirty_pages -= 1;
                }
            }
            self.evict_page(key);
        }
    }

    /// Actually evict a page by key
    fn evict_page(&mut self, key: PageKey) {
        if let Some(page) = self.pages.get(&key) {
            if page.dirty {
                self.stats.writebacks = self.stats.writebacks.saturating_add(1);
                if self.stats.dirty_pages > 0 {
                    self.stats.dirty_pages -= 1;
                }
            }
            // Free the physical page
            crate::memory::buddy::free_page(page.phys_addr);
        }
        self.pages.remove(&key);
        self.lru.remove(key);
        self.count -= 1;
        self.stats.evictions = self.stats.evictions.saturating_add(1);
    }

    /// Writeback dirty pages that are older than the age threshold
    fn writeback_aged(&mut self) {
        let threshold = if self.access_tick > WRITEBACK_AGE_THRESHOLD {
            self.access_tick - WRITEBACK_AGE_THRESHOLD
        } else {
            0
        };

        let keys: alloc::vec::Vec<PageKey> = self.pages.keys().copied().collect();
        for key in keys {
            if let Some(page) = self.pages.get_mut(&key) {
                if page.dirty && page.dirty_since > 0 && page.dirty_since < threshold {
                    // In a real implementation, write page back to block device here
                    page.dirty = false;
                    page.dirty_since = 0;
                    self.stats.writebacks = self.stats.writebacks.saturating_add(1);
                    self.stats.forced_writebacks = self.stats.forced_writebacks.saturating_add(1);
                    if self.stats.dirty_pages > 0 {
                        self.stats.dirty_pages -= 1;
                    }
                }
            }
        }
    }

    /// Sync all dirty pages (write back to disk)
    pub fn sync_all(&mut self) -> usize {
        let mut synced = 0;
        for (_key, page) in self.pages.iter_mut() {
            if page.dirty {
                // In a real implementation, write page back to block device
                page.dirty = false;
                page.dirty_since = 0;
                synced += 1;
                self.stats.writebacks = self.stats.writebacks.saturating_add(1);
            }
        }
        self.stats.dirty_pages = 0;
        synced
    }

    /// Sync dirty pages for a specific inode
    pub fn sync_inode(&mut self, ino: u64) -> usize {
        let mut synced = 0;
        for (key, page) in self.pages.iter_mut() {
            if key.0 == ino && page.dirty {
                page.dirty = false;
                page.dirty_since = 0;
                synced += 1;
                self.stats.writebacks = self.stats.writebacks.saturating_add(1);
                if self.stats.dirty_pages > 0 {
                    self.stats.dirty_pages -= 1;
                }
            }
        }
        synced
    }

    /// Invalidate all pages for an inode (on file deletion or truncation)
    pub fn invalidate_inode(&mut self, ino: u64) {
        let keys: alloc::vec::Vec<PageKey> =
            self.pages.keys().filter(|k| k.0 == ino).copied().collect();
        for key in keys {
            if let Some(page) = self.pages.remove(&key) {
                if page.dirty {
                    if self.stats.dirty_pages > 0 {
                        self.stats.dirty_pages -= 1;
                    }
                }
                crate::memory::buddy::free_page(page.phys_addr);
                self.lru.remove(key);
                self.count -= 1;
                self.stats.evictions = self.stats.evictions.saturating_add(1);
            }
        }

        // Remove tracker for this inode
        for i in 0..self.tracker_count {
            if self.trackers[i].active && self.trackers[i].ino == ino {
                self.trackers[i].active = false;
            }
        }
    }

    /// Drop all clean cached pages (memory pressure)
    pub fn shrink(&mut self) -> usize {
        let mut freed = 0;
        let keys: alloc::vec::Vec<PageKey> = self.pages.keys().copied().collect();
        for key in keys {
            if let Some(page) = self.pages.get(&key) {
                if !page.dirty && page.refcount <= 1 {
                    let phys = page.phys_addr;
                    self.pages.remove(&key);
                    crate::memory::buddy::free_page(phys);
                    self.lru.remove(key);
                    self.count -= 1;
                    freed += 1;
                }
            }
        }
        freed
    }

    /// Increment reference count on a cached page
    pub fn pin(&mut self, ino: u64, offset: u64) {
        if let Some(page) = self.pages.get_mut(&(ino, offset)) {
            page.refcount = page.refcount.saturating_add(1);
        }
    }

    /// Decrement reference count on a cached page
    pub fn unpin(&mut self, ino: u64, offset: u64) {
        if let Some(page) = self.pages.get_mut(&(ino, offset)) {
            if page.refcount > 0 {
                page.refcount -= 1;
            }
        }
    }

    /// Number of cached pages
    pub fn cached_count(&self) -> usize {
        self.count
    }

    /// Number of dirty pages
    pub fn dirty_count(&self) -> u64 {
        self.stats.dirty_pages
    }

    /// Hit rate as percentage * 100 (e.g., 9523 = 95.23%)
    pub fn hit_rate_x100(&self) -> usize {
        let total = self.stats.hits + self.stats.misses;
        if total == 0 {
            return 0;
        }
        ((self.stats.hits * 10000) / total) as usize
    }
}

pub static PAGE_CACHE: Mutex<PageCache> = Mutex::new(PageCache::new());

pub fn init() {
    crate::serial_println!("  [page_cache] initialized, max {} pages", MAX_CACHED_PAGES);
}

/// Read a page through the cache
pub fn read_page(ino: u64, offset: u64) -> Option<usize> {
    let mut cache = PAGE_CACHE.lock();
    if let Some(page) = cache.find(ino, offset) {
        return Some(page.phys_addr);
    }
    // Cache miss — caller must read from disk and insert
    None
}

/// Insert a page into the cache after reading from disk
pub fn insert_page(ino: u64, offset: u64, phys_addr: usize) {
    PAGE_CACHE.lock().insert(ino, offset, phys_addr);
}

/// Check if read-ahead should be triggered; returns number of pages to prefetch
pub fn check_readahead(ino: u64, offset: u64) -> usize {
    PAGE_CACHE.lock().should_readahead(ino, offset)
}

/// Insert a read-ahead page
pub fn insert_readahead(ino: u64, offset: u64, phys_addr: usize) {
    PAGE_CACHE.lock().insert_flags(ino, offset, phys_addr, true);
}

/// Sync all dirty pages
pub fn sync() -> usize {
    PAGE_CACHE.lock().sync_all()
}

/// Sync dirty pages for a specific inode
pub fn sync_inode(ino: u64) -> usize {
    PAGE_CACHE.lock().sync_inode(ino)
}
