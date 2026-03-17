/// Buddy allocator for Genesis — power-of-2 physical page allocation
///
/// Implements a classic buddy system allocator that manages physical memory
/// in blocks of power-of-2 pages. Supports orders 0-10 (4KB to 4MB).
///
/// When a block of order N is freed, the allocator checks if its "buddy"
/// (the adjacent block of the same order) is also free. If so, they merge
/// into a block of order N+1. This coalescing prevents external fragmentation.
///
/// Features:
///   - Order-based free lists (order 0 = 4KB through order 10 = 4MB)
///   - Buddy finding via XOR address with block size
///   - Split higher-order blocks when lower order is empty
///   - Recursive buddy coalescing on free
///   - Per-order statistics (free count, alloc count)
///   - Allocation watermarks (min/low/high per zone)
///   - Compaction trigger when fragmentation is high
///   - Reserved pool for emergency allocations (GFP_ATOMIC equivalent)
///
/// Inspired by: Linux buddy allocator (mm/page_alloc.c). All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

/// Maximum order (2^MAX_ORDER pages = 4MB blocks at order 10)
pub const MAX_ORDER: usize = 11; // orders 0..10

/// Page size (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Maximum physical memory we manage (512 MB)
pub const MAX_MEMORY: usize = 512 * 1024 * 1024;

/// Total pages
pub const MAX_PAGES: usize = MAX_MEMORY / PAGE_SIZE;

/// Number of pages reserved for emergency (GFP_ATOMIC) allocations
const EMERGENCY_RESERVE_PAGES: usize = 256; // 1 MB

/// Watermark levels (as percentage * 10 of total pages)
const WATERMARK_MIN_PCT_X10: usize = 20; // 2.0%
const WATERMARK_LOW_PCT_X10: usize = 50; // 5.0%
const WATERMARK_HIGH_PCT_X10: usize = 100; // 10.0%

/// Fragmentation threshold (per-mille) above which compaction is suggested
const FRAGMENTATION_COMPACT_THRESHOLD: usize = 500; // 50%

/// Page flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageFlags {
    Free = 0,
    Allocated = 1,
    Kernel = 2,
    Slab = 3,
    PageCache = 4,
    Reserved = 5,
    Emergency = 6,
    Movable = 7,
}

/// Per-page metadata
#[derive(Clone, Copy)]
pub struct PageInfo {
    /// Flags for this page
    pub flags: PageFlags,
    /// Order of the block this page belongs to (for free blocks, head page only)
    pub order: u8,
    /// Reference count (for shared pages, COW, page cache)
    pub refcount: u16,
    /// Mapping: which address space / file this page belongs to
    pub mapping: u32,
}

impl PageInfo {
    const fn new() -> Self {
        PageInfo {
            flags: PageFlags::Reserved,
            order: 0,
            refcount: 0,
            mapping: 0,
        }
    }
}

/// Free list node — embedded in free pages (at the start of the free block)
/// Since free pages aren't used for anything, we store the linked list there
struct FreeNode {
    next: usize, // physical address of next free block (0 = end)
    prev: usize, // physical address of prev free block (0 = end)
}

/// A free list for one order
struct FreeList {
    /// Head of the doubly-linked list (physical address, 0 = empty)
    head: usize,
    /// Number of free blocks at this order
    count: usize,
}

impl FreeList {
    const fn new() -> Self {
        FreeList { head: 0, count: 0 }
    }
}

/// Per-order statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct OrderStats {
    /// Number of allocations at this order
    pub alloc_count: u64,
    /// Number of frees at this order
    pub free_count: u64,
    /// Current free blocks at this order
    pub current_free: usize,
}

/// Watermark levels for the zone
#[derive(Debug, Clone, Copy)]
pub struct Watermarks {
    /// Minimum free pages before emergency-only allocations
    pub min: usize,
    /// Low free pages — triggers background reclaim (kswapd)
    pub low: usize,
    /// High free pages — reclaim can stop
    pub high: usize,
}

impl Watermarks {
    const fn new() -> Self {
        Watermarks {
            min: 0,
            low: 0,
            high: 0,
        }
    }

