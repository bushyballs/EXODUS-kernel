use crate::serial_println;
/// stats --- global memory statistics tracking for Genesis
///
/// Aggregates memory usage information from all subsystems into a single
/// unified view. Provides /proc/meminfo-style reporting.
///
/// Tracks:
///   - Total, free, used physical memory
///   - Cached (page cache) pages
///   - Dirty pages (awaiting writeback)
///   - Swap usage (in/out, total)
///   - Slab usage
///   - Buddy allocator free/used
///   - Frame allocator stats
///   - Kernel heap usage
///   - High/low watermark alerts
///
/// All values are in pages (4KB) or bytes as noted. Integer math only.
///
/// Inspired by: Linux /proc/meminfo, vm_stat. All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Page size
const PAGE_SIZE: usize = 4096;

/// Snapshot history depth
const HISTORY_SIZE: usize = 16;

/// Memory pressure watermarks (percentage * 10 of total)
const PRESSURE_LOW_PCT_X10: usize = 250; // 25.0%
const PRESSURE_MED_PCT_X10: usize = 100; // 10.0%
const PRESSURE_HIGH_PCT_X10: usize = 50; //  5.0%
const PRESSURE_CRIT_PCT_X10: usize = 10; //  1.0%

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A point-in-time memory statistics snapshot
#[derive(Debug, Clone, Copy, Default)]
pub struct MemSnapshot {
    /// Tick / timestamp of this snapshot
    pub tick: u64,

    // --- Physical memory ---
    /// Total physical pages managed by frame allocator
    pub total_pages: usize,
    /// Free physical pages (frame allocator)
    pub free_pages: usize,
    /// Used physical pages
    pub used_pages: usize,

    // --- Buddy allocator ---
    /// Buddy free pages
    pub buddy_free: usize,
    /// Buddy used pages
    pub buddy_used: usize,
    /// Buddy fragmentation score (0..1000)
    pub buddy_frag: usize,

    // --- Page cache ---
    /// Pages in page cache
    pub cached_pages: usize,
    /// Dirty pages awaiting writeback
    pub dirty_pages: usize,
    /// Page cache hit rate (x100, e.g. 9523 = 95.23%)
    pub cache_hit_rate_x100: usize,

    // --- Swap ---
    /// Total pages swapped out
    pub swap_out_total: u64,
    /// Total pages swapped in
    pub swap_in_total: u64,

    // --- Slab ---
    /// Total slab caches
    pub slab_caches: usize,
    /// Active slab objects (across all caches)
    pub slab_active_objs: usize,
    /// Total slab object capacity
    pub slab_total_objs: usize,
    /// Estimated slab memory in use (bytes): active_objs * avg_obj_size
    pub slab_bytes: usize,

    // --- Huge pages ---
    /// Total 2MB huge pages managed (pool + direct buddy)
    pub huge_pages_total: usize,
    /// Free 2MB huge pages available from buddy (order-9 free blocks)
    pub huge_pages_free: usize,
    /// Total THP promotions since boot
    pub thp_promotions: u64,
    /// Total THP splits since boot
    pub thp_splits: u64,

    // --- Kernel heap ---
    /// Kernel heap size in bytes
    pub heap_size: usize,

    // --- Derived ---
    /// Memory pressure level (0=none, 1=low, 2=medium, 3=high, 4=critical)
    pub pressure_level: u8,
    /// Free percentage * 10 (e.g. 253 = 25.3%)
    pub free_pct_x10: usize,
}

/// Memory statistics manager
struct StatsManager {
    /// Current snapshot
    current: MemSnapshot,
    /// Historical snapshots (ring buffer)
    history: [MemSnapshot; HISTORY_SIZE],
    /// Next history write index
    history_idx: usize,
    /// Global tick counter
    tick: u64,
    /// Number of snapshots taken
    snapshot_count: u64,
}

impl StatsManager {
    const fn new() -> Self {
        const EMPTY: MemSnapshot = MemSnapshot {
            tick: 0,
            total_pages: 0,
            free_pages: 0,
            used_pages: 0,
            buddy_free: 0,
            buddy_used: 0,
            buddy_frag: 0,
            cached_pages: 0,
            dirty_pages: 0,
            cache_hit_rate_x100: 0,
            swap_out_total: 0,
            swap_in_total: 0,
            slab_caches: 0,
            slab_active_objs: 0,
            slab_total_objs: 0,
            slab_bytes: 0,
            huge_pages_total: 0,
            huge_pages_free: 0,
            thp_promotions: 0,
            thp_splits: 0,
            heap_size: 0,
            pressure_level: 0,
            free_pct_x10: 0,
        };
        StatsManager {
            current: EMPTY,
            history: [EMPTY; HISTORY_SIZE],
            history_idx: 0,
            tick: 0,
            snapshot_count: 0,
        }
    }
}

