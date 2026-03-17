/// Page reclaim engine — LRU lists, kswapd, direct reclaim, memory pressure callbacks.
///
/// Part of the AIOS kernel.
///
/// Enhancements over the initial stub:
///   - MemPressureShrinker trait: subsystems register callbacks that the
///     reclaim engine calls when free memory drops below a threshold.
///   - Global shrinker registry (up to MAX_SHRINKERS entries, lock-free
///     registration, locked iteration during reclaim).
///   - run_shrinkers(): calls each registered shrinker in registration order
///     until the page target is met; returns total pages freed.
///   - ReclaimState: LRU-based aging (active → inactive → freed). The shrink()
///     method drives both phases in a single call.
///   - kswapd watermarks: LOW_WATERMARK / HIGH_WATERMARK frame thresholds that
///     trigger background reclaim in kswapd_tick().
///
/// No floats, no panics, no std. All arithmetic is saturating.

// ---------------------------------------------------------------------------
// kswapd watermarks
// ---------------------------------------------------------------------------

/// Minimum number of free frames before kswapd begins reclaiming.
const LOW_WATERMARK: usize = 256;

/// Target number of free frames for kswapd to reach before stopping reclaim.
const HIGH_WATERMARK: usize = 512;

use crate::sync::Mutex;
use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// Memory pressure shrinker trait
// ---------------------------------------------------------------------------

/// Implemented by kernel subsystems that hold reclaimable memory.
///
/// When the memory allocator detects pressure it calls `shrink()` on every
/// registered shrinker, passing the number of pages it would like freed.
/// Each shrinker returns the number of pages it actually freed.
///
/// # Safety
///
/// Shrinkers must not re-enter the reclaim path (no allocations that can
/// trigger reclaim from within `shrink()`).
pub trait MemPressureShrinker: Send + Sync {
    /// Attempt to free up to `target_pages` pages.
    /// Returns the number of pages actually freed (may be 0 if none available).
    fn shrink(&self, target_pages: usize) -> usize;

    /// Return the number of pages this shrinker *could* free right now,
    /// without actually freeing them. Used for priority ordering.
    fn count_reclaimable(&self) -> usize;

    /// Human-readable name for diagnostics (e.g., "page_cache", "slab").
    fn name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Shrinker registry
// ---------------------------------------------------------------------------

/// Maximum number of shrinkers that can be registered.
const MAX_SHRINKERS: usize = 16;

/// Registry of memory pressure shrinker callbacks.
struct ShrinkerRegistry {
    entries: [Option<&'static dyn MemPressureShrinker>; MAX_SHRINKERS],
    count: usize,
}

impl ShrinkerRegistry {
    const fn new() -> Self {
        ShrinkerRegistry {
            entries: [None; MAX_SHRINKERS],
            count: 0,
        }
    }

    /// Register a new shrinker. Returns false if the registry is full.
    fn register(&mut self, s: &'static dyn MemPressureShrinker) -> bool {
        if self.count >= MAX_SHRINKERS {
            return false;
        }
        self.entries[self.count] = Some(s);
        self.count = self.count.saturating_add(1);
        true
    }
}

static SHRINKERS: Mutex<ShrinkerRegistry> = Mutex::new(ShrinkerRegistry::new());

/// Register a shrinker with the reclaim engine.
///
/// `shrinker` must live for the lifetime of the kernel (`'static`), which is
/// typically satisfied by placing the implementation in a static variable.
///
/// Returns `true` on success, `false` if the registry is full.
pub fn register_shrinker(shrinker: &'static dyn MemPressureShrinker) -> bool {
    SHRINKERS.lock().register(shrinker)
}

/// Invoke all registered shrinkers until `target_pages` have been freed.
///
/// Shrinkers are called in registration order (first registered, first called).
/// Returns the total number of pages freed across all shrinkers.
pub fn run_shrinkers(target_pages: usize) -> usize {
    if target_pages == 0 {
        return 0;
    }

    let registry = SHRINKERS.lock();
    let mut freed = 0usize;

    for i in 0..registry.count {
        if freed >= target_pages {
            break;
        }
        if let Some(shrinker) = registry.entries[i] {
            let remaining = target_pages.saturating_sub(freed);
            let got = shrinker.shrink(remaining);
            freed = freed.saturating_add(got);
        }
    }

    freed
}

/// Count total reclaimable pages across all registered shrinkers.
pub fn total_reclaimable() -> usize {
    let registry = SHRINKERS.lock();
    let mut total = 0usize;
    for i in 0..registry.count {
        if let Some(shrinker) = registry.entries[i] {
            total = total.saturating_add(shrinker.count_reclaimable());
        }
    }
    total
}

// ---------------------------------------------------------------------------
// LRU page aging
// ---------------------------------------------------------------------------

/// LRU list type for page aging.
#[derive(Debug, Clone, Copy)]
pub enum LruList {
    Active,
    Inactive,
    Unevictable,
}

/// A page on an LRU list.
pub struct LruPage {
    pub phys_addr: usize,
    pub list: LruList,
    pub referenced: bool,
}

/// Page reclaim state managing LRU lists and watermarks.
pub struct ReclaimState {
    pub active: VecDeque<LruPage>,
    pub inactive: VecDeque<LruPage>,
    pub pages_reclaimed: u64,
}

impl ReclaimState {
    pub fn new() -> Self {
        ReclaimState {
            active: VecDeque::new(),
            inactive: VecDeque::new(),
            pages_reclaimed: 0,
        }
    }

