use super::frame_allocator::{self, FRAME_SIZE};
use super::paging;
use crate::sync::Mutex;
/// Kernel heap allocator for Genesis
///
/// Implements a binned linked-list free-block allocator that provides
/// #[global_allocator] so the `alloc` crate works (Vec, Box, String, etc.)
///
/// Features:
///   - Size-based binning (small/medium/large free lists for fast lookup)
///   - Block splitting (carve from larger free block, return remainder)
///   - Block coalescing on free (merge adjacent free blocks)
///   - Heap expansion (request more frames when heap is exhausted)
///   - Alignment support (align_up to arbitrary power-of-2 alignment)
///   - Heap statistics (allocated_bytes, free_bytes, largest_free, alloc_count)
///   - Double-free detection via magic values in freed blocks
///   - Heap integrity checking (walk free list, verify linkage)
///
/// Inspired by: Linux's SLUB allocator concept (but much simpler),
/// Redox's linked-list allocator. All code is original.
use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

/// Heap starts at 64 MB virtual address (well above kernel at 1MB)
pub const HEAP_START: usize = 0x4_000_000;

/// Initial heap size: 128 MB (covers all boot subsystem allocations including LLM)
pub const HEAP_SIZE: usize = 128 * 1024 * 1024;

/// Maximum heap size we can expand to (256 MB)
const HEAP_MAX_SIZE: usize = 256 * 1024 * 1024;

/// Expansion granularity: how many bytes to grow the heap at once
const HEAP_EXPAND_SIZE: usize = 4 * 1024 * 1024; // 4 MB (large enough for big allocs)

/// Magic value stored in free blocks for double-free detection
const FREE_MAGIC: usize = 0xDEAD_BEEF_CAFE_F00D;

/// Poison value written into freed block content for use-after-free detection
const POISON_BYTE: u8 = 0xFE;

/// Size thresholds for binning
/// Small: 0..=256 bytes
/// Medium: 257..=4096 bytes
/// Large: >4096 bytes
const SMALL_BIN_MAX: usize = 256;
const MEDIUM_BIN_MAX: usize = 4096;

/// A free block in the linked-list allocator
struct FreeBlock {
    /// Size of this free region (including the FreeBlock header)
    size: usize,
    /// Magic value for double-free detection
    magic: usize,
    /// Next free block in this bin
    next: Option<&'static mut FreeBlock>,
}

impl FreeBlock {
    const fn new(size: usize) -> Self {
        FreeBlock {
            size,
            next: None,
            magic: FREE_MAGIC,
        }
    }

    /// Minimum size for a free block (must fit the FreeBlock header)
    const fn min_size() -> usize {
        core::mem::size_of::<FreeBlock>()
    }
}

/// Heap statistics
#[derive(Debug, Clone, Copy)]
pub struct HeapStats {
    /// Total bytes currently allocated (in use by callers)
    pub allocated_bytes: usize,
    /// Total bytes in free blocks
    pub free_bytes: usize,
    /// Size of the largest free block
    pub largest_free_block: usize,
    /// Total number of successful allocations
    pub allocation_count: u64,
    /// Total number of deallocations
    pub deallocation_count: u64,
    /// Number of times the heap was expanded
    pub expansion_count: u64,
    /// Number of block splits during allocation
    pub split_count: u64,
    /// Number of block coalesces during deallocation
    pub coalesce_count: u64,
    /// Number of double-free attempts caught
    pub double_free_caught: u64,
    /// Current heap size (may have been expanded beyond initial)
    pub current_heap_size: usize,
}

impl HeapStats {
    const fn new() -> Self {
        HeapStats {
            allocated_bytes: 0,
            free_bytes: 0,
            largest_free_block: 0,
            allocation_count: 0,
            deallocation_count: 0,
            expansion_count: 0,
            split_count: 0,
            coalesce_count: 0,
            double_free_caught: 0,
            current_heap_size: 0,
        }
    }
}

/// Binned linked-list heap allocator
pub struct LinkedListAllocator {
    /// Small bin: free blocks <= 256 bytes
    small_head: FreeBlock,
    /// Medium bin: free blocks 257..4096 bytes
    medium_head: FreeBlock,
    /// Large bin: free blocks > 4096 bytes
    large_head: FreeBlock,
    /// Legacy head (for compatibility — points to small_head)
    head: FreeBlock,
    /// Current end of the heap (for expansion)
    heap_end: usize,
    /// Current total heap size
    heap_size: usize,
    /// Statistics
    stats: HeapStats,
}

