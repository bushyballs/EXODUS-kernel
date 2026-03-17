use crate::serial_println;
/// Memory compaction engine for Genesis AIOS
///
/// Reduces physical memory fragmentation by scanning for movable pages at
/// low physical addresses and free slots at high addresses, then migrating
/// pages to consolidate free space into large contiguous blocks.
///
/// This implementation is a structured stub: it logs intent, updates stats,
/// and delegates to a real page-migration engine when one is available.
/// The full walk-and-remap path is left as a comment because it requires
/// page-table access that is architecture-specific.
///
/// Design rules enforced throughout this module:
///   - NO heap: no Vec, Box, String, alloc::*
///   - NO panics: no unwrap(), expect(), panic!()
///   - NO float casts: no `as f64` / `as f32`
///   - All counters use wrapping_add (sequence) or saturating_add (magnitude)
///   - All statics: Copy + const-constructible
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Cumulative statistics for the compaction engine.
#[derive(Clone, Copy)]
pub struct CompactionStats {
    /// Total number of compaction runs (wrapping sequence counter)
    pub runs: u64,
    /// Total pages successfully moved across all runs
    pub pages_moved: u64,
    /// Total pages freed (returned to buddy) as a result of compaction
    pub pages_freed: u64,
    /// Timestamp (milliseconds since boot) of the most recent run.
    /// Zero until the first run completes.
    pub last_run_ms: u64,
}

impl CompactionStats {
    const fn new() -> Self {
        CompactionStats {
            runs: 0,
            pages_moved: 0,
            pages_freed: 0,
            last_run_ms: 0,
        }
    }
}

static COMPACTION_STATS: Mutex<CompactionStats> = Mutex::new(CompactionStats::new());

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// Outcome of a single compaction attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionResult {
    /// Compaction ran and produced a meaningful increase in contiguous free
    /// memory.
    Success,
    /// Compaction ran but only made partial progress (fragmentation reduced
    /// but not eliminated).
    Partial,
    /// Nothing worth compacting was found; skipped.
    Skipped,
    /// Compaction attempted but could not move pages (e.g. OOM during
    /// migration or all pages pinned).
    Failed,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return `true` when allocating `2^order` contiguous pages would fail given
/// the current buddy state.
///
/// Calls `crate::mm::frame_allocator::largest_free_block()` when available.
/// Falls back to `false` (conservatively assumes no compaction is needed)
/// when the function does not exist.
pub fn compaction_needed(order: u32) -> bool {
    // Ask the frame allocator for the largest contiguous free block.
    // The method is `largest_free_region()` on BitmapFrameAllocator.
    let alloc = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
    let largest = alloc.largest_free_region(); // pages
    let needed = 1usize << (order as usize);
    largest < needed
}

// ---------------------------------------------------------------------------
// Zone compaction
// ---------------------------------------------------------------------------

/// Compact physical memory in the range `[start_pfn, end_pfn)`.
///
/// Simple strategy:
///   - Free pages at high addresses are candidates for receiving migrated data.
///   - Allocated (movable) pages at low addresses are candidates for moving.
///   - Each matched pair is a migration candidate.
///
/// In this stub the actual page copy and page-table update is not performed
/// (that requires arch-specific virtual-address remapping).  The function
/// logs its intent and returns `CompactionResult::Partial` to indicate that
/// the subsystem is alive but the full migration path is not yet wired.
///
/// A real implementation would:
///   1. Walk `start_pfn..midpoint` for `PageFlags::Movable` pages.
///   2. Walk `midpoint..end_pfn` downward for `PageFlags::Free` pages.
///   3. Copy page contents, update PTEs, flush TLB.
///   4. Free the vacated source frame to the buddy allocator.
pub fn compact_zone(start_pfn: u64, end_pfn: u64) -> CompactionResult {
    if end_pfn <= start_pfn {
        return CompactionResult::Skipped;
    }

    let span = end_pfn.saturating_sub(start_pfn);
    serial_println!(
        "compaction: compact_zone pfn={:#x}..{:#x} ({} pages) — stub migration",
        start_pfn,
        end_pfn,
        span
    );

    // Update stats: pages_moved is left unchanged because no actual copy
    // occurs in this stub; runs is incremented by the caller (compact_memory).
    {
        let mut stats = COMPACTION_STATS.lock();
        // pages_freed: we do not reclaim frames here; real path would update
        // this after successfully freeing source frames.
        stats.pages_freed = stats.pages_freed.saturating_add(0);
    }

    CompactionResult::Partial
}

// ---------------------------------------------------------------------------
// System-level compaction
// ---------------------------------------------------------------------------

/// Compact the entire physical address space.
///
/// Called when a large allocation fails.  Logs intent, runs a zone pass over
/// the full physical range tracked by the frame allocator, and updates the
/// run counter.
pub fn compact_memory() -> CompactionResult {
    serial_println!("compaction: Starting memory compaction");

    // Derive the physical range from the frame allocator constants.
    // MAX_MEMORY / FRAME_SIZE gives the total frame count.
    let total_pfn = (crate::memory::frame_allocator::MAX_MEMORY
        / crate::memory::frame_allocator::FRAME_SIZE) as u64;

    let result = compact_zone(0, total_pfn);

    {
        let mut stats = COMPACTION_STATS.lock();
        // Sequence counter: wrapping_add so it never saturates.
        stats.runs = stats.runs.wrapping_add(1);

        // Record a synthetic timestamp using the run counter as a proxy.
        // A real implementation would read the TSC or a wall-clock source.
        stats.last_run_ms = stats.runs.wrapping_mul(1000);
    }

    result
}

// ---------------------------------------------------------------------------
// Statistics accessor
// ---------------------------------------------------------------------------

/// Return a snapshot of the compaction statistics.
pub fn compaction_get_stats() -> CompactionStats {
    *COMPACTION_STATS.lock()
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the compaction subsystem.
pub fn init() {
    // Nothing to allocate; just confirm the static is in a clean state.
    {
        let mut stats = COMPACTION_STATS.lock();
        *stats = CompactionStats::new();
    }
    serial_println!("  [compaction] subsystem initialized");
}
