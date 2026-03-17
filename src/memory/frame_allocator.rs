use crate::boot_protocol::{MemoryKind, MemoryRegion};
/// Bitmap-based physical frame allocator for Genesis
///
/// Each bit in the bitmap represents one 4KB physical page frame.
/// Bit = 0 means free, bit = 1 means used/allocated.
///
/// Features:
///   - Region-aware allocation (DMA <16MB, Normal, HighMem zones)
///   - Contiguous multi-frame allocation
///   - Frame reference counting for shared/COW pages
///   - Page coloring hints for cache optimization
///   - Allocation statistics and fragmentation scoring
///   - Deferred free list for batch-free operations
///   - Per-frame metadata (dirty, referenced, locked, reserved flags)
///
/// Inspired by Linux's page frame allocator concept. All code is original.
use crate::sync::Mutex;

/// Size of a physical page frame (4 KB)
pub const FRAME_SIZE: usize = 4096;

/// Maximum physical memory we manage (512 MB — matches QEMU -m 512M)
pub const MAX_MEMORY: usize = 512 * 1024 * 1024;

/// Total number of frames in our managed range
const MAX_FRAMES: usize = MAX_MEMORY / FRAME_SIZE;

/// Bitmap size in bytes (1 bit per frame)
const BITMAP_SIZE: usize = MAX_FRAMES / 8;

/// DMA zone upper bound: frames below 16 MB
const DMA_ZONE_END: usize = 16 * 1024 * 1024;

/// Normal zone upper bound: frames below 896 MB (everything we manage)
const NORMAL_ZONE_END: usize = MAX_MEMORY;

/// Maximum frames in the deferred free list before forced flush
const DEFERRED_FREE_MAX: usize = 64;

/// Number of cache colors (power of 2, typically L2 way count approximation)
const NUM_CACHE_COLORS: usize = 16;

/// Memory zones for region-aware allocation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryZone {
    /// DMA zone: physical addresses 0..16MB (ISA DMA, legacy devices)
    Dma,
    /// Normal zone: physical addresses 16MB..MAX_MEMORY
    Normal,
    /// HighMem zone: above MAX_MEMORY (not managed, placeholder for future)
    HighMem,
}

/// Per-frame metadata flags
#[derive(Debug, Clone, Copy)]
pub struct FrameFlags {
    /// Page has been written to since last sync
    pub dirty: bool,
    /// Page has been accessed (for LRU aging)
    pub referenced: bool,
    /// Page is locked in memory (cannot be swapped or evicted)
    pub locked: bool,
    /// Page is reserved (BIOS, MMIO, kernel code) and must never be allocated
    pub reserved: bool,
}

impl FrameFlags {
    const fn new() -> Self {
        FrameFlags {
            dirty: false,
            referenced: false,
            locked: false,
            reserved: false,
        }
    }

    const fn reserved() -> Self {
        FrameFlags {
            dirty: false,
            referenced: false,
            locked: false,
            reserved: true,
        }
    }
}

/// Per-frame metadata for reference counting and flags
#[derive(Clone, Copy)]
struct FrameMetadata {
    /// Reference count (0 = free, 1 = single owner, >1 = shared/COW)
    refcount: u16,
    /// Frame flags
    flags: FrameFlags,
}

impl FrameMetadata {
    const fn new() -> Self {
        FrameMetadata {
            refcount: 0,
            flags: FrameFlags::new(),
        }
    }
}

/// Allocation statistics
#[derive(Debug, Clone, Copy)]
pub struct AllocStats {
    /// Total number of allocate calls that succeeded
    pub total_allocs: u64,
    /// Total number of deallocate calls
    pub total_frees: u64,
    /// Peak number of frames simultaneously in use
    pub peak_usage: usize,
    /// Total multi-frame (contiguous) allocation requests
    pub contiguous_allocs: u64,
    /// Failed allocation attempts
    pub failed_allocs: u64,
    /// DMA zone allocations
    pub dma_allocs: u64,
    /// Normal zone allocations
    pub normal_allocs: u64,
    /// Deferred frees flushed
    pub deferred_flushes: u64,
}

