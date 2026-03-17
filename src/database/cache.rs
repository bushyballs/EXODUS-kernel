/// Page cache with LRU eviction for Genesis embedded database
///
/// Provides an in-memory cache of database pages with:
///   - LRU (Least Recently Used) eviction policy
///   - Dirty page tracking for write-back
///   - Pin/unpin for pages that must stay resident
///   - Page-level checksums for corruption detection
///   - Cache statistics (hit rate, eviction count, dirty ratio)
///
/// Pages are fixed-size (4096 bytes), identified by a (table_id, page_num) pair.
/// Cache capacity is configurable at init time.
///
/// No floats — statistics use Q16 fixed-point.
///
/// Inspired by: SQLite pager, PostgreSQL buffer manager, Linux page cache.
/// All code is original.
use crate::{serial_print, serial_println};

use crate::sync::Mutex;
use alloc::vec::Vec;

/// Q16 fixed-point constant
const Q16_ONE: i32 = 65536;

/// Page size in bytes
pub const PAGE_SIZE: usize = 4096;

/// Default maximum number of cached pages
const DEFAULT_CACHE_SIZE: usize = 256;

/// A page identifier: (table_id, page_number)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageId {
    pub table_id: u32,
    pub page_num: u32,
}

impl PageId {
    pub fn new(table_id: u32, page_num: u32) -> Self {
        PageId { table_id, page_num }
    }

    /// Hash the page ID for checksum seeding
    fn hash_seed(&self) -> u32 {
        let mut h: u32 = 0x811C9DC5;
        let t_bytes = self.table_id.to_le_bytes();
        let p_bytes = self.page_num.to_le_bytes();
        for &b in t_bytes.iter().chain(p_bytes.iter()) {
            h ^= b as u32;
            h = h.wrapping_mul(0x01000193);
        }
        h
    }
}

/// A cached page
struct CachePage {
    /// Page identifier
    id: PageId,
    /// Page data (fixed 4096 bytes)
    data: [u8; PAGE_SIZE],
    /// Whether this page has been modified since loading
    dirty: bool,
    /// Whether this page is pinned (cannot be evicted)
    pinned: bool,
    /// Access counter for LRU tracking (higher = more recently used)
    access_tick: u64,
    /// Checksum of page data for integrity verification
    checksum: u32,
    /// Number of times this page has been accessed
    hit_count: u32,
}

impl CachePage {
    fn new(id: PageId) -> Self {
        CachePage {
            id,
            data: [0u8; PAGE_SIZE],
            dirty: false,
            pinned: false,
            access_tick: 0,
            checksum: 0,
            hit_count: 0,
        }
    }

    /// Compute checksum of current page data (FNV-1a 32-bit)
    fn compute_checksum(&self) -> u32 {
        let mut hash: u32 = self.id.hash_seed();
        for &byte in &self.data {
            hash ^= byte as u32;
            hash = hash.wrapping_mul(0x01000193);
        }
        hash
    }

    /// Update the stored checksum
    fn update_checksum(&mut self) {
        self.checksum = self.compute_checksum();
    }

    /// Verify data integrity against stored checksum
    fn verify(&self) -> bool {
        self.checksum == self.compute_checksum()
    }
}

/// Cache statistics
struct CacheStats {
    total_hits: u64,
    total_misses: u64,
    total_evictions: u64,
    total_flushes: u64,
    total_checksum_failures: u64,
}

impl CacheStats {
    fn new() -> Self {
        CacheStats {
            total_hits: 0,
            total_misses: 0,
            total_evictions: 0,
            total_flushes: 0,
            total_checksum_failures: 0,
        }
    }

    /// Hit rate as Q16 fixed-point (0 = 0%, Q16_ONE = 100%)
    fn hit_rate_q16(&self) -> i32 {
        let total = self.total_hits + self.total_misses;
        if total == 0 {
            return 0;
        }
        (((self.total_hits as i64) << 16) / (total as i64)) as i32
    }

    /// Eviction rate as Q16 (evictions / total accesses)
    fn eviction_rate_q16(&self) -> i32 {
        let total = self.total_hits + self.total_misses;
        if total == 0 {
            return 0;
        }
        (((self.total_evictions as i64) << 16) / (total as i64)) as i32
    }
}

/// The page cache
struct PageCache {
    /// All cached pages
    pages: Vec<CachePage>,
    /// Maximum number of pages to cache
    max_pages: usize,
    /// Global tick counter for LRU ordering
    current_tick: u64,
    /// Cache statistics
    stats: CacheStats,
}

static CACHE: Mutex<Option<PageCache>> = Mutex::new(None);

