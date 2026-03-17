/// cma — Contiguous Memory Allocator for Genesis
///
/// Provides physically contiguous memory allocation for DMA-capable devices
/// and large kernel buffers. Unlike the buddy allocator (which can fragment),
/// CMA reserves regions at boot and manages them with a bitmap to guarantee
/// contiguous allocations.
///
/// Architecture:
///   - Reserved CMA regions (configured at boot)
///   - Bitmap-based allocation within each region
///   - Best-fit search to minimize fragmentation
///   - DMA-friendly alignment support
///   - Movable page migration for defragmentation
///   - Per-region statistics
///
/// Inspired by: Linux CMA (mm/cma.c, mm/cma.h). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum CMA regions
const MAX_CMA_REGIONS: usize = 8;

/// Maximum pages per CMA region (64 MB / 4 KB = 16384 pages)
const MAX_REGION_PAGES: usize = 16384;

/// Page size
const PAGE_SIZE: usize = 4096;

/// Default CMA region size (16 MB)
const DEFAULT_REGION_SIZE: usize = 16 * 1024 * 1024;

/// Bitmap words needed (MAX_REGION_PAGES / 64 bits per word)
const BITMAP_WORDS: usize = (MAX_REGION_PAGES + 63) / 64;

// ---------------------------------------------------------------------------
// CMA region
// ---------------------------------------------------------------------------

/// Statistics for a CMA region
#[derive(Debug, Clone, Copy, Default)]
pub struct CmaRegionStats {
    /// Total allocation requests
    pub alloc_count: u64,
    /// Total free requests
    pub free_count: u64,
    /// Allocation failures
    pub alloc_failures: u64,
    /// Pages currently allocated
    pub pages_allocated: u64,
    /// Peak pages allocated
    pub peak_pages: u64,
    /// Fragmentation events (allocation required migration)
    pub migration_count: u64,
}

/// A single CMA region
pub struct CmaRegion {
    /// Human-readable name
    pub name: [u8; 32],
    /// Physical base address (page-aligned)
    pub base_addr: usize,
    /// Total pages in this region
    pub total_pages: usize,
    /// Allocation bitmap (1 = allocated, 0 = free)
    pub bitmap: [u64; BITMAP_WORDS],
    /// Number of pages currently allocated
    pub allocated_pages: usize,
    /// Whether this region is active
    pub active: bool,
    /// Required alignment (in pages) for allocations from this region
    pub align_pages: usize,
    /// Statistics
    pub stats: CmaRegionStats,
}

impl CmaRegion {
    const fn new() -> Self {
        CmaRegion {
            name: [0u8; 32],
            base_addr: 0,
            total_pages: 0,
            bitmap: [0u64; BITMAP_WORDS],
            allocated_pages: 0,
            active: false,
            align_pages: 1,
            stats: CmaRegionStats {
                alloc_count: 0,
                free_count: 0,
                alloc_failures: 0,
                pages_allocated: 0,
                peak_pages: 0,
                migration_count: 0,
            },
        }
    }