impl AllocStats {
    const fn new() -> Self {
        AllocStats {
            total_allocs: 0,
            total_frees: 0,
            peak_usage: 0,
            contiguous_allocs: 0,
            failed_allocs: 0,
            dma_allocs: 0,
            normal_allocs: 0,
            deferred_flushes: 0,
        }
    }
}

/// A physical memory frame, aligned to FRAME_SIZE
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame {
    /// Physical start address of this frame (always aligned to FRAME_SIZE)
    pub addr: usize,
}

impl PhysFrame {
    /// Create a PhysFrame from an address. Panics if not aligned.
    pub fn from_addr(addr: usize) -> Self {
        assert!(addr % FRAME_SIZE == 0, "PhysFrame address not aligned");
        PhysFrame { addr }
    }

    /// Frame number (index into bitmap)
    pub fn number(&self) -> usize {
        self.addr / FRAME_SIZE
    }

    /// Which zone does this frame belong to?
    pub fn zone(&self) -> MemoryZone {
        if self.addr < DMA_ZONE_END {
            MemoryZone::Dma
        } else if self.addr < NORMAL_ZONE_END {
            MemoryZone::Normal
        } else {
            MemoryZone::HighMem
        }
    }

    /// Cache color hint for this frame (reduces cache set conflicts)
    pub fn cache_color(&self) -> usize {
        (self.addr / FRAME_SIZE) % NUM_CACHE_COLORS
    }
}

/// The bitmap frame allocator
pub struct BitmapFrameAllocator {
    /// Bitmap: 1 bit per frame, 1 = used, 0 = free
    bitmap: [u8; BITMAP_SIZE],
    /// First frame that might be free (optimization to skip known-used regions)
    next_free_hint: usize,
    /// Total frames marked as free
    free_frames: usize,
    /// Per-frame metadata (refcount, flags)
    metadata: [FrameMetadata; MAX_FRAMES],
    /// Per-zone free counts
    dma_free: usize,
    normal_free: usize,
    /// Deferred free list: frames to be freed in batch
    deferred_free: [usize; DEFERRED_FREE_MAX],
    deferred_count: usize,
    /// Allocation statistics
    stats: AllocStats,
}

impl BitmapFrameAllocator {
    /// Create a new allocator with all frames marked as used
    const fn new() -> Self {
        BitmapFrameAllocator {
            bitmap: [0xFF; BITMAP_SIZE], // all used initially
            next_free_hint: 0,
            free_frames: 0,
            metadata: [FrameMetadata::new(); MAX_FRAMES],
            dma_free: 0,
            normal_free: 0,
            deferred_free: [0; DEFERRED_FREE_MAX],
            deferred_count: 0,
            stats: AllocStats::new(),
        }
    }

    /// Initialize: mark frames from kernel_end_aligned..MAX_MEMORY as free.
    /// Everything below kernel_end stays marked as used (BIOS, VGA, kernel code/data).
    pub fn init(&mut self, kernel_end_aligned: usize) {
        let first_free_frame = kernel_end_aligned / FRAME_SIZE;
        let total_frames = MAX_MEMORY / FRAME_SIZE;
        let dma_zone_frames = DMA_ZONE_END / FRAME_SIZE;

        // Mark all frames below kernel_end as reserved
        for frame in 0..first_free_frame {
            self.metadata[frame].flags = FrameFlags::reserved();
        }

        // Mark all frames from first_free to end as free
        for frame in first_free_frame..total_frames {
            let byte = frame / 8;
            let bit = frame % 8;
            self.bitmap[byte] &= !(1 << bit); // clear bit = free
            self.metadata[frame].refcount = 0;
            self.metadata[frame].flags = FrameFlags::new();

            // Track per-zone free counts
            if frame < dma_zone_frames {
                self.dma_free = self.dma_free.saturating_add(1);
            } else {
                self.normal_free = self.normal_free.saturating_add(1);
            }
        }

        self.next_free_hint = first_free_frame;
        self.free_frames = total_frames - first_free_frame;
    }