impl PageCache {
    fn new(max_pages: usize) -> Self {
        PageCache {
            pages: Vec::new(),
            max_pages,
            current_tick: 0,
            stats: CacheStats::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.current_tick = self.current_tick.saturating_add(1);
        self.current_tick
    }

    /// Find a page in the cache. Returns index if found.
    fn find_page(&self, page_id: &PageId) -> Option<usize> {
        self.pages.iter().position(|p| p.id == *page_id)
    }

    /// Get a page from cache, recording a hit. Returns a reference to the data.
    fn get(&mut self, page_id: &PageId) -> Option<&[u8; PAGE_SIZE]> {
        let tick = self.tick();
        if let Some(idx) = self.find_page(page_id) {
            self.pages[idx].access_tick = tick;
            self.pages[idx].hit_count = self.pages[idx].hit_count.saturating_add(1);
            self.stats.total_hits = self.stats.total_hits.saturating_add(1);
            Some(&self.pages[idx].data)
        } else {
            self.stats.total_misses = self.stats.total_misses.saturating_add(1);
            None
        }
    }

    /// Get mutable access to a page's data, marking it dirty
    fn get_mut(&mut self, page_id: &PageId) -> Option<&mut [u8; PAGE_SIZE]> {
        let tick = self.tick();
        if let Some(idx) = self.find_page(page_id) {
            self.pages[idx].access_tick = tick;
            self.pages[idx].hit_count = self.pages[idx].hit_count.saturating_add(1);
            self.pages[idx].dirty = true;
            self.stats.total_hits = self.stats.total_hits.saturating_add(1);
            Some(&mut self.pages[idx].data)
        } else {
            self.stats.total_misses = self.stats.total_misses.saturating_add(1);
            None
        }
    }

    /// Insert a page into the cache. Evicts LRU if at capacity.
    /// Returns the evicted page's data if dirty (for write-back).
    fn insert(
        &mut self,
        page_id: PageId,
        data: [u8; PAGE_SIZE],
    ) -> Option<(PageId, [u8; PAGE_SIZE])> {
        let mut evicted = None;

        // Check if already cached
        if let Some(idx) = self.find_page(&page_id) {
            let tick = self.tick();
            self.pages[idx].data = data;
            self.pages[idx].access_tick = tick;
            self.pages[idx].dirty = false;
            self.pages[idx].update_checksum();
            return None;
        }

        // Evict if at capacity
        if self.pages.len() >= self.max_pages {
            evicted = self.evict_lru();
        }

        let tick = self.tick();
        let mut page = CachePage::new(page_id);
        page.data = data;
        page.access_tick = tick;
        page.update_checksum();
        self.pages.push(page);

        evicted
    }

    /// Find and remove the least-recently-used unpinned page.
    /// Returns its ID and data if it was dirty.
    fn evict_lru(&mut self) -> Option<(PageId, [u8; PAGE_SIZE])> {
        if self.pages.is_empty() {
            return None;
        }

        // Find unpinned page with lowest access_tick
        let mut lru_idx: Option<usize> = None;
        let mut lru_tick = u64::MAX;

        for (i, page) in self.pages.iter().enumerate() {
            if !page.pinned && page.access_tick < lru_tick {
                lru_tick = page.access_tick;
                lru_idx = Some(i);
            }
        }

        if let Some(idx) = lru_idx {
            let page = self.pages.remove(idx);
            self.stats.total_evictions = self.stats.total_evictions.saturating_add(1);

            if page.dirty {
                Some((page.id, page.data))
            } else {
                None
            }
        } else {
            // All pages are pinned — cannot evict
            None
        }
    }

    /// Pin a page to prevent eviction
    fn pin(&mut self, page_id: &PageId) -> bool {
        if let Some(idx) = self.find_page(page_id) {
            self.pages[idx].pinned = true;
            true
        } else {
            false
        }
    }

    /// Unpin a page to allow eviction
    fn unpin(&mut self, page_id: &PageId) -> bool {
        if let Some(idx) = self.find_page(page_id) {
            self.pages[idx].pinned = false;
            true
        } else {
            false
        }
    }

    /// Mark a page as dirty
    fn mark_dirty(&mut self, page_id: &PageId) -> bool {
        if let Some(idx) = self.find_page(page_id) {
            self.pages[idx].dirty = true;
            true
        } else {
            false
        }
    }

    /// Flush all dirty pages (returns list of dirty page IDs and data for write-back)
    fn flush_all(&mut self) -> Vec<(PageId, [u8; PAGE_SIZE])> {
        let mut flushed = Vec::new();
        for page in &mut self.pages {
            if page.dirty {
                page.update_checksum();
                flushed.push((page.id, page.data));
                page.dirty = false;
                self.stats.total_flushes = self.stats.total_flushes.saturating_add(1);
            }
        }
        flushed
    }

    /// Flush dirty pages for a specific table
    fn flush_table(&mut self, table_id: u32) -> Vec<(PageId, [u8; PAGE_SIZE])> {
        let mut flushed = Vec::new();
        for page in &mut self.pages {
            if page.id.table_id == table_id && page.dirty {
                page.update_checksum();
                flushed.push((page.id, page.data));
                page.dirty = false;
                self.stats.total_flushes = self.stats.total_flushes.saturating_add(1);
            }
        }
        flushed
    }

    /// Invalidate all pages for a table (e.g., after DROP TABLE)
    fn invalidate_table(&mut self, table_id: u32) {
        self.pages.retain(|p| p.id.table_id != table_id);
    }

    /// Invalidate a single page
    fn invalidate(&mut self, page_id: &PageId) {
        self.pages.retain(|p| p.id != *page_id);
    }

    /// Verify all cached pages for data integrity
    fn verify_all(&mut self) -> u32 {
        let mut failures = 0u32;
        for page in &self.pages {
            if !page.verify() {
                failures += 1;
                self.stats.total_checksum_failures =
                    self.stats.total_checksum_failures.saturating_add(1);
            }
        }
        failures
    }

    /// Count of dirty pages
    fn dirty_count(&self) -> usize {
        self.pages.iter().filter(|p| p.dirty).count()
    }

    /// Count of pinned pages
    fn pinned_count(&self) -> usize {
        self.pages.iter().filter(|p| p.pinned).count()
    }

    /// Dirty ratio as Q16 fixed-point
    fn dirty_ratio_q16(&self) -> i32 {
        let total = self.pages.len() as i64;
        if total == 0 {
            return 0;
        }
        let dirty = self.dirty_count() as i64;
        (((dirty) << 16) / (total)) as i32
    }

    /// Current utilization as Q16 (pages used / capacity)
    fn utilization_q16(&self) -> i32 {
        if self.max_pages == 0 {
            return 0;
        }
        (((self.pages.len() as i64) << 16) / (self.max_pages as i64)) as i32
    }
}

// === Public API ===

/// Get a page from the cache
pub fn get_page(table_id: u32, page_num: u32) -> Option<[u8; PAGE_SIZE]> {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        let pid = PageId::new(table_id, page_num);
        cache.get(&pid).map(|data| *data)
    } else {
        None
    }
}