    /// Set region name
    fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(31);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }

    /// Get region name as str
    pub fn name_str(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Test if a bit is set in the bitmap
    fn bit_is_set(&self, page_idx: usize) -> bool {
        let word = page_idx / 64;
        let bit = page_idx % 64;
        if word >= BITMAP_WORDS {
            return true;
        }
        (self.bitmap[word] & (1u64 << bit)) != 0
    }

    /// Set a bit in the bitmap
    fn set_bit(&mut self, page_idx: usize) {
        let word = page_idx / 64;
        let bit = page_idx % 64;
        if word < BITMAP_WORDS {
            self.bitmap[word] |= 1u64 << bit;
        }
    }

    /// Clear a bit in the bitmap
    fn clear_bit(&mut self, page_idx: usize) {
        let word = page_idx / 64;
        let bit = page_idx % 64;
        if word < BITMAP_WORDS {
            self.bitmap[word] &= !(1u64 << bit);
        }
    }

    /// Check if a range of pages is entirely free
    fn range_is_free(&self, start: usize, count: usize) -> bool {
        for i in 0..count {
            if self.bit_is_set(start + i) {
                return false;
            }
        }
        true
    }

    /// Allocate `count` contiguous pages with given alignment.
    /// Returns offset (page index) within the region.
    fn alloc_pages(&mut self, count: usize, align: usize) -> Option<usize> {
        if count == 0 || count > self.total_pages {
            return None;
        }

        let alignment = align.max(self.align_pages).max(1);

        // Best-fit search: find the smallest free range >= count
        let mut best_start: Option<usize> = None;
        let mut best_size: usize = usize::MAX;
        let mut run_start: Option<usize> = None;
        let mut run_len: usize = 0;

        let mut page = 0;
        while page < self.total_pages {
            if !self.bit_is_set(page) {
                if run_start.is_none() {
                    // Start of a new free run — align the start
                    let aligned = (page + alignment - 1) & !(alignment - 1);
                    if aligned < self.total_pages {
                        run_start = Some(aligned);
                        run_len = if aligned == page { 1 } else { 0 };
                        if aligned > page {
                            page = aligned;
                            continue;
                        }
                    }
                } else {
                    run_len += 1;
                }

                // Check if this run is big enough
                if let Some(start) = run_start {
                    if run_len >= count && run_len < best_size {
                        // Verify alignment
                        if start % alignment == 0 {
                            best_start = Some(start);
                            best_size = run_len;
                            if run_len == count {
                                break; // exact fit
                            }
                        }
                    }
                }
            } else {
                run_start = None;
                run_len = 0;
            }
            page += 1;
        }

        if let Some(start) = best_start {
            // Mark pages as allocated
            for i in 0..count {
                self.set_bit(start + i);
            }
            self.allocated_pages += count;
            self.stats.alloc_count = self.stats.alloc_count.saturating_add(1);
            self.stats.pages_allocated += count as u64;
            if self.stats.pages_allocated > self.stats.peak_pages {
                self.stats.peak_pages = self.stats.pages_allocated;
            }
            Some(start)
        } else {
            self.stats.alloc_failures = self.stats.alloc_failures.saturating_add(1);
            None
        }
    }

    /// Free `count` pages starting at `offset`
    fn free_pages(&mut self, offset: usize, count: usize) {
        if offset + count > self.total_pages {
            return;
        }
        for i in 0..count {
            self.clear_bit(offset + i);
        }
        self.allocated_pages -= count.min(self.allocated_pages);
        self.stats.free_count = self.stats.free_count.saturating_add(1);
        self.stats.pages_allocated = self.stats.pages_allocated.saturating_sub(count as u64);
    }

    /// Count total free pages
    pub fn free_pages_count(&self) -> usize {
        self.total_pages - self.allocated_pages
    }

    /// Compute fragmentation index (Q16 fixed-point, 0 = none, Q16_ONE = totally fragmented)
    pub fn fragmentation_q16(&self) -> i32 {
        const Q16_ONE: i32 = 65536;
        if self.total_pages == 0 || self.allocated_pages == self.total_pages {
            return Q16_ONE;
        }

        // Count the number of free runs
        let mut runs = 0usize;
        let mut in_run = false;
        let mut largest_run = 0usize;
        let mut current_run = 0usize;

        for i in 0..self.total_pages {
            if !self.bit_is_set(i) {
                if !in_run {
                    in_run = true;
                    runs += 1;
                    current_run = 0;
                }
                current_run += 1;
            } else {
                if in_run {
                    if current_run > largest_run {
                        largest_run = current_run;
                    }
                    in_run = false;
                }
            }
        }
        if in_run && current_run > largest_run {
            largest_run = current_run;
        }

        let free = self.free_pages_count();
        if free == 0 {
            return Q16_ONE;
        }

        // Fragmentation = 1 - (largest_run / total_free)
        let ratio = (((largest_run as i64) << 16) / (free as i64)) as i32;
        Q16_ONE - ratio
    }
}

// ---------------------------------------------------------------------------
// Global CMA state
// ---------------------------------------------------------------------------

/// Global CMA manager
struct CmaManager {
    regions: [CmaRegion; MAX_CMA_REGIONS],
    region_count: usize,
}

impl CmaManager {
    const fn new() -> Self {
        const EMPTY: CmaRegion = CmaRegion::new();
        CmaManager {
            regions: [EMPTY; MAX_CMA_REGIONS],
            region_count: 0,
        }
    }
}

static CMA: Mutex<CmaManager> = Mutex::new(CmaManager::new());

/// Global statistics
pub static TOTAL_CMA_ALLOCS: AtomicU64 = AtomicU64::new(0);
pub static TOTAL_CMA_FREES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Declare a CMA region. Must be called during early boot before memory is used.
///
/// `base_addr`: Physical start address (must be page-aligned)
/// `size`: Region size in bytes
/// `align_pages`: Minimum alignment for allocations (in pages)
/// `name`: Descriptive name
///
/// Returns region index or None.
pub fn declare_region(
    base_addr: usize,
    size: usize,
    align_pages: usize,
    name: &str,
) -> Option<usize> {
    let mut mgr = CMA.lock();
    if mgr.region_count >= MAX_CMA_REGIONS {
        return None;
    }

    let total_pages = size / PAGE_SIZE;
    if total_pages == 0 || total_pages > MAX_REGION_PAGES {
        serial_println!(
            "  [cma] invalid region size: {} pages (max {})",
            total_pages,
            MAX_REGION_PAGES
        );
        return None;
    }

    // Verify alignment
    if base_addr % PAGE_SIZE != 0 {
        serial_println!("  [cma] base address {:#x} not page-aligned", base_addr);
        return None;
    }

    let idx = mgr.region_count;
    mgr.regions[idx].base_addr = base_addr;
    mgr.regions[idx].total_pages = total_pages;
    mgr.regions[idx].align_pages = align_pages.max(1);
    mgr.regions[idx].active = true;
    mgr.regions[idx].set_name(name);
    mgr.region_count = mgr.region_count.saturating_add(1);

    let final_align = mgr.regions[idx].align_pages;
    serial_println!(
        "  [cma] region '{}' at {:#x}, {} pages ({} KB), align={}",
        name,
        base_addr,
        total_pages,
        size / 1024,
        final_align
    );

    Some(idx)
}