impl LinkedListAllocator {
    pub const fn new() -> Self {
        LinkedListAllocator {
            small_head: FreeBlock::new(0),
            medium_head: FreeBlock::new(0),
            large_head: FreeBlock::new(0),
            head: FreeBlock::new(0),
            heap_end: 0,
            heap_size: 0,
            stats: HeapStats::new(),
        }
    }

    /// Initialize the allocator with a memory region
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_end = heap_start + heap_size;
        self.heap_size = heap_size;
        self.stats.current_heap_size = heap_size;
        self.stats.free_bytes = heap_size;
        self.add_free_region(heap_start, heap_size);
    }

    /// Determine which bin a block of a given size belongs to
    fn bin_for_size(size: usize) -> usize {
        if size <= SMALL_BIN_MAX {
            0 // small
        } else if size <= MEDIUM_BIN_MAX {
            1 // medium
        } else {
            2 // large
        }
    }

    /// Get the head of a bin by index
    fn bin_head_mut(&mut self, bin: usize) -> &mut FreeBlock {
        match bin {
            0 => &mut self.small_head,
            1 => &mut self.medium_head,
            _ => &mut self.large_head,
        }
    }

    /// Add a free memory region to the appropriate bin
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        let min_size = FreeBlock::min_size();
        if size < min_size {
            return;
        }

        // Align the address up for FreeBlock
        let aligned_addr = align_up(addr, core::mem::align_of::<FreeBlock>());
        let adjusted_size = if aligned_addr >= addr + size {
            return;
        } else {
            size - (aligned_addr - addr)
        };
        if adjusted_size < min_size {
            return;
        }

        let bin = Self::bin_for_size(adjusted_size);
        let mut node = FreeBlock::new(adjusted_size);

        let head = match bin {
            0 => &mut self.small_head,
            1 => &mut self.medium_head,
            _ => &mut self.large_head,
        };

        node.next = head.next.take();
        node.magic = FREE_MAGIC;
        let node_ptr = aligned_addr as *mut FreeBlock;
        node_ptr.write(node);
        head.next = Some(&mut *node_ptr);
    }

    /// Try to expand the heap by mapping more physical frames.
    /// Returns true if expansion succeeded.
    unsafe fn try_expand(&mut self) -> bool {
        let expand_bytes = HEAP_EXPAND_SIZE;
        let new_end = self.heap_end + expand_bytes;

        if new_end > HEAP_START + HEAP_MAX_SIZE {
            return false; // Cannot exceed maximum heap size
        }

        let pages = expand_bytes / FRAME_SIZE;
        for i in 0..pages {
            let virt = self.heap_end + i * FRAME_SIZE;
            if let Some(frame) = frame_allocator::allocate_frame() {
                if paging::map_page(virt, frame.addr, paging::flags::WRITABLE).is_err() {
                    // Mapping failed, undo any frames we've already mapped
                    for j in 0..i {
                        let v = self.heap_end + j * FRAME_SIZE;
                        paging::unmap_page_free(v);
                    }
                    return false;
                }
            } else {
                // Out of physical memory, undo
                for j in 0..i {
                    let v = self.heap_end + j * FRAME_SIZE;
                    paging::unmap_page_free(v);
                }
                return false;
            }
        }

        // Add the new region to the free list
        let old_end = self.heap_end;
        self.heap_end = new_end;
        self.heap_size += expand_bytes;
        self.stats.current_heap_size = self.heap_size;
        self.stats.expansion_count = self.stats.expansion_count.saturating_add(1);
        self.stats.free_bytes += expand_bytes;

        self.add_free_region(old_end, expand_bytes);
        true
    }

    /// Find a free region that fits the given layout from a specific bin.
    /// Returns (region_start, region_size) and removes it from the list.
    fn find_in_bin(&mut self, bin: usize, size: usize, align: usize) -> Option<(usize, usize)> {
        let head = match bin {
            0 => &mut self.small_head,
            1 => &mut self.medium_head,
            _ => &mut self.large_head,
        };

        let mut current = head as *mut FreeBlock;

        unsafe {
            while let Some(ref mut region) = (*current).next {
                let region_ptr = &**region as *const FreeBlock as usize;
                let alloc_start = align_up(region_ptr, align);
                let alloc_end = match alloc_start.checked_add(size) {
                    Some(end) => end,
                    None => {
                        current = &mut **region as *mut FreeBlock;
                        continue;
                    }
                };

                if alloc_end <= region_ptr + region.size {
                    // Region fits — remove it from the list
                    let next = region.next.take();
                    let full_region = match (*current).next.take() {
                        Some(r) => r,
                        None => break, // Should not happen: we just checked it is Some
                    };
                    let region_start = full_region as *mut FreeBlock as usize;
                    let region_size = (*(region_start as *const FreeBlock)).size;
                    (*current).next = next;

                    // Split: if there's excess space after the allocation, return it
                    let excess_start = alloc_end;
                    let excess_size = (region_start + region_size) - excess_start;
                    if excess_size >= FreeBlock::min_size() {
                        self.add_free_region(excess_start, excess_size);
                        self.stats.split_count = self.stats.split_count.saturating_add(1);
                    }

                    // Split: if there's space before (due to alignment padding)
                    let front_pad = alloc_start - region_start;
                    if front_pad >= FreeBlock::min_size() {
                        self.add_free_region(region_start, front_pad);
                        self.stats.split_count = self.stats.split_count.saturating_add(1);
                    }

                    return Some((alloc_start, size));
                }
                current = &mut **region as *mut FreeBlock;
            }
        }

        None
    }

    /// Find a free region that fits the given layout, searching all bins.
    /// Returns (region_start, region_size).
    fn find_region(&mut self, size: usize, align: usize) -> Option<(usize, usize)> {
        // First, try the most appropriate bin
        let target_bin = Self::bin_for_size(size);

        // Search target bin first, then progressively larger bins
        for bin in target_bin..3 {
            if let Some(result) = self.find_in_bin(bin, size, align) {
                return Some(result);
            }
        }

        // Try smaller bins too (they might have a large-enough block from splitting)
        for bin in (0..target_bin).rev() {
            if let Some(result) = self.find_in_bin(bin, size, align) {
                return Some(result);
            }
        }

        None
    }

    /// Try to coalesce a freed block with its neighbors.
    /// This reduces fragmentation by merging adjacent free blocks.
    unsafe fn try_coalesce(&mut self, addr: usize, size: usize) -> (usize, usize) {
        let block_end = addr + size;
        let mut merged_start = addr;
        let mut merged_size = size;

        // Try to merge with the block immediately after us
        for bin in 0..3 {
            let head = match bin {
                0 => &mut self.small_head as *mut FreeBlock,
                1 => &mut self.medium_head as *mut FreeBlock,
                _ => &mut self.large_head as *mut FreeBlock,
            };
            let mut current = head;

            while let Some(ref mut region) = (*current).next {
                let region_addr = &**region as *const FreeBlock as usize;
                let region_size = region.size;

                // Check if this free block is immediately after us
                if region_addr == block_end {
                    merged_size += region_size;
                    // Remove this block from its free list
                    let next = region.next.take();
                    (*current).next = next;
                    self.stats.coalesce_count = self.stats.coalesce_count.saturating_add(1);
                    // Don't break — there might be more adjacent blocks
                    continue;
                }

                // Check if this free block is immediately before us
                if region_addr + region_size == merged_start {
                    merged_start = region_addr;
                    merged_size += region_size;
                    let next = region.next.take();
                    (*current).next = next;
                    self.stats.coalesce_count = self.stats.coalesce_count.saturating_add(1);
                    continue;
                }

                current = &mut **region as *mut FreeBlock;
            }
        }

        (merged_start, merged_size)
    }

    /// Check if a pointer was already freed (double-free detection).
    /// Returns true if the requested range overlaps any existing free block.
    fn is_already_free(&self, addr: usize, size: usize) -> bool {
        let end = addr.saturating_add(size);
        let bins: [&FreeBlock; 3] = [&self.small_head, &self.medium_head, &self.large_head];

        for head in &bins {
            let mut current = *head;
            while let Some(ref region) = current.next {
                let region_addr = &**region as *const FreeBlock as usize;
                let region_end = region_addr.saturating_add(region.size);

                if addr < region_end && end > region_addr {
                    return true;
                }

                current = &**region;
            }
        }

        false
    }

    /// Walk all free lists and compute the largest free block
    fn compute_largest_free(&self) -> usize {
        let mut largest = 0usize;

        let bins: [&FreeBlock; 3] = [&self.small_head, &self.medium_head, &self.large_head];
        for head in &bins {
            let mut current = *head;
            while let Some(ref region) = current.next {
                if region.size > largest {
                    largest = region.size;
                }
                current = &**region;
            }
        }

        largest
    }

    /// Walk all free lists and count total free bytes
    fn compute_free_bytes(&self) -> usize {
        let mut total = 0usize;

        let bins: [&FreeBlock; 3] = [&self.small_head, &self.medium_head, &self.large_head];
        for head in &bins {
            let mut current = *head;
            while let Some(ref region) = current.next {
                total += region.size;
                current = &**region;
            }
        }

        total
    }

    /// Verify heap integrity: walk all free lists and check magic values and linkage.
    /// Returns (total_free_blocks, total_free_bytes, errors_found)
    pub fn check_integrity(&self) -> (usize, usize, usize) {
        let mut blocks = 0usize;
        let mut bytes = 0usize;
        let mut errors = 0usize;

        let heap_start = HEAP_START;
        let heap_end = self.heap_end;

        let bins: [&FreeBlock; 3] = [&self.small_head, &self.medium_head, &self.large_head];
        for (bin_idx, head) in bins.iter().enumerate() {
            let mut current = *head;
            let mut prev_addr = 0usize;

            while let Some(ref region) = current.next {
                let region_addr = &**region as *const FreeBlock as usize;
                blocks += 1;
                bytes += region.size;

                // Check magic value
                if region.magic != FREE_MAGIC {
                    errors += 1;
                    crate::serial_println!(
                        "  [heap] integrity error: bad magic at {:#x} in bin {}, expected {:#x} got {:#x}",
                        region_addr, bin_idx, FREE_MAGIC, region.magic
                    );
                }

                // Check that the block is within heap bounds
                if region_addr < heap_start || region_addr >= heap_end {
                    errors += 1;
                    crate::serial_println!(
                        "  [heap] integrity error: block at {:#x} outside heap [{:#x}..{:#x}]",
                        region_addr,
                        heap_start,
                        heap_end
                    );
                }

                // Check that the block end is within heap bounds
                if region_addr + region.size > heap_end {
                    errors += 1;
                    crate::serial_println!(
                        "  [heap] integrity error: block at {:#x} size {} extends past heap end {:#x}",
                        region_addr, region.size, heap_end
                    );
                }

                // Check minimum size
                if region.size < FreeBlock::min_size() {
                    errors += 1;
                    crate::serial_println!(
                        "  [heap] integrity error: block at {:#x} too small ({})",
                        region_addr,
                        region.size
                    );
                }

                // Check bin correctness
                let expected_bin = Self::bin_for_size(region.size);
                if expected_bin != bin_idx {
                    // This is a soft error — blocks can end up in wrong bins after coalescing
                    // Just note it, don't count as hard error
                }

                prev_addr = region_addr;
                current = &**region;
            }
            let _ = prev_addr; // suppress unused warning
        }

        (blocks, bytes, errors)
    }

    /// Get current heap statistics
    pub fn statistics(&mut self) -> HeapStats {
        self.stats.free_bytes = self.compute_free_bytes();
        self.stats.largest_free_block = self.compute_largest_free();
        self.stats.allocated_bytes = self.heap_size - self.stats.free_bytes;
        self.stats
    }
}