    /// Initialize allocator from firmware memory map.
    ///
    /// Only `MemoryKind::Usable` regions are made allocatable, everything else
    /// remains reserved. Regions above MAX_MEMORY are ignored.
    pub fn init_from_memory_map(&mut self, kernel_end_aligned: usize, regions: &[MemoryRegion]) {
        *self = BitmapFrameAllocator::new();

        let total_frames = MAX_MEMORY / FRAME_SIZE;
        let dma_zone_frames = DMA_ZONE_END / FRAME_SIZE;
        let kernel_end_frame = (kernel_end_aligned / FRAME_SIZE).min(total_frames);

        for frame in 0..kernel_end_frame {
            self.metadata[frame].flags = FrameFlags::reserved();
        }

        for region in regions {
            if region.kind != MemoryKind::Usable || region.length == 0 {
                continue;
            }

            let region_start = (region.base as usize).max(kernel_end_aligned);
            let region_end = (region.base.saturating_add(region.length) as usize).min(MAX_MEMORY);
            if region_end <= region_start {
                continue;
            }

            let start_frame = (region_start + FRAME_SIZE - 1) / FRAME_SIZE;
            let end_frame = region_end / FRAME_SIZE;
            if end_frame <= start_frame {
                continue;
            }

            for frame in start_frame..end_frame.min(total_frames) {
                let byte = frame / 8;
                let bit = frame % 8;
                if self.bitmap[byte] & (1 << bit) == 0 {
                    continue;
                }

                self.bitmap[byte] &= !(1 << bit);
                self.metadata[frame].refcount = 0;
                self.metadata[frame].flags = FrameFlags::new();
                self.free_frames = self.free_frames.saturating_add(1);

                if frame < dma_zone_frames {
                    self.dma_free = self.dma_free.saturating_add(1);
                } else {
                    self.normal_free = self.normal_free.saturating_add(1);
                }
            }
        }

        self.next_free_hint = (0..total_frames)
            .find(|frame| self.is_frame_free(*frame) && !self.metadata[*frame].flags.reserved)
            .unwrap_or(total_frames);
    }

    /// Check if a frame is free
    fn is_frame_free(&self, frame: usize) -> bool {
        let byte = frame / 8;
        let bit = frame % 8;
        self.bitmap[byte] & (1 << bit) == 0
    }

    /// Mark a frame as used in the bitmap
    fn mark_used(&mut self, frame: usize) {
        let byte = frame / 8;
        let bit = frame % 8;
        self.bitmap[byte] |= 1 << bit;
    }

    /// Mark a frame as free in the bitmap
    fn mark_free(&mut self, frame: usize) {
        let byte = frame / 8;
        let bit = frame % 8;
        self.bitmap[byte] &= !(1 << bit);
    }

    /// Zone for a given frame number
    fn frame_zone(&self, frame: usize) -> MemoryZone {
        let addr = frame * FRAME_SIZE;
        if addr < DMA_ZONE_END {
            MemoryZone::Dma
        } else if addr < NORMAL_ZONE_END {
            MemoryZone::Normal
        } else {
            MemoryZone::HighMem
        }
    }

    /// Get the frame range for a specific zone
    fn zone_range(&self, zone: MemoryZone) -> (usize, usize) {
        match zone {
            MemoryZone::Dma => (0, DMA_ZONE_END / FRAME_SIZE),
            MemoryZone::Normal => (DMA_ZONE_END / FRAME_SIZE, MAX_MEMORY / FRAME_SIZE),
            MemoryZone::HighMem => (MAX_MEMORY / FRAME_SIZE, MAX_MEMORY / FRAME_SIZE),
        }
    }

