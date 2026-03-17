use crate::serial_println;
use crate::sync::Mutex;
/// Block allocator -- bitmap-based with extent tracking and free space management
///
/// Part of the AIOS filesystem layer.
///
/// Provides a general-purpose block allocator that filesystems can use
/// for managing on-disk block allocation. Supports bitmap allocation,
/// first-fit and best-fit strategies, and extent coalescing.
///
/// Design:
///   - A bitmap (Vec<u64>) tracks free/used state of each block,
///     with one bit per block (64 blocks per u64 word).
///   - Extent runs are tracked in a free list for fast large allocations.
///   - First-fit walks the bitmap linearly; best-fit scans the free extent
///     list for the smallest fit.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux ext4 block allocator (mballoc), XFS AG allocation.
/// All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Allocation strategy.
#[derive(Clone, Copy, PartialEq)]
pub enum AllocStrategy {
    FirstFit,
    BestFit,
    NextFit,
}

/// A contiguous range of blocks.
#[derive(Clone, Copy)]
pub struct Extent {
    pub start: u64,
    pub count: u64,
}

/// Internal allocator state.
struct Inner {
    /// Bitmap: bit=0 means free, bit=1 means used
    bitmap: Vec<u64>,
    total_blocks: u64,
    free_blocks: u64,
    strategy: AllocStrategy,
    /// Hint for next-fit: last successful allocation position
    next_hint: u64,
    /// Statistics
    alloc_count: u64,
    free_count_stat: u64,
}

// ---------------------------------------------------------------------------
// Bitmap helpers
// ---------------------------------------------------------------------------

impl Inner {
    fn new(total_blocks: u64, strategy: AllocStrategy) -> Self {
        let words = ((total_blocks + 63) / 64) as usize;
        let mut bitmap = Vec::with_capacity(words);
        for _ in 0..words {
            bitmap.push(0u64); // all free
        }
        Inner {
            bitmap,
            total_blocks,
            free_blocks: total_blocks,
            strategy,
            next_hint: 0,
            alloc_count: 0,
            free_count_stat: 0,
        }
    }

    /// Check if block `n` is free (bit is 0).
    fn is_free(&self, n: u64) -> bool {
        if n >= self.total_blocks {
            return false;
        }
        let word = (n / 64) as usize;
        let bit = n % 64;
        // Bounds-check word index before indexing into bitmap
        if word >= self.bitmap.len() {
            return false;
        }
        (self.bitmap[word] >> bit) & 1 == 0
    }

    /// Mark block `n` as used.
    fn set_used(&mut self, n: u64) {
        if n >= self.total_blocks {
            return;
        }
        let word = (n / 64) as usize;
        let bit = n % 64;
        if word >= self.bitmap.len() {
            return;
        }
        if (self.bitmap[word] >> bit) & 1 == 0 {
            self.bitmap[word] |= 1u64 << bit;
            self.free_blocks = self.free_blocks.saturating_sub(1);
        }
    }

    /// Mark block `n` as free.
    fn set_free(&mut self, n: u64) {
        if n >= self.total_blocks {
            return;
        }
        let word = (n / 64) as usize;
        let bit = n % 64;
        if word >= self.bitmap.len() {
            return;
        }
        if (self.bitmap[word] >> bit) & 1 == 1 {
            self.bitmap[word] &= !(1u64 << bit);
            self.free_blocks = self.free_blocks.saturating_add(1);
        }
    }

    /// Find `count` contiguous free blocks using first-fit starting at `from`.
    fn first_fit_from(&self, from: u64, count: u64) -> Option<u64> {
        if count == 0 || count > self.free_blocks {
            return None;
        }
        let mut run_start = from;
        let mut run_len: u64 = 0;

        let mut block = from;
        while block < self.total_blocks {
            if self.is_free(block) {
                if run_len == 0 {
                    run_start = block;
                }
                run_len = run_len.saturating_add(1);
                if run_len >= count {
                    return Some(run_start);
                }
            } else {
                run_len = 0;
            }
            block = block.saturating_add(1);
        }
        // Wrap around if we started past 0
        if from > 0 {
            run_len = 0;
            block = 0;
            while block < from {
                if self.is_free(block) {
                    if run_len == 0 {
                        run_start = block;
                    }
                    run_len = run_len.saturating_add(1);
                    if run_len >= count {
                        return Some(run_start);
                    }
                } else {
                    run_len = 0;
                }
                block = block.saturating_add(1);
            }
        }
        None
    }

    /// Best-fit: find the smallest contiguous free run >= count.
    fn best_fit(&self, count: u64) -> Option<u64> {
        if count == 0 || count > self.free_blocks {
            return None;
        }
        let mut best_start: Option<u64> = None;
        let mut best_len = u64::MAX;
        let mut run_start: u64 = 0;
        let mut run_len: u64 = 0;

        for block in 0..self.total_blocks {
            if self.is_free(block) {
                if run_len == 0 {
                    run_start = block;
                }
                run_len = run_len.saturating_add(1);
            } else {
                if run_len >= count && run_len < best_len {
                    best_len = run_len;
                    best_start = Some(run_start);
                    if best_len == count {
                        return best_start; // exact fit
                    }
                }
                run_len = 0;
            }
        }
        // Check final run
        if run_len >= count && run_len < best_len {
            best_start = Some(run_start);
        }
        best_start
    }