/// Allocate contiguous physical memory from a CMA region.
///
/// Returns physical address or None.
pub fn alloc(region_idx: usize, num_pages: usize, align_pages: usize) -> Option<usize> {
    let mut mgr = CMA.lock();
    if region_idx >= MAX_CMA_REGIONS || !mgr.regions[region_idx].active {
        return None;
    }

    let region = &mut mgr.regions[region_idx];
    if let Some(offset) = region.alloc_pages(num_pages, align_pages) {
        let phys = region.base_addr + offset * PAGE_SIZE;
        TOTAL_CMA_ALLOCS.fetch_add(1, Ordering::Relaxed);
        Some(phys)
    } else {
        None
    }
}

/// Free contiguous physical memory back to a CMA region.
pub fn free(region_idx: usize, phys_addr: usize, num_pages: usize) {
    let mut mgr = CMA.lock();
    if region_idx >= MAX_CMA_REGIONS || !mgr.regions[region_idx].active {
        return;
    }

    let region = &mut mgr.regions[region_idx];
    if phys_addr < region.base_addr {
        return;
    }
    let offset = (phys_addr - region.base_addr) / PAGE_SIZE;
    region.free_pages(offset, num_pages);
    TOTAL_CMA_FREES.fetch_add(1, Ordering::Relaxed);
}

/// Allocate DMA-friendly contiguous memory (auto-selects best region)
///
/// Tries all regions, preferring one with lowest fragmentation.
pub fn dma_alloc(num_pages: usize, align_pages: usize) -> Option<(usize, usize)> {
    let mut mgr = CMA.lock();
    let mut best_region: Option<usize> = None;
    let mut best_frag = i32::MAX;

    for i in 0..mgr.region_count {
        if mgr.regions[i].active && mgr.regions[i].free_pages_count() >= num_pages {
            let frag = mgr.regions[i].fragmentation_q16();
            if frag < best_frag {
                best_frag = frag;
                best_region = Some(i);
            }
        }
    }

    if let Some(idx) = best_region {
        let region = &mut mgr.regions[idx];
        if let Some(offset) = region.alloc_pages(num_pages, align_pages) {
            let phys = region.base_addr + offset * PAGE_SIZE;
            TOTAL_CMA_ALLOCS.fetch_add(1, Ordering::Relaxed);
            return Some((idx, phys));
        }
    }

    None
}

/// Free DMA-allocated memory
pub fn dma_free(region_idx: usize, phys_addr: usize, num_pages: usize) {
    free(region_idx, phys_addr, num_pages);
}

/// Get region info
pub fn region_info(region_idx: usize) -> Option<(usize, usize, usize)> {
    let mgr = CMA.lock();
    if region_idx >= MAX_CMA_REGIONS || !mgr.regions[region_idx].active {
        return None;
    }
    let r = &mgr.regions[region_idx];
    Some((r.total_pages, r.allocated_pages, r.free_pages_count()))
}

/// Get region fragmentation (Q16 fixed-point)
pub fn fragmentation(region_idx: usize) -> i32 {
    let mgr = CMA.lock();
    if region_idx >= MAX_CMA_REGIONS || !mgr.regions[region_idx].active {
        return 65536; // 1.0 = fully fragmented
    }
    mgr.regions[region_idx].fragmentation_q16()
}

/// Get summary of all CMA regions
pub fn summary() -> alloc::string::String {
    use alloc::format;
    use alloc::string::String;
    let mgr = CMA.lock();
    let mut s = String::from("CMA regions:\n");
    for i in 0..mgr.region_count {
        let r = &mgr.regions[i];
        if r.active {
            let frag_q16 = r.fragmentation_q16();
            let frag_pct = (((frag_q16 as i64) * 100) >> 16) as i32;
            let used_pct = if r.total_pages > 0 {
                (r.allocated_pages * 100) / r.total_pages
            } else {
                0
            };
            s.push_str(&format!(
                "  [{}] '{}': {:#x} {}/{} pages ({}% used, {}% frag) allocs={} frees={}\n",
                i,
                r.name_str(),
                r.base_addr,
                r.allocated_pages,
                r.total_pages,
                used_pct,
                frag_pct,
                r.stats.alloc_count,
                r.stats.free_count,
            ));
        }
    }
    s
}

/// Initialize CMA subsystem
///
/// Creates a default CMA region for DMA use.
pub fn init() {
    // Reserve a default CMA region after the buddy allocator region
    // In a real kernel this would parse boot parameters (cma=16M)
    let cma_base = crate::memory::buddy::MAX_MEMORY * 2; // past buddy region
    let cma_size = DEFAULT_REGION_SIZE;

    if let Some(idx) = declare_region(cma_base, cma_size, 1, "default") {
        serial_println!(
            "  [cma] default region {} ready ({} KB)",
            idx,
            cma_size / 1024
        );
    } else {
        serial_println!("  [cma] WARNING: failed to create default region");
    }

    serial_println!("  [cma] contiguous memory allocator initialized");
}