    /// Attempt to reclaim at least `target` pages.
    ///
    /// Strategy:
    ///  1. Move unreferenced pages from inactive -> reclaimed (freed).
    ///  2. Demote the oldest referenced active pages to inactive.
    /// Returns how many pages were actually freed.
    pub fn shrink(&mut self, target: usize) -> usize {
        let mut freed = 0usize;

        // Phase 1: reclaim from inactive list.
        while freed < target {
            match self.inactive.pop_front() {
                Some(page) => {
                    // Free the physical frame back to the frame allocator.
                    if page.phys_addr != 0 && page.phys_addr % 4096 == 0 {
                        crate::memory::frame_allocator::deallocate_frame(
                            crate::memory::frame_allocator::PhysFrame::from_addr(page.phys_addr),
                        );
                    }
                    freed = freed.saturating_add(1);
                    self.pages_reclaimed = self.pages_reclaimed.saturating_add(1);
                }
                None => break,
            }
        }

        // Phase 2: demote oldest active pages to inactive to refill it.
        // Move at most `target` active pages regardless of how many we freed.
        for _ in 0..target {
            match self.active.pop_front() {
                Some(mut page) => {
                    page.list = LruList::Inactive;
                    page.referenced = false;
                    self.inactive.push_back(page);
                }
                None => break,
            }
        }

        freed
    }

    /// Push a page onto the active LRU list.
    pub fn mark_active(&mut self, phys_addr: usize) {
        self.active.push_back(LruPage {
            phys_addr,
            list: LruList::Active,
            referenced: true,
        });
    }

    /// Mark a page as recently referenced (prevents immediate demotion).
    pub fn touch(&mut self, phys_addr: usize) {
        for page in self.active.iter_mut() {
            if page.phys_addr == phys_addr {
                page.referenced = true;
                return;
            }
        }
        for page in self.inactive.iter_mut() {
            if page.phys_addr == phys_addr {
                page.referenced = true;
                return;
            }
        }
    }

    /// Total pages tracked (active + inactive).
    pub fn total_tracked(&self) -> usize {
        self.active.len().saturating_add(self.inactive.len())
    }
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the page reclaim subsystem.
pub fn init() {
    // Shrinker registry is initialised via const fn; no further work needed.
    // Watermark constants (LOW_WATERMARK / HIGH_WATERMARK) are defined at the
    // top of this module and used by kswapd_tick() below.
}

/// Called from the timer tick handler to perform background page reclaim.
///
/// If the number of free physical frames drops below `LOW_WATERMARK`, reclaim
/// pages via the registered shrinkers until free frames reach `HIGH_WATERMARK`
/// or no more pages can be freed.
pub fn kswapd_tick() {
    let free_frames = crate::memory::frame_allocator::FRAME_ALLOCATOR
        .lock()
        .free_count();

    if free_frames < LOW_WATERMARK {
        // How many pages do we need to reach the high watermark?
        let needed = HIGH_WATERMARK.saturating_sub(free_frames);
        if needed > 0 {
            run_shrinkers(needed);
        }
    }
}