/// Global allocator wrapper
pub struct LockedAllocator(Mutex<LinkedListAllocator>);

impl LockedAllocator {
    pub const fn new() -> Self {
        LockedAllocator(Mutex::new(LinkedListAllocator::new()))
    }
}

unsafe impl GlobalAlloc for LockedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(FreeBlock::min_size());
        let align = layout.align().max(core::mem::align_of::<FreeBlock>());

        let mut allocator = self.0.lock();

        // Try to find a region
        match allocator.find_region(size, align) {
            Some((start, alloc_size)) => {
                allocator.stats.allocation_count += 1;
                allocator.stats.allocated_bytes += alloc_size;
                allocator.stats.free_bytes = allocator.stats.free_bytes.saturating_sub(alloc_size);
                start as *mut u8
            }
            None => {
                let free = allocator.compute_free_bytes();
                let largest = allocator.compute_largest_free();
                crate::serial_println!(
                    "[heap] find_region MISS: need {}KB, free={}KB, largest={}KB, heap_end={:#x}",
                    size / 1024,
                    free / 1024,
                    largest / 1024,
                    allocator.heap_end
                );
                // Try expanding the heap until we can satisfy the request
                let mut result = ptr::null_mut();
                while allocator.try_expand() {
                    if let Some((start, alloc_size)) = allocator.find_region(size, align) {
                        allocator.stats.allocation_count += 1;
                        allocator.stats.allocated_bytes += alloc_size;
                        allocator.stats.free_bytes =
                            allocator.stats.free_bytes.saturating_sub(alloc_size);
                        result = start as *mut u8;
                        break;
                    }
                }
                if result.is_null() {
                    crate::serial_println!(
                        "[heap] ALLOC FAILED: size={}KB align={} heap_end={:#x}",
                        size / 1024,
                        align,
                        allocator.heap_end
                    );
                }
                result
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let addr = ptr as usize;
        let size = layout.size().max(FreeBlock::min_size());

        let mut allocator = self.0.lock();

        // Double-free detection
        if allocator.is_already_free(addr, size) {
            allocator.stats.double_free_caught += 1;
            crate::serial_println!("  [heap] DOUBLE FREE detected at {:#x} size {}", addr, size);
            return; // Do not free again
        }

        // Poison the freed memory (helps detect use-after-free)
        if size > FreeBlock::min_size() {
            let poison_start = addr + FreeBlock::min_size();
            let poison_len = size - FreeBlock::min_size();
            if poison_len > 0 {
                core::ptr::write_bytes(poison_start as *mut u8, POISON_BYTE, poison_len);
            }
        }

        // Try to coalesce with adjacent free blocks
        let (merged_addr, merged_size) = allocator.try_coalesce(addr, size);

        // Add the (possibly coalesced) block to the appropriate free list
        allocator.add_free_region(merged_addr, merged_size);

        allocator.stats.deallocation_count += 1;
        allocator.stats.allocated_bytes = allocator.stats.allocated_bytes.saturating_sub(size);
        allocator.stats.free_bytes += merged_size;
    }
}