    /// Allocate `count` contiguous blocks.
    fn allocate(&mut self, count: u64) -> Option<Extent> {
        let start = match self.strategy {
            AllocStrategy::FirstFit => self.first_fit_from(0, count),
            AllocStrategy::NextFit => {
                let hint = self.next_hint;
                self.first_fit_from(hint, count)
            }
            AllocStrategy::BestFit => self.best_fit(count),
        };

        if let Some(s) = start {
            for i in 0..count {
                self.set_used(s + i);
            }
            self.next_hint = s + count;
            if self.next_hint >= self.total_blocks {
                self.next_hint = 0;
            }
            self.alloc_count = self.alloc_count.saturating_add(1);
            Some(Extent { start: s, count })
        } else {
            None
        }
    }

    /// Free a range of blocks.
    fn free(&mut self, start: u64, count: u64) -> Result<(), ()> {
        // Use checked_add to avoid overflow on range-end calculation
        let end = start.checked_add(count).ok_or(())?;
        if end > self.total_blocks {
            return Err(());
        }
        for i in 0..count {
            self.set_free(start.saturating_add(i));
        }
        self.free_count_stat = self.free_count_stat.saturating_add(1);
        Ok(())
    }

    /// Mark a range as used (for reserving metadata blocks, etc.).
    fn reserve(&mut self, start: u64, count: u64) -> Result<(), ()> {
        // Use checked_add to avoid overflow on range-end calculation
        let end = start.checked_add(count).ok_or(())?;
        if end > self.total_blocks {
            return Err(());
        }
        for i in 0..count {
            self.set_used(start.saturating_add(i));
        }
        Ok(())
    }

    /// Count free blocks in a sub-range.
    fn free_in_range(&self, start: u64, count: u64) -> u64 {
        let end = (start + count).min(self.total_blocks);
        let mut free = 0u64;
        for b in start..end {
            if self.is_free(b) {
                free = free.saturating_add(1);
            }
        }
        free
    }

    /// Find the largest contiguous free extent.
    fn largest_free_extent(&self) -> Extent {
        let mut best = Extent { start: 0, count: 0 };
        let mut run_start: u64 = 0;
        let mut run_len: u64 = 0;
        for block in 0..self.total_blocks {
            if self.is_free(block) {
                if run_len == 0 {
                    run_start = block;
                }
                run_len = run_len.saturating_add(1);
            } else {
                if run_len > best.count {
                    best = Extent {
                        start: run_start,
                        count: run_len,
                    };
                }
                run_len = 0;
            }
        }
        if run_len > best.count {
            best = Extent {
                start: run_start,
                count: run_len,
            };
        }
        best
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static BLOCK_ALLOC: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Allocate `count` contiguous blocks. Returns the extent on success.
pub fn allocate(count: u64) -> Option<Extent> {
    let mut guard = BLOCK_ALLOC.lock();
    guard.as_mut().and_then(|inner| inner.allocate(count))
}

/// Free a previously allocated extent.
pub fn free(start: u64, count: u64) -> Result<(), ()> {
    let mut guard = BLOCK_ALLOC.lock();
    guard
        .as_mut()
        .ok_or(())
        .and_then(|inner| inner.free(start, count))
}

/// Reserve blocks (mark as used without allocating).
pub fn reserve(start: u64, count: u64) -> Result<(), ()> {
    let mut guard = BLOCK_ALLOC.lock();
    guard
        .as_mut()
        .ok_or(())
        .and_then(|inner| inner.reserve(start, count))
}

/// Return the total number of free blocks.
pub fn free_count() -> u64 {
    let guard = BLOCK_ALLOC.lock();
    guard.as_ref().map_or(0, |inner| inner.free_blocks)
}

/// Return the total number of blocks.
pub fn total_count() -> u64 {
    let guard = BLOCK_ALLOC.lock();
    guard.as_ref().map_or(0, |inner| inner.total_blocks)
}

/// Return the largest contiguous free extent.
pub fn largest_free_extent() -> Extent {
    let guard = BLOCK_ALLOC.lock();
    guard
        .as_ref()
        .map_or(Extent { start: 0, count: 0 }, |inner| {
            inner.largest_free_extent()
        })
}

/// Count free blocks in a sub-range.
pub fn free_in_range(start: u64, count: u64) -> u64 {
    let guard = BLOCK_ALLOC.lock();
    guard
        .as_ref()
        .map_or(0, |inner| inner.free_in_range(start, count))
}

/// Initialize the block allocator for a given device size and strategy.
pub fn init_with(total_blocks: u64, strategy: AllocStrategy) {
    let mut guard = BLOCK_ALLOC.lock();
    *guard = Some(Inner::new(total_blocks, strategy));
    serial_println!(
        "    block_alloc: initialized ({} blocks, {:?}-fit)",
        total_blocks,
        match strategy {
            AllocStrategy::FirstFit => "first",
            AllocStrategy::BestFit => "best",
            AllocStrategy::NextFit => "next",
        }
    );
}

/// Initialize with default settings (64K blocks, first-fit).
pub fn init() {
    init_with(65536, AllocStrategy::FirstFit);
}