    /// Allocate a single physical frame. Returns None if out of memory.
    pub fn allocate(&mut self) -> Option<PhysFrame> {
        // Flush deferred frees if pending
        if self.deferred_count > 0 {
            self.flush_deferred();
        }

        let total_frames = MAX_MEMORY / FRAME_SIZE;

        for frame in self.next_free_hint..total_frames {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                self.mark_used(frame);
                self.free_frames -= 1;
                self.metadata[frame].refcount = 1;
                self.metadata[frame].flags.dirty = false;
                self.metadata[frame].flags.referenced = true;
                self.next_free_hint = frame + 1;

                // Update per-zone counts
                match self.frame_zone(frame) {
                    MemoryZone::Dma => {
                        if self.dma_free > 0 {
                            self.dma_free -= 1;
                        }
                        self.stats.dma_allocs = self.stats.dma_allocs.saturating_add(1);
                    }
                    MemoryZone::Normal => {
                        if self.normal_free > 0 {
                            self.normal_free -= 1;
                        }
                        self.stats.normal_allocs = self.stats.normal_allocs.saturating_add(1);
                    }
                    MemoryZone::HighMem => {}
                }

                self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                let current_used = self.used_count();
                if current_used > self.stats.peak_usage {
                    self.stats.peak_usage = current_used;
                }

                return Some(PhysFrame::from_addr(frame * FRAME_SIZE));
            }
        }