    fn compute(total_pages: usize) -> Self {
        Watermarks {
            min: (total_pages * WATERMARK_MIN_PCT_X10) / 1000,
            low: (total_pages * WATERMARK_LOW_PCT_X10) / 1000,
            high: (total_pages * WATERMARK_HIGH_PCT_X10) / 1000,
        }
    }
}

/// Allocation flags (similar to Linux GFP flags)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocFlags {
    /// Normal allocation — may sleep, may reclaim
    Normal,
    /// Atomic allocation — cannot sleep, uses emergency reserves
    Atomic,
    /// Movable allocation — pages can be relocated for compaction
    Movable,
    /// Kernel allocation — for kernel data structures
    Kernel,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BuddyStats {
    pub alloc_count: u64,
    pub free_count: u64,
    pub split_count: u64,
    pub merge_count: u64,
    pub alloc_failures: u64,
    pub emergency_allocs: u64,
    pub compaction_runs: u64,
}

/// The buddy allocator
pub struct BuddyAllocator {
    /// Free lists, one per order (0..MAX_ORDER)
    free_lists: [FreeList; MAX_ORDER],
    /// Per-page metadata array
    pages: [PageInfo; MAX_PAGES],
    /// Total free pages
    free_pages: usize,
    /// Total managed pages
    total_pages: usize,
    /// Start of managed physical memory
    base_addr: usize,
    /// Per-order statistics
    order_stats: [OrderStats; MAX_ORDER],
    /// Watermarks
    watermarks: Watermarks,
    /// Emergency reserve free pages
    emergency_free: usize,
    /// Emergency reserve maximum
    emergency_max: usize,
    /// Whether compaction has been suggested
    compact_needed: bool,
    /// Statistics
    pub stats: BuddyStats,
}

impl BuddyAllocator {
    const fn new() -> Self {
        const EMPTY_FREE_LIST: FreeList = FreeList::new();
        const EMPTY_PAGE: PageInfo = PageInfo::new();
        const EMPTY_ORDER_STATS: OrderStats = OrderStats {
            alloc_count: 0,
            free_count: 0,
            current_free: 0,
        };
        BuddyAllocator {
            free_lists: [EMPTY_FREE_LIST; MAX_ORDER],
            pages: [EMPTY_PAGE; MAX_PAGES],
            free_pages: 0,
            total_pages: 0,
            base_addr: 0,
            order_stats: [EMPTY_ORDER_STATS; MAX_ORDER],
            watermarks: Watermarks::new(),
            emergency_free: 0,
            emergency_max: EMERGENCY_RESERVE_PAGES,
            compact_needed: false,
            stats: BuddyStats {
                alloc_count: 0,
                free_count: 0,
                split_count: 0,
                merge_count: 0,
                alloc_failures: 0,
                emergency_allocs: 0,
                compaction_runs: 0,
            },
        }
    }

    /// Initialize the buddy allocator with free memory starting at `start_addr`
    pub fn init(&mut self, start_addr: usize, end_addr: usize) {
        self.base_addr = 0; // we manage from physical 0
        let mut start_page = (start_addr + PAGE_SIZE - 1) / PAGE_SIZE; // round up
        let mut end_page = end_addr / PAGE_SIZE;
        if end_page > MAX_PAGES {
            end_page = MAX_PAGES;
        }
        if start_page > end_page {
            start_page = end_page;
        }
        self.total_pages = end_page;

        // Compute watermarks based on total managed pages
        self.watermarks = Watermarks::compute(end_page);

        // Mark all pages below start as reserved
        for i in 0..start_page {
            self.pages[i].flags = PageFlags::Reserved;
        }

        // Add free pages in largest possible buddy blocks
        let mut page = start_page;
        while page < end_page {
            // Find the largest order block that fits and is properly aligned
            let mut order = MAX_ORDER - 1;
            loop {
                let block_pages = 1 << order;
                let aligned = page % block_pages == 0;
                let fits = page + block_pages <= end_page;
                if aligned && fits {
                    break;
                }
                if order == 0 {
                    break;
                }
                order -= 1;
            }

            let block_pages = 1 << order;
            if page % block_pages == 0 && page + block_pages <= end_page {
                // Add this block to the free list
                self.add_to_free_list(page, order);
                for i in 0..block_pages {
                    self.pages[page + i].flags = PageFlags::Free;
                    self.pages[page + i].order = order as u8;
                }
                self.free_pages += block_pages;
                page += block_pages;
            } else {
                // Can't place any block here, skip this page
                page += 1;
            }
        }

        // Seed the emergency reserve from order-0 pages
        self.fill_emergency_reserve();
    }

