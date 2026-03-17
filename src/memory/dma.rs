use crate::sync::Mutex;

/// Maximum number of entries in the DMA free list.
const DMA_FREE_LIST_SIZE: usize = 64;

/// Free list for returning non-tail DMA allocations.
///
/// Each entry is `Some((phys_addr, size))` for a freed region.
/// On the next `dma_alloc()` call the free list is checked first;
/// if a region is large enough (and alignment is satisfied) it is
/// reused rather than bump-allocating new memory.
static DMA_FREE_LIST: Mutex<[Option<(u64, usize)>; DMA_FREE_LIST_SIZE]> =
    Mutex::new([None; DMA_FREE_LIST_SIZE]);

/// A DMA-capable memory region descriptor.
pub struct DmaRegion {
    /// Virtual address of the allocation.
    pub virt_addr: usize,
    /// Physical (bus) address for device programming.
    pub phys_addr: usize,
    /// Size in bytes.
    pub size: usize,
}

/// DMA memory allocator managing a low-memory pool.
pub struct DmaAllocator {
    pub pool_start: usize,
    pub pool_size: usize,
    pub next_offset: usize,
}

impl DmaAllocator {
    pub fn new(pool_start: usize, pool_size: usize) -> Self {
        DmaAllocator {
            pool_start,
            pool_size,
            next_offset: 0,
        }
    }

    /// Allocate contiguous DMA-safe memory aligned to `align` bytes.
    ///
    /// Strategy:
    ///  1. Check the global `DMA_FREE_LIST` for a previously freed region that
    ///     satisfies the size and alignment requirements.  If found, reuse it.
    ///  2. Fall back to bump-allocating from the pool.
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<DmaRegion, &'static str> {
        if size == 0 {
            return Err("dma: alloc size must be > 0");
        }
        let align = if align == 0 { 1 } else { align };

        // --- Step 1: check the free list ---
        {
            let mut free_list = DMA_FREE_LIST.lock();
            for slot in free_list.iter_mut() {
                if let Some((phys, region_size)) = *slot {
                    let phys_usize = phys as usize;
                    // Check alignment: phys_usize must be aligned to `align`.
                    let aligned_phys = (phys_usize.saturating_add(align - 1)) & !(align - 1);
                    let padding = aligned_phys.saturating_sub(phys_usize);
                    let usable = region_size.saturating_sub(padding);
                    if usable >= size {
                        // Consume this slot.
                        *slot = None;
                        return Ok(DmaRegion {
                            virt_addr: aligned_phys, // identity-mapped
                            phys_addr: aligned_phys,
                            size,
                        });
                    }
                }
            }
        }

        // --- Step 2: bump allocate ---
        let aligned_offset = (self.next_offset.saturating_add(align - 1)) & !(align - 1);
        let end = aligned_offset.saturating_add(size);
        if end > self.pool_size {
            return Err("dma: pool exhausted");
        }
        self.next_offset = end;
        let virt = self.pool_start.saturating_add(aligned_offset);
        Ok(DmaRegion {
            virt_addr: virt,
            phys_addr: virt, // Identity-mapped in early kernel; adjust when IOMMU present.
            size,
        })
    }

    /// Return a DMA region to the pool.
    ///
    /// If this is the tail allocation, the bump pointer is retracted (fast
    /// path).  Otherwise the region is pushed onto `DMA_FREE_LIST` so that a
    /// future `alloc()` can reuse it.  If the free list is full the region is
    /// silently leaked — a production driver should size the list appropriately
    /// or use a buddy sub-allocator per size class.
    pub fn free(&mut self, region: DmaRegion) {
        let region_offset = region.virt_addr.saturating_sub(self.pool_start);
        let region_end = region_offset.saturating_add(region.size);
        if region_end == self.next_offset {
            // Last allocation — safe to reclaim by retracting the bump pointer.
            self.next_offset = region_offset;
        } else {
            // Non-tail free: push to free list for reuse.
            dma_free(region.phys_addr as u64, region.size);
        }
    }
}

/// Push a freed DMA region onto the global free list.
///
/// If the free list is full the region is silently leaked.  Callers that need
/// guaranteed reclaim should reduce concurrent allocation pressure or increase
/// `DMA_FREE_LIST_SIZE`.
pub fn dma_free(phys: u64, size: usize) {
    let mut free_list = DMA_FREE_LIST.lock();
    for slot in free_list.iter_mut() {
        if slot.is_none() {
            *slot = Some((phys, size));
            return;
        }
    }
    // Free list full — log and leak.
    crate::serial_println!(
        "dma: free-list full, leaking region phys=0x{:x} size={}",
        phys,
        size
    );
}

/// Allocate a DMA region using the global free list, then the global bump pool.
///
/// This is a convenience wrapper for callers that do not hold a `DmaAllocator`
/// instance directly.  It checks the free list first (size ≥ requested, with
/// alignment), falling back to returning `0` on failure.
pub fn dma_alloc(size: usize, align: usize) -> u64 {
    if size == 0 {
        return 0;
    }
    let align = if align == 0 { 1 } else { align };
    let mut free_list = DMA_FREE_LIST.lock();
    for slot in free_list.iter_mut() {
        if let Some((phys, region_size)) = *slot {
            let phys_usize = phys as usize;
            let aligned_phys = (phys_usize.saturating_add(align - 1)) & !(align - 1);
            let padding = aligned_phys.saturating_sub(phys_usize);
            let usable = region_size.saturating_sub(padding);
            if usable >= size {
                *slot = None;
                return aligned_phys as u64;
            }
        }
    }
    // No suitable free-list entry found; caller must use a DmaAllocator for
    // bump allocation.
    0
}

/// Initialize the DMA allocator.
pub fn init() {
    // Free list is initialized via const fn; no runtime work needed.
    // Reserve low-memory pool for DMA: caller should construct a DmaAllocator
    // with the desired pool_start / pool_size from the memory map.
}