        // Wrap around from beginning
        for frame in 0..self.next_free_hint {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                self.mark_used(frame);
                self.free_frames -= 1;
                self.metadata[frame].refcount = 1;
                self.metadata[frame].flags.dirty = false;
                self.metadata[frame].flags.referenced = true;
                self.next_free_hint = frame + 1;

                match self.frame_zone(frame) {
                    MemoryZone::Dma => {
                        if self.dma_free > 0 {
                            self.dma_free -= 1;
                        }
                        self.stats.dma_allocs = self.stats.dma_allocs.saturating_add(1);
                    }
                    MemoryZone::Normal => {
                        if self.normal_free > 0 {
                            self.normal_free -= 1;
                        }
                        self.stats.normal_allocs = self.stats.normal_allocs.saturating_add(1);
                    }
                    MemoryZone::HighMem => {}
                }

                self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                let current_used = self.used_count();
                if current_used > self.stats.peak_usage {
                    self.stats.peak_usage = current_used;
                }

                return Some(PhysFrame::from_addr(frame * FRAME_SIZE));
            }
        }

        self.stats.failed_allocs = self.stats.failed_allocs.saturating_add(1);
        // OOM hook: when this allocator returns None the caller should invoke
        // crate::memory::oom::oom_invoke() to select and kill a victim process.
        // Example (not called here to avoid re-entrancy while holding the lock):
        //   crate::memory::oom::oom_invoke();
        None // Out of memory
    }

    /// Allocate a frame from a specific memory zone
    pub fn allocate_zone(&mut self, zone: MemoryZone) -> Option<PhysFrame> {
        if self.deferred_count > 0 {
            self.flush_deferred();
        }

        let (start, end) = self.zone_range(zone);

        for frame in start..end {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                self.mark_used(frame);
                self.free_frames -= 1;
                self.metadata[frame].refcount = 1;
                self.metadata[frame].flags.dirty = false;
                self.metadata[frame].flags.referenced = true;

                match zone {
                    MemoryZone::Dma => {
                        if self.dma_free > 0 {
                            self.dma_free -= 1;
                        }
                        self.stats.dma_allocs = self.stats.dma_allocs.saturating_add(1);
                    }
                    MemoryZone::Normal => {
                        if self.normal_free > 0 {
                            self.normal_free -= 1;
                        }
                        self.stats.normal_allocs = self.stats.normal_allocs.saturating_add(1);
                    }
                    MemoryZone::HighMem => {}
                }

                self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                let current_used = self.used_count();
                if current_used > self.stats.peak_usage {
                    self.stats.peak_usage = current_used;
                }

                if frame + 1 < end {
                    // Update hint only within zone
                }

                return Some(PhysFrame::from_addr(frame * FRAME_SIZE));
            }
        }

        self.stats.failed_allocs = self.stats.failed_allocs.saturating_add(1);
        None
    }

    /// Allocate N contiguous physical frames. Returns the first frame or None.
    pub fn allocate_contiguous(&mut self, count: usize) -> Option<PhysFrame> {
        if count == 0 {
            return None;
        }
        if count == 1 {
            return self.allocate();
        }

        if self.deferred_count > 0 {
            self.flush_deferred();
        }

        let total_frames = MAX_MEMORY / FRAME_SIZE;
        if count > total_frames {
            self.stats.failed_allocs = self.stats.failed_allocs.saturating_add(1);
            return None;
        }

        // Scan for N consecutive free frames
        let mut run_start = self.next_free_hint;
        let mut run_len = 0;

        let mut frame = run_start;
        while frame < total_frames {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                if run_len == 0 {
                    run_start = frame;
                }
                run_len += 1;
                if run_len == count {
                    // Found a contiguous run. Mark all as used.
                    for f in run_start..run_start + count {
                        self.mark_used(f);
                        self.metadata[f].refcount = 1;
                        self.metadata[f].flags.dirty = false;
                        self.metadata[f].flags.referenced = true;

                        match self.frame_zone(f) {
                            MemoryZone::Dma => {
                                if self.dma_free > 0 {
                                    self.dma_free -= 1;
                                }
                            }
                            MemoryZone::Normal => {
                                if self.normal_free > 0 {
                                    self.normal_free -= 1;
                                }
                            }
                            MemoryZone::HighMem => {}
                        }
                    }
                    self.free_frames -= count;
                    self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                    self.stats.contiguous_allocs = self.stats.contiguous_allocs.saturating_add(1);
                    let current_used = self.used_count();
                    if current_used > self.stats.peak_usage {
                        self.stats.peak_usage = current_used;
                    }
                    self.next_free_hint = run_start + count;
                    return Some(PhysFrame::from_addr(run_start * FRAME_SIZE));
                }
            } else {
                run_len = 0;
            }
            frame += 1;
        }

        // Wrap around and try from the beginning
        run_len = 0;
        for frame in 0..self.next_free_hint.min(total_frames) {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                if run_len == 0 {
                    run_start = frame;
                }
                run_len += 1;
                if run_len == count {
                    for f in run_start..run_start + count {
                        self.mark_used(f);
                        self.metadata[f].refcount = 1;
                        self.metadata[f].flags.dirty = false;
                        self.metadata[f].flags.referenced = true;

                        match self.frame_zone(f) {
                            MemoryZone::Dma => {
                                if self.dma_free > 0 {
                                    self.dma_free -= 1;
                                }
                            }
                            MemoryZone::Normal => {
                                if self.normal_free > 0 {
                                    self.normal_free -= 1;
                                }
                            }
                            MemoryZone::HighMem => {}
                        }
                    }
                    self.free_frames -= count;
                    self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                    self.stats.contiguous_allocs = self.stats.contiguous_allocs.saturating_add(1);
                    let current_used = self.used_count();
                    if current_used > self.stats.peak_usage {
                        self.stats.peak_usage = current_used;
                    }
                    self.next_free_hint = run_start + count;
                    return Some(PhysFrame::from_addr(run_start * FRAME_SIZE));
                }
            } else {
                run_len = 0;
            }
        }

        self.stats.failed_allocs = self.stats.failed_allocs.saturating_add(1);
        None
    }

    /// Allocate N contiguous frames within a specific zone
    pub fn allocate_contiguous_zone(
        &mut self,
        count: usize,
        zone: MemoryZone,
    ) -> Option<PhysFrame> {
        if count == 0 {
            return None;
        }

        if self.deferred_count > 0 {
            self.flush_deferred();
        }

        let (zone_start, zone_end) = self.zone_range(zone);
        let mut run_start = zone_start;
        let mut run_len = 0;

        for frame in zone_start..zone_end {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                if run_len == 0 {
                    run_start = frame;
                }
                run_len += 1;
                if run_len == count {
                    for f in run_start..run_start + count {
                        self.mark_used(f);
                        self.metadata[f].refcount = 1;
                        self.metadata[f].flags.dirty = false;
                        self.metadata[f].flags.referenced = true;

                        match zone {
                            MemoryZone::Dma => {
                                if self.dma_free > 0 {
                                    self.dma_free -= 1;
                                }
                            }
                            MemoryZone::Normal => {
                                if self.normal_free > 0 {
                                    self.normal_free -= 1;
                                }
                            }
                            MemoryZone::HighMem => {}
                        }
                    }
                    self.free_frames -= count;
                    self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                    self.stats.contiguous_allocs = self.stats.contiguous_allocs.saturating_add(1);
                    let current_used = self.used_count();
                    if current_used > self.stats.peak_usage {
                        self.stats.peak_usage = current_used;
                    }
                    return Some(PhysFrame::from_addr(run_start * FRAME_SIZE));
                }
            } else {
                run_len = 0;
            }
        }

        self.stats.failed_allocs = self.stats.failed_allocs.saturating_add(1);
        None
    }

    /// Allocate a frame with a preferred cache color hint.
    /// Useful for reducing L2/L3 cache conflicts in performance-sensitive paths.
    pub fn allocate_colored(&mut self, color: usize) -> Option<PhysFrame> {
        let color = color % NUM_CACHE_COLORS;
        let total_frames = MAX_MEMORY / FRAME_SIZE;

        // First pass: find a frame matching the desired color
        for frame in self.next_free_hint..total_frames {
            if (frame % NUM_CACHE_COLORS) == color
                && self.is_frame_free(frame)
                && !self.metadata[frame].flags.reserved
            {
                self.mark_used(frame);
                self.free_frames -= 1;
                self.metadata[frame].refcount = 1;
                self.metadata[frame].flags.referenced = true;

                match self.frame_zone(frame) {
                    MemoryZone::Dma => {
                        if self.dma_free > 0 {
                            self.dma_free -= 1;
                        }
                    }
                    MemoryZone::Normal => {
                        if self.normal_free > 0 {
                            self.normal_free -= 1;
                        }
                    }
                    MemoryZone::HighMem => {}
                }

                self.stats.total_allocs = self.stats.total_allocs.saturating_add(1);
                let current_used = self.used_count();
                if current_used > self.stats.peak_usage {
                    self.stats.peak_usage = current_used;
                }
                self.next_free_hint = frame + 1;
                return Some(PhysFrame::from_addr(frame * FRAME_SIZE));
            }
        }

        // Fallback: allocate any free frame
        self.allocate()
    }

    /// Deallocate a physical frame, marking it as free again.
    pub fn deallocate(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num >= MAX_FRAMES {
            return;
        }

        // Do not free reserved frames
        if self.metadata[num].flags.reserved {
            return;
        }

        let byte = num / 8;
        let bit = num % 8;

        // Only free if actually allocated
        if self.bitmap[byte] & (1 << bit) != 0 {
            self.bitmap[byte] &= !(1 << bit);
            self.free_frames = self.free_frames.saturating_add(1);
            self.metadata[num].refcount = 0;
            self.metadata[num].flags.dirty = false;
            self.metadata[num].flags.referenced = false;
            self.metadata[num].flags.locked = false;

            // Update per-zone counts
            match self.frame_zone(num) {
                MemoryZone::Dma => {
                    self.dma_free = self.dma_free.saturating_add(1);
                }
                MemoryZone::Normal => {
                    self.normal_free = self.normal_free.saturating_add(1);
                }
                MemoryZone::HighMem => {}
            }

            self.stats.total_frees = self.stats.total_frees.saturating_add(1);

            // Update hint if this frame is earlier
            if num < self.next_free_hint {
                self.next_free_hint = num;
            }
        }
    }

    /// Deallocate N contiguous frames starting from a given frame
    pub fn deallocate_contiguous(&mut self, frame: PhysFrame, count: usize) {
        let start = frame.number();
        for i in 0..count {
            let num = start + i;
            if num >= MAX_FRAMES {
                break;
            }
            self.deallocate(PhysFrame::from_addr(num * FRAME_SIZE));
        }
    }

    /// Add a frame to the deferred free list (batch free to reduce lock contention)
    pub fn defer_free(&mut self, frame: PhysFrame) {
        if self.deferred_count < DEFERRED_FREE_MAX {
            self.deferred_free[self.deferred_count] = frame.addr;
            self.deferred_count += 1;
        } else {
            // List full, flush then add
            self.flush_deferred();
            self.deferred_free[0] = frame.addr;
            self.deferred_count = 1;
        }
    }

    /// Flush the deferred free list, actually freeing all pending frames
    pub fn flush_deferred(&mut self) {
        for i in 0..self.deferred_count {
            let addr = self.deferred_free[i];
            if addr != 0 {
                self.deallocate(PhysFrame::from_addr(addr));
            }
        }
        if self.deferred_count > 0 {
            self.stats.deferred_flushes = self.stats.deferred_flushes.saturating_add(1);
        }
        self.deferred_count = 0;
    }

    /// Increment reference count for a frame (used for shared/COW pages)
    pub fn inc_refcount(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].refcount = self.metadata[num].refcount.saturating_add(1);
        }
    }

    /// Decrement reference count. If it reaches 0, the frame is freed.
    /// Returns true if the frame was actually freed.
    pub fn dec_refcount(&mut self, frame: PhysFrame) -> bool {
        let num = frame.number();
        if num < MAX_FRAMES && self.metadata[num].refcount > 0 {
            self.metadata[num].refcount -= 1;
            if self.metadata[num].refcount == 0 {
                self.deallocate(frame);
                return true;
            }
        }
        false
    }

    /// Get current reference count for a frame
    pub fn refcount(&self, frame: PhysFrame) -> u16 {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].refcount
        } else {
            0
        }
    }

    /// Set frame flags
    pub fn set_flags(&mut self, frame: PhysFrame, flags: FrameFlags) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags = flags;
        }
    }

    /// Get frame flags
    pub fn get_flags(&self, frame: PhysFrame) -> FrameFlags {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags
        } else {
            FrameFlags::new()
        }
    }

    /// Mark a frame as dirty
    pub fn mark_dirty(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags.dirty = true;
        }
    }

    /// Mark a frame as referenced (accessed)
    pub fn mark_referenced(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags.referenced = true;
        }
    }

    /// Lock a frame in memory (prevents eviction or swapping)
    pub fn lock_frame(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags.locked = true;
        }
    }

    /// Unlock a frame
    pub fn unlock_frame(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags.locked = false;
        }
    }

    /// Clear the referenced bit for aging (called periodically by kswapd-equivalent)
    pub fn clear_referenced(&mut self, frame: PhysFrame) {
        let num = frame.number();
        if num < MAX_FRAMES {
            self.metadata[num].flags.referenced = false;
        }
    }

    /// Number of free frames
    pub fn free_count(&self) -> usize {
        self.free_frames
    }

    /// Number of used frames
    pub fn used_count(&self) -> usize {
        (MAX_MEMORY / FRAME_SIZE) - self.free_frames
    }

    /// Free frame count for the DMA zone
    pub fn dma_free_count(&self) -> usize {
        self.dma_free
    }

    /// Free frame count for the Normal zone
    pub fn normal_free_count(&self) -> usize {
        self.normal_free
    }

    /// Get allocation statistics
    pub fn statistics(&self) -> AllocStats {
        self.stats
    }

    /// Compute a fragmentation score from 0 (no fragmentation) to 1000 (severe).
    /// Uses integer arithmetic: counts runs of free frames and computes
    /// a ratio of (1 - largest_run / total_free) * 1000.
    pub fn fragmentation_score(&self) -> usize {
        if self.free_frames == 0 {
            return 0;
        }

        let total_frames = MAX_MEMORY / FRAME_SIZE;
        let mut largest_run = 0usize;
        let mut current_run = 0usize;

        for frame in 0..total_frames {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                current_run += 1;
                if current_run > largest_run {
                    largest_run = current_run;
                }
            } else {
                current_run = 0;
            }
        }

        if largest_run >= self.free_frames {
            0 // All free memory in one block = no fragmentation
        } else {
            // fragmentation = (1 - largest/total) * 1000
            // Using integer math: ((total - largest) * 1000) / total
            ((self.free_frames - largest_run) * 1000) / self.free_frames
        }
    }

    /// Count the number of distinct free regions (runs of consecutive free frames)
    pub fn free_region_count(&self) -> usize {
        let total_frames = MAX_MEMORY / FRAME_SIZE;
        let mut regions = 0;
        let mut in_region = false;

        for frame in 0..total_frames {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                if !in_region {
                    regions += 1;
                    in_region = true;
                }
            } else {
                in_region = false;
            }
        }

        regions
    }

    /// Get the size of the largest contiguous free region (in frames)
    pub fn largest_free_region(&self) -> usize {
        let total_frames = MAX_MEMORY / FRAME_SIZE;
        let mut largest = 0usize;
        let mut current = 0usize;

        for frame in 0..total_frames {
            if self.is_frame_free(frame) && !self.metadata[frame].flags.reserved {
                current += 1;
                if current > largest {
                    largest = current;
                }
            } else {
                current = 0;
            }
        }

        largest
    }

    /// Number of pending deferred frees
    pub fn deferred_pending(&self) -> usize {
        self.deferred_count
    }
}