    /// Fill the emergency reserve pool from regular free pages
    fn fill_emergency_reserve(&mut self) {
        // We don't actually separate memory; we just track a count.
        // Emergency allocations are allowed to dip below watermark_min.
        self.emergency_free = self.emergency_max.min(self.free_pages / 4);
    }

    /// Add a block to a free list
    fn add_to_free_list(&mut self, page_idx: usize, order: usize) {
        let addr = page_idx * PAGE_SIZE;
        let node = unsafe { &mut *(addr as *mut FreeNode) };
        node.next = self.free_lists[order].head;
        node.prev = 0;

        if self.free_lists[order].head != 0 {
            let old_head = unsafe { &mut *(self.free_lists[order].head as *mut FreeNode) };
            old_head.prev = addr;
        }

        self.free_lists[order].head = addr;
        self.free_lists[order].count = self.free_lists[order].count.saturating_add(1);
        self.pages[page_idx].order = order as u8;
        self.order_stats[order].current_free =
            self.order_stats[order].current_free.saturating_add(1);
    }

    /// Remove a block from a free list
    fn remove_from_free_list(&mut self, page_idx: usize, order: usize) {
        let addr = page_idx * PAGE_SIZE;
        let node = unsafe { &*(addr as *const FreeNode) };
        let next = node.next;
        let prev = node.prev;

        if prev != 0 {
            let prev_node = unsafe { &mut *(prev as *mut FreeNode) };
            prev_node.next = next;
        } else {
            self.free_lists[order].head = next;
        }

        if next != 0 {
            let next_node = unsafe { &mut *(next as *mut FreeNode) };
            next_node.prev = prev;
        }

        self.free_lists[order].count -= 1;
        if self.order_stats[order].current_free > 0 {
            self.order_stats[order].current_free -= 1;
        }
    }

    /// Get buddy page index for a given page at a given order
    fn buddy_index(&self, page_idx: usize, order: usize) -> usize {
        page_idx ^ (1 << order)
    }

    /// Check if a buddy at a given order is free and can be merged
    fn buddy_is_free(&self, buddy_page: usize, order: usize) -> bool {
        if buddy_page >= self.total_pages {
            return false;
        }
        if self.pages[buddy_page].flags != PageFlags::Free {
            return false;
        }
        if self.pages[buddy_page].order as usize != order {
            return false;
        }
        true
    }

    /// Check watermarks before allocation. Returns true if allocation is allowed.
    fn check_watermarks(&self, flags: AllocFlags) -> bool {
        match flags {
            AllocFlags::Atomic => {
                // Atomic allocations can use emergency reserve
                self.free_pages > 0
            }
            AllocFlags::Normal | AllocFlags::Movable | AllocFlags::Kernel => {
                // Normal allocations must stay above watermark_min
                self.free_pages > self.watermarks.min
            }
        }
    }