static STATS: Mutex<StatsManager> = Mutex::new(StatsManager::new());

/// Global counters (lockless, updated by other subsystems)
pub static PAGES_ALLOCATED: AtomicU64 = AtomicU64::new(0);
pub static PAGES_FREED: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Internal: gather stats from each subsystem
// ---------------------------------------------------------------------------

fn gather_snapshot() -> MemSnapshot {
    let mut snap = MemSnapshot::default();

    // Frame allocator
    {
        let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
        snap.total_pages = crate::memory::frame_allocator::MAX_MEMORY / PAGE_SIZE;
        snap.free_pages = fa.free_count();
        snap.used_pages = fa.used_count();
    }

    // Buddy allocator
    {
        let buddy = crate::memory::buddy::BUDDY.lock();
        snap.buddy_free = buddy.free_count();
        snap.buddy_used = buddy.used_count();
        snap.buddy_frag = buddy.fragmentation_score();
    }

    // Page cache
    {
        let pc = crate::memory::page_cache::PAGE_CACHE.lock();
        snap.cached_pages = pc.cached_count();
        snap.dirty_pages = pc.dirty_count() as usize;
        snap.cache_hit_rate_x100 = pc.hit_rate_x100();
    }

    // Swap counters
    snap.swap_out_total = crate::memory::swap::TOTAL_SWAP_OUT.load(Ordering::Relaxed);
    snap.swap_in_total = crate::memory::swap::TOTAL_SWAP_IN.load(Ordering::Relaxed);

    // Slab
    {
        let slab = crate::memory::slab::SLAB.lock();
        snap.slab_caches = slab.cache_count;
        let mut active = 0usize;
        let mut total = 0usize;
        let mut bytes = 0usize;
        for i in 0..slab.cache_count {
            if slab.caches[i].active {
                let c = &slab.caches[i];
                let a = c.active_objects();
                active = active.saturating_add(a);
                total = total.saturating_add(c.total_objects());
                // Approximate bytes = active objects * object size
                bytes = bytes.saturating_add(a.saturating_mul(c.obj_size));
            }
        }
        snap.slab_active_objs = active;
        snap.slab_total_objs = total;
        snap.slab_bytes = bytes;
    }

    // Huge pages
    {
        let (thp_allocated, thp_promotions) = crate::memory::thp::thp_get_stats();
        // Free huge pages available from the buddy allocator at order-9
        snap.huge_pages_free = crate::memory::buddy::free_huge_page_count();
        // Total = free + those currently allocated
        snap.huge_pages_total = snap.huge_pages_free.saturating_add(thp_allocated as usize);
        snap.thp_promotions = thp_promotions;
        snap.thp_splits = 0; // splits not tracked separately
    }

    // Heap
    snap.heap_size = crate::memory::heap::HEAP_SIZE;

    // Derived: pressure
    if snap.total_pages > 0 {
        snap.free_pct_x10 = (snap.free_pages * 1000) / snap.total_pages;
    }
    snap.pressure_level = if snap.free_pct_x10 <= PRESSURE_CRIT_PCT_X10 {
        4
    } else if snap.free_pct_x10 <= PRESSURE_HIGH_PCT_X10 {
        3
    } else if snap.free_pct_x10 <= PRESSURE_MED_PCT_X10 {
        2
    } else if snap.free_pct_x10 <= PRESSURE_LOW_PCT_X10 {
        1
    } else {
        0
    };

    snap
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Take a new snapshot and store it
pub fn update() {
    let snap = gather_snapshot();
    let mut mgr = STATS.lock();
    mgr.tick += 1;
    let mut s = snap;
    s.tick = mgr.tick;

    mgr.current = s;
    let idx = mgr.history_idx;
    mgr.history[idx] = s;
    mgr.history_idx = (idx + 1) % HISTORY_SIZE;
    mgr.snapshot_count += 1;
}

/// Get the current snapshot (does NOT refresh -- call update() first)
pub fn current() -> MemSnapshot {
    STATS.lock().current
}

/// Get a fresh snapshot (calls update internally)
pub fn snapshot() -> MemSnapshot {
    update();
    STATS.lock().current
}

/// Get recent history snapshots (newest first)
pub fn history() -> alloc::vec::Vec<MemSnapshot> {
    let mgr = STATS.lock();
    let mut result = alloc::vec::Vec::new();
    for i in 0..HISTORY_SIZE {
        let idx = if mgr.history_idx > i {
            mgr.history_idx - i - 1
        } else {
            HISTORY_SIZE - (i + 1 - mgr.history_idx)
        };
        let snap = mgr.history[idx];
        if snap.tick > 0 {
            result.push(snap);
        }
    }
    result
}

/// Get the current memory pressure level (0-4)
pub fn pressure_level() -> u8 {
    let snap = gather_snapshot();
    snap.pressure_level
}

/// Get free memory in KB
pub fn free_kb() -> usize {
    let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
    fa.free_count() * (PAGE_SIZE / 1024)
}

/// Get used memory in KB
pub fn used_kb() -> usize {
    let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
    fa.used_count() * (PAGE_SIZE / 1024)
}

/// Get total memory in KB
pub fn total_kb() -> usize {
    crate::memory::frame_allocator::MAX_MEMORY / 1024
}

/// Format a /proc/meminfo-style string
pub fn meminfo() -> alloc::string::String {
    use alloc::format;
    let s = snapshot();

    let total_kb = s.total_pages * (PAGE_SIZE / 1024);
    let free_kb = s.free_pages * (PAGE_SIZE / 1024);
    let used_kb = s.used_pages * (PAGE_SIZE / 1024);
    let cached_kb = s.cached_pages * (PAGE_SIZE / 1024);
    let dirty_kb = s.dirty_pages * (PAGE_SIZE / 1024);
    let buddy_free_kb = s.buddy_free * (PAGE_SIZE / 1024);
    let heap_kb = s.heap_size / 1024;

    let pressure_str = match s.pressure_level {
        0 => "none",
        1 => "low",
        2 => "medium",
        3 => "high",
        4 => "critical",
        _ => "unknown",
    };

    let mut out = alloc::string::String::new();
    out.push_str(&format!("MemTotal:      {:>8} kB\n", total_kb));
    out.push_str(&format!("MemFree:       {:>8} kB\n", free_kb));
    out.push_str(&format!("MemUsed:       {:>8} kB\n", used_kb));
    out.push_str(&format!("Cached:        {:>8} kB\n", cached_kb));
    out.push_str(&format!("Dirty:         {:>8} kB\n", dirty_kb));
    out.push_str(&format!("BuddyFree:     {:>8} kB\n", buddy_free_kb));
    out.push_str(&format!("BuddyFrag:     {:>8}/1000\n", s.buddy_frag));
    out.push_str(&format!("KernelHeap:    {:>8} kB\n", heap_kb));
    out.push_str(&format!("SwapOut:       {:>8}\n", s.swap_out_total));
    out.push_str(&format!("SwapIn:        {:>8}\n", s.swap_in_total));
    out.push_str(&format!("SlabCaches:    {:>8}\n", s.slab_caches));
    out.push_str(&format!("SlabActive:    {:>8} objs\n", s.slab_active_objs));
    out.push_str(&format!("SlabTotal:     {:>8} objs\n", s.slab_total_objs));
    out.push_str(&format!("SlabBytes:     {:>8} kB\n", s.slab_bytes / 1024));
    let huge_page_size_kb = crate::memory::buddy::HUGE_PAGE_SIZE / 1024;
    out.push_str(&format!("HugePages_Total: {:>6}\n", s.huge_pages_total));
    out.push_str(&format!("HugePages_Free:  {:>6}\n", s.huge_pages_free));
    out.push_str(&format!("Hugepagesize:    {:>6} kB\n", huge_page_size_kb));
    out.push_str(&format!("THPPromotions: {:>8}\n", s.thp_promotions));
    out.push_str(&format!("THPSplits:     {:>8}\n", s.thp_splits));
    out.push_str(&format!(
        "CacheHitRate:  {:>5}.{:02}%\n",
        s.cache_hit_rate_x100 / 100,
        s.cache_hit_rate_x100 % 100
    ));
    out.push_str(&format!("Pressure:      {}\n", pressure_str));
    out.push_str(&format!(
        "FreePct:       {:>5}.{}%\n",
        s.free_pct_x10 / 10,
        s.free_pct_x10 % 10
    ));
    out
}

/// Initialize stats subsystem
pub fn init() {
    update();
    serial_println!("  [stats] memory statistics initialized");
}