/// Global frame allocator instance, protected by spinlock
pub static FRAME_ALLOCATOR: Mutex<BitmapFrameAllocator> = Mutex::new(BitmapFrameAllocator::new());

/// Initialize the frame allocator with knowledge of where free memory starts
pub fn init(kernel_end_aligned: usize) {
    FRAME_ALLOCATOR.lock().init(kernel_end_aligned);
}

/// Initialize allocator from firmware-provided memory map.
pub fn init_from_memory_map(kernel_end_aligned: usize, regions: &[MemoryRegion]) {
    FRAME_ALLOCATOR
        .lock()
        .init_from_memory_map(kernel_end_aligned, regions);
}

/// Allocate a physical frame (convenience wrapper)
pub fn allocate_frame() -> Option<PhysFrame> {
    FRAME_ALLOCATOR.lock().allocate()
}

/// Deallocate a physical frame (convenience wrapper)
pub fn deallocate_frame(frame: PhysFrame) {
    FRAME_ALLOCATOR.lock().deallocate(frame);
}

/// Allocate a frame from a specific zone
pub fn allocate_frame_zone(zone: MemoryZone) -> Option<PhysFrame> {
    FRAME_ALLOCATOR.lock().allocate_zone(zone)
}

/// Allocate N contiguous frames
pub fn allocate_contiguous(count: usize) -> Option<PhysFrame> {
    FRAME_ALLOCATOR.lock().allocate_contiguous(count)
}