/// Insert a page into the cache
pub fn insert_page(table_id: u32, page_num: u32, data: [u8; PAGE_SIZE]) {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        let pid = PageId::new(table_id, page_num);
        let _ = cache.insert(pid, data);
    }
}

/// Pin a cached page
pub fn pin_page(table_id: u32, page_num: u32) -> bool {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        let pid = PageId::new(table_id, page_num);
        cache.pin(&pid)
    } else {
        false
    }
}

/// Unpin a cached page
pub fn unpin_page(table_id: u32, page_num: u32) -> bool {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        let pid = PageId::new(table_id, page_num);
        cache.unpin(&pid)
    } else {
        false
    }
}

/// Mark a page dirty
pub fn mark_dirty(table_id: u32, page_num: u32) -> bool {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        let pid = PageId::new(table_id, page_num);
        cache.mark_dirty(&pid)
    } else {
        false
    }
}

/// Flush all dirty pages
pub fn flush_all() -> usize {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        cache.flush_all().len()
    } else {
        0
    }
}

/// Flush dirty pages for a specific table
pub fn flush_table(table_id: u32) -> usize {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        cache.flush_table(table_id).len()
    } else {
        0
    }
}

/// Invalidate all cached pages for a table
pub fn invalidate_table(table_id: u32) {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        cache.invalidate_table(table_id);
    }
}

/// Verify integrity of all cached pages
pub fn verify_integrity() -> u32 {
    let mut guard = CACHE.lock();
    if let Some(ref mut cache) = *guard {
        cache.verify_all()
    } else {
        0
    }
}

/// Get cache hit rate as a percentage (0-100) using Q16 intermediate
pub fn hit_rate_percent() -> u32 {
    let guard = CACHE.lock();
    if let Some(ref cache) = *guard {
        let q16 = cache.stats.hit_rate_q16();
        (((q16 as i64) * 100) >> 16) as u32
    } else {
        0
    }
}

/// Initialize the page cache subsystem
pub fn init() {
    let mut guard = CACHE.lock();
    *guard = Some(PageCache::new(DEFAULT_CACHE_SIZE));
    serial_println!(
        "    Page cache ready (LRU eviction, {} pages, 4KB each)",
        DEFAULT_CACHE_SIZE
    );
}