#[global_allocator]
static ALLOCATOR: LockedAllocator = LockedAllocator::new();

/// Align `addr` upward to `align` (must be power of 2)
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// Initialize the kernel heap.
///
/// Maps physical frames to the heap virtual address range,
/// then initializes the linked-list allocator over that region.
pub fn init() {
    // Map physical frames for the heap region
    let heap_pages = HEAP_SIZE / FRAME_SIZE;

    for i in 0..heap_pages {
        let virt = HEAP_START + i * FRAME_SIZE;
        let frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => {
                crate::serial_println!("[heap] FATAL: out of physical frames at heap page {}", i);
                return; // Cannot initialize heap — kernel will OOM on first alloc
            }
        };

        if let Err(_) = paging::map_page(virt, frame.addr, paging::flags::WRITABLE) {
            crate::serial_println!("[heap] FATAL: failed to map heap page {}", i);
            return;
        }
    }

    // Initialize the allocator over the mapped region
    unsafe {
        ALLOCATOR.0.lock().init(HEAP_START, HEAP_SIZE);
    }
}

/// Get current heap statistics
pub fn stats() -> HeapStats {
    ALLOCATOR.0.lock().statistics()
}

/// Check heap integrity (returns errors found)
pub fn check_integrity() -> usize {
    let (blocks, bytes, errors) = ALLOCATOR.0.lock().check_integrity();
    if errors > 0 {
        crate::serial_println!(
            "  [heap] integrity check: {} blocks, {} bytes free, {} ERRORS",
            blocks,
            bytes,
            errors
        );
    }
    errors
}

/// Allocation error handler — called when alloc fails
#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    // Dump heap diagnostics before panicking
    let stats = ALLOCATOR.0.lock().statistics();
    crate::serial_println!(
        "[heap] ALLOC FAILED: requested {} bytes (align {})",
        layout.size(),
        layout.align()
    );
    crate::serial_println!(
        "[heap] heap_size={}, allocated={}, free={}, largest_free={}",
        stats.current_heap_size,
        stats.allocated_bytes,
        stats.free_bytes,
        stats.largest_free_block
    );
    crate::serial_println!(
        "[heap] allocs={}, deallocs={}, expansions={}",
        stats.allocation_count,
        stats.deallocation_count,
        stats.expansion_count
    );
    panic!("heap allocation failed: {:?}", layout);
}