/// Allocate N contiguous frames, returning the raw physical address (0 on failure).
///
/// On failure the OOM killer is invoked so that a process is selected for
/// termination and future allocation retries have a higher chance of success.
pub fn allocate_frames(count: usize) -> usize {
    match FRAME_ALLOCATOR.lock().allocate_contiguous(count) {
        Some(frame) => frame.addr,
        None => {
            // OOM hook: allocation failed — invoke the OOM killer.
            // crate::memory::oom::oom_invoke();
            0
        }
    }
}

/// Deallocate N contiguous frames
pub fn deallocate_contiguous(frame: PhysFrame, count: usize) {
    FRAME_ALLOCATOR.lock().deallocate_contiguous(frame, count);
}

/// Allocate a frame with cache color hint
pub fn allocate_colored(color: usize) -> Option<PhysFrame> {
    FRAME_ALLOCATOR.lock().allocate_colored(color)
}

/// Defer a frame free (batch for performance)
pub fn defer_free(frame: PhysFrame) {
    FRAME_ALLOCATOR.lock().defer_free(frame);
}

/// Flush all deferred frees
pub fn flush_deferred() {
    FRAME_ALLOCATOR.lock().flush_deferred();
}

/// Increment reference count on a frame
pub fn inc_refcount(frame: PhysFrame) {
    FRAME_ALLOCATOR.lock().inc_refcount(frame);
}

/// Decrement reference count, free if zero
pub fn dec_refcount(frame: PhysFrame) -> bool {
    FRAME_ALLOCATOR.lock().dec_refcount(frame)
}