    /// Allocate 2^order contiguous pages. Returns physical address or None.
    pub fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        self.alloc_pages_flags(order, AllocFlags::Normal)
    }

    /// Allocate with explicit flags
    pub fn alloc_pages_flags(&mut self, order: usize, flags: AllocFlags) -> Option<usize> {
        if order >= MAX_ORDER {
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            return None;
        }

        // Check watermarks
        let block_pages = 1 << order;
        if !self.check_watermarks(flags) && flags != AllocFlags::Atomic {
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            return None;
        }

        // Find the smallest order that has a free block >= requested order
        let mut found_order = order;
        while found_order < MAX_ORDER && self.free_lists[found_order].count == 0 {
            found_order += 1;
        }

        if found_order >= MAX_ORDER {
            // For atomic allocations, try harder (allow dipping into reserve)
            if flags == AllocFlags::Atomic && self.emergency_free > 0 {
                // Still nothing at any order — true OOM
                self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
                self.compact_needed = true;
                return None;
            }
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            self.compact_needed = true;
            return None;
        }

        // Take a block from the found order
        let addr = self.free_lists[found_order].head;
        if addr == 0 {
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            return None;
        }

        let page_idx = addr / PAGE_SIZE;
        self.remove_from_free_list(page_idx, found_order);

        // Split down to the requested order
        while found_order > order {
            found_order -= 1;
            self.stats.split_count = self.stats.split_count.saturating_add(1);

            // The upper half becomes a free buddy
            let buddy_page = page_idx + (1 << found_order);
            self.add_to_free_list(buddy_page, found_order);

            for i in 0..(1 << found_order) {
                self.pages[buddy_page + i].flags = PageFlags::Free;
                self.pages[buddy_page + i].order = found_order as u8;
            }
        }

        // Mark allocated pages with appropriate flags
        let page_flag = match flags {
            AllocFlags::Kernel => PageFlags::Kernel,
            AllocFlags::Movable => PageFlags::Movable,
            AllocFlags::Atomic => {
                self.stats.emergency_allocs = self.stats.emergency_allocs.saturating_add(1);
                if self.emergency_free > 0 {
                    self.emergency_free -= 1;
                }
                PageFlags::Kernel
            }
            AllocFlags::Normal => PageFlags::Allocated,
        };

        for i in 0..block_pages {
            self.pages[page_idx + i].flags = page_flag;
            self.pages[page_idx + i].order = order as u8;
            self.pages[page_idx + i].refcount = 1;
        }

        self.free_pages -= block_pages;
        self.stats.alloc_count = self.stats.alloc_count.saturating_add(1);
        self.order_stats[order].alloc_count = self.order_stats[order].alloc_count.saturating_add(1);

        // Check if we need to suggest compaction
        if self.free_pages < self.watermarks.low {
            self.compact_needed = true;
        }

        Some(addr)
    }

    /// Free 2^order contiguous pages starting at `addr`
    pub fn free_pages(&mut self, addr: usize, order: usize) {
        if addr == 0 || order >= MAX_ORDER {
            return;
        }

        let page_idx = addr / PAGE_SIZE;
        if page_idx >= self.total_pages {
            return;
        }

        let block_pages = 1 << order;

        // Mark pages as free
        for i in 0..block_pages {
            self.pages[page_idx + i].flags = PageFlags::Free;
            self.pages[page_idx + i].refcount = 0;
        }
        self.free_pages += block_pages;

        // Try to merge with buddy (coalesce) — recursive upward
        let mut current_page = page_idx;
        let mut current_order = order;

        while current_order < MAX_ORDER - 1 {
            let buddy = self.buddy_index(current_page, current_order);

            if !self.buddy_is_free(buddy, current_order) {
                break;
            }

            // Merge: remove buddy from its free list
            self.remove_from_free_list(buddy, current_order);
            self.stats.merge_count = self.stats.merge_count.saturating_add(1);

            // The merged block starts at the lower address
            current_page = current_page.min(buddy);
            current_order += 1;

            // Update page info for merged block
            for i in 0..(1 << current_order) {
                self.pages[current_page + i].order = current_order as u8;
            }
        }

        // Add the (possibly merged) block to the free list
        self.add_to_free_list(current_page, current_order);
        self.stats.free_count = self.stats.free_count.saturating_add(1);
        self.order_stats[order].free_count = self.order_stats[order].free_count.saturating_add(1);

        // Replenish emergency reserve
        if self.emergency_free < self.emergency_max {
            self.emergency_free += block_pages.min(self.emergency_max - self.emergency_free);
        }

        // Clear compaction suggestion if we're above high watermark
        if self.free_pages >= self.watermarks.high {
            self.compact_needed = false;
        }
    }

    /// Allocate a single page (order 0). Returns physical address.
    pub fn alloc_page(&mut self) -> Option<usize> {
        self.alloc_pages(0)
    }

    /// Free a single page
    pub fn free_page(&mut self, addr: usize) {
        self.free_pages(addr, 0);
    }

    /// Allocate the smallest buddy block that contains at least `n` contiguous
    /// pages.  Returns the physical address of the first page, or `None` if no
    /// suitable block is available.
    ///
    /// The allocated block may be larger than `n` pages (it is always a
    /// power-of-2 number of pages).  The caller receives the address; any
    /// unused trailing pages within the block remain allocated — callers that
    /// need exact sizes should free the surplus via `free_pages`.
    ///
    /// For large `n` that are not a power of two, a higher order is used so
    /// the entire requested range falls within a single buddy block.
    pub fn alloc_contiguous(&mut self, n: usize) -> Option<usize> {
        if n == 0 {
            return None;
        }
        // Find the smallest order whose block covers n pages: 2^order >= n.
        let mut order = 0usize;
        while order < MAX_ORDER - 1 && (1usize << order) < n {
            order += 1;
        }
        if (1usize << order) < n {
            // n is larger than the maximum buddy block (2^(MAX_ORDER-1) pages).
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            return None;
        }
        self.alloc_pages(order)
    }

    /// Get page info for a physical address
    pub fn page_info(&self, addr: usize) -> Option<&PageInfo> {
        let idx = addr / PAGE_SIZE;
        if idx < self.total_pages {
            Some(&self.pages[idx])
        } else {
            None
        }
    }

    /// Get mutable page info
    pub fn page_info_mut(&mut self, addr: usize) -> Option<&mut PageInfo> {
        let idx = addr / PAGE_SIZE;
        if idx < self.total_pages {
            Some(&mut self.pages[idx])
        } else {
            None
        }
    }

    /// Increment reference count for a page (used by COW, page cache)
    pub fn get_page(&mut self, addr: usize) {
        let idx = addr / PAGE_SIZE;
        if idx < self.total_pages {
            self.pages[idx].refcount = self.pages[idx].refcount.saturating_add(1);
        }
    }

    /// Decrement reference count, free if it reaches 0
    pub fn put_page(&mut self, addr: usize) -> bool {
        let idx = addr / PAGE_SIZE;
        if idx < self.total_pages && self.pages[idx].refcount > 0 {
            self.pages[idx].refcount -= 1;
            if self.pages[idx].refcount == 0 {
                self.free_page(addr);
                return true; // page was freed
            }
        }
        false
    }

    /// Number of free pages
    pub fn free_count(&self) -> usize {
        self.free_pages
    }

    /// Number of used pages
    pub fn used_count(&self) -> usize {
        self.total_pages - self.free_pages
    }

    /// Total managed pages
    pub fn total_count(&self) -> usize {
        self.total_pages
    }

    /// Get watermarks
    pub fn watermarks(&self) -> Watermarks {
        self.watermarks
    }

    /// Set custom watermarks
    pub fn set_watermarks(&mut self, min: usize, low: usize, high: usize) {
        self.watermarks = Watermarks { min, low, high };
    }

    /// Check if compaction is needed
    pub fn needs_compaction(&self) -> bool {
        self.compact_needed
    }

    /// Get per-order statistics
    pub fn order_stats(&self) -> [OrderStats; MAX_ORDER] {
        self.order_stats
    }

    /// Get emergency reserve info
    pub fn emergency_reserve_info(&self) -> (usize, usize) {
        (self.emergency_free, self.emergency_max)
    }

    /// Compute fragmentation score (0..1000, where 1000 = severely fragmented)
    /// Measures the ratio of memory trapped in low-order blocks vs. total free.
    pub fn fragmentation_score(&self) -> usize {
        if self.free_pages == 0 {
            return 0;
        }

        // Count pages available at each order
        let mut high_order_pages = 0usize;
        for order in 4..MAX_ORDER {
            high_order_pages += self.free_lists[order].count * (1 << order);
        }

        // Fragmentation = how much free memory is trapped in low orders
        // Score: (total_free - high_order_pages) * 1000 / total_free
        if high_order_pages >= self.free_pages {
            0
        } else {
            ((self.free_pages - high_order_pages) * 1000) / self.free_pages
        }
    }

    /// Attempt compaction: try to move movable pages to consolidate free blocks.
    /// Returns the number of pages moved.
    pub fn compact(&mut self) -> usize {
        self.stats.compaction_runs = self.stats.compaction_runs.saturating_add(1);
        let mut moved = 0usize;

        // Simple compaction: for each low-order free block, check if a movable
        // page exists at its buddy location. If so, we could move it.
        // This is a simplified version — real compaction is much more complex.

        // Walk all pages from the end backward, looking for movable pages
        // that can be relocated to lower addresses to free up contiguous blocks.
        let mut target_page = 0usize;
        let mut source_page = self.total_pages;

        while target_page < source_page {
            // Find the next free page from the start
            while target_page < self.total_pages && self.pages[target_page].flags != PageFlags::Free
            {
                target_page += 1;
            }
            if target_page >= source_page {
                break;
            }

            // Find the next movable page from the end
            source_page = source_page.saturating_sub(1);
            while source_page > target_page && self.pages[source_page].flags != PageFlags::Movable {
                if source_page == 0 {
                    break;
                }
                source_page -= 1;
            }
            if source_page <= target_page {
                break;
            }

            // We could move source_page to target_page here.
            // In practice this requires updating page tables, which we can't do
            // without knowing which address spaces reference this page.
            // For now, just count what we could move.
            moved += 1;
            target_page += 1;
        }

        self.compact_needed = self.fragmentation_score() > FRAGMENTATION_COMPACT_THRESHOLD;
        moved
    }

    /// Get free list counts for each order (for /proc/buddyinfo)
    pub fn free_list_counts(&self) -> [usize; MAX_ORDER] {
        let mut counts = [0usize; MAX_ORDER];
        for i in 0..MAX_ORDER {
            counts[i] = self.free_lists[i].count;
        }
        counts
    }

    /// Memory zone info string (like /proc/buddyinfo)
    pub fn buddyinfo(&self) -> alloc::string::String {
        use alloc::format;
        let counts = self.free_list_counts();
        let mut s = alloc::string::String::from("Node 0, zone   Normal ");
        for i in 0..MAX_ORDER {
            s.push_str(&format!("{:6} ", counts[i]));
        }
        s
    }

    /// Detailed statistics string
    pub fn detailed_stats(&self) -> alloc::string::String {
        use alloc::format;
        let mut s = alloc::string::String::new();
        s.push_str(&format!("Buddy Allocator Statistics:\n"));
        s.push_str(&format!("  Total pages: {}\n", self.total_pages));
        s.push_str(&format!(
            "  Free pages:  {} ({} KB)\n",
            self.free_pages,
            self.free_pages * PAGE_SIZE / 1024
        ));
        s.push_str(&format!(
            "  Used pages:  {}\n",
            self.total_pages - self.free_pages
        ));
        s.push_str(&format!(
            "  Watermarks:  min={} low={} high={}\n",
            self.watermarks.min, self.watermarks.low, self.watermarks.high
        ));
        s.push_str(&format!(
            "  Emergency reserve: {}/{}\n",
            self.emergency_free, self.emergency_max
        ));
        s.push_str(&format!(
            "  Fragmentation: {}/1000\n",
            self.fragmentation_score()
        ));
        s.push_str(&format!(
            "  Allocs: {}  Frees: {}  Splits: {}  Merges: {}\n",
            self.stats.alloc_count,
            self.stats.free_count,
            self.stats.split_count,
            self.stats.merge_count
        ));
        s.push_str(&format!(
            "  Failures: {}  Emergency allocs: {}\n",
            self.stats.alloc_failures, self.stats.emergency_allocs
        ));
        s.push_str(&format!("  Per-order free blocks:\n"));
        for i in 0..MAX_ORDER {
            let block_size = (1 << i) * PAGE_SIZE;
            let block_size_str = if block_size >= 1024 * 1024 {
                format!("{}MB", block_size / (1024 * 1024))
            } else if block_size >= 1024 {
                format!("{}KB", block_size / 1024)
            } else {
                format!("{}B", block_size)
            };
            s.push_str(&format!(
                "    order {:2} ({:>6}): {:5} free, {:8} allocs, {:8} frees\n",
                i,
                block_size_str,
                self.free_lists[i].count,
                self.order_stats[i].alloc_count,
                self.order_stats[i].free_count
            ));
        }
        s
    }
}

/// Global buddy allocator
pub static BUDDY: Mutex<BuddyAllocator> = Mutex::new(BuddyAllocator::new());

/// Statistics counters (lockless)
pub static ALLOC_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static FREE_TOTAL: AtomicU64 = AtomicU64::new(0);

/// Initialize the buddy allocator
pub fn init(start_addr: usize, end_addr: usize) {
    BUDDY.lock().init(start_addr, end_addr);
}

/// Allocate pages of a given order
pub fn alloc_pages(order: usize) -> Option<usize> {
    let result = BUDDY.lock().alloc_pages(order);
    if result.is_some() {
        ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Allocate pages with explicit flags
pub fn alloc_pages_flags(order: usize, flags: AllocFlags) -> Option<usize> {
    let result = BUDDY.lock().alloc_pages_flags(order, flags);
    if result.is_some() {
        ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Free pages of a given order
pub fn free_pages(addr: usize, order: usize) {
    BUDDY.lock().free_pages(addr, order);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Allocate a single page
pub fn alloc_page() -> Option<usize> {
    alloc_pages(0)
}

/// Free a single page
pub fn free_page(addr: usize) {
    free_pages(addr, 0);
}

/// Allocate the smallest buddy block containing at least `n` contiguous pages.
///
/// Returns the physical address of the first page, or `None` if unavailable.
/// The allocated block is always a power-of-2 number of pages; any surplus
/// pages remain allocated and must be freed by the caller if not needed.
pub fn alloc_contiguous(n: usize) -> Option<usize> {
    let result = BUDDY.lock().alloc_contiguous(n);
    if result.is_some() {
        ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Check if compaction is needed
pub fn needs_compaction() -> bool {
    BUDDY.lock().needs_compaction()
}

/// Run compaction
pub fn compact() -> usize {
    BUDDY.lock().compact()
}

/// Get fragmentation score
pub fn fragmentation_score() -> usize {
    BUDDY.lock().fragmentation_score()
}

// ---------------------------------------------------------------------------
// Huge page convenience API (2MB = order-9 allocation)
// ---------------------------------------------------------------------------

/// Order for a 2MB huge page (2^9 = 512 × 4KB pages)
pub const HUGE_PAGE_ORDER: usize = 9;

/// Size of a 2MB huge page in bytes
pub const HUGE_PAGE_SIZE: usize = 512 * PAGE_SIZE; // 2MB

/// Allocate a single 2MB huge page from the buddy allocator.
/// Returns the physical address (2MB-aligned) or None on failure.
pub fn alloc_huge_page() -> Option<usize> {
    let result = BUDDY.lock().alloc_pages(HUGE_PAGE_ORDER);
    if result.is_some() {
        ALLOC_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Free a 2MB huge page back to the buddy allocator.
/// `phys_addr` must be the address originally returned by `alloc_huge_page`.
pub fn free_huge_page(phys_addr: usize) {
    BUDDY.lock().free_pages(phys_addr, HUGE_PAGE_ORDER);
    FREE_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Total managed memory in bytes
pub fn total_bytes() -> usize {
    BUDDY.lock().total_count().saturating_mul(PAGE_SIZE)
}

/// Free memory in bytes
pub fn free_bytes() -> usize {
    BUDDY.lock().free_count().saturating_mul(PAGE_SIZE)
}

/// Count of free huge pages (order-9 blocks available directly or after split)
/// Returns the number of free blocks at order >= HUGE_PAGE_ORDER.
pub fn free_huge_page_count() -> usize {
    let buddy = BUDDY.lock();
    let counts = buddy.free_list_counts();
    let mut total = 0usize;
    for order in HUGE_PAGE_ORDER..MAX_ORDER {
        // Each order-N block can yield 2^(N-HUGE_PAGE_ORDER) huge pages
        let factor = 1usize << (order.saturating_sub(HUGE_PAGE_ORDER));
        total = total.saturating_add(counts[order].saturating_mul(factor));
    }
    total
}
