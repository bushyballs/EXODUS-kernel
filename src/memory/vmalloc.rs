use crate::memory::{buddy, paging};
/// vmalloc — non-contiguous virtual memory allocator for Genesis
///
/// Allocates virtually contiguous but physically non-contiguous kernel memory.
/// Used when you need a large buffer but don't need physically contiguous pages
/// (e.g., module loading, large kernel buffers, ioremap).
///
/// Manages a region of virtual address space (VMALLOC_START..VMALLOC_END)
/// and maps individual pages from the buddy allocator.
///
/// Features:
///   - Virtual address space management (track allocated VA ranges)
///   - Allocate contiguous virtual, mapped to non-contiguous physical
///   - Free vmalloc range (unmap, return frames, free VA range)
///   - VA range finding (first-fit in sorted free list)
///   - Guard pages between vmalloc allocations
///   - ioremap: map physical MMIO regions into kernel virtual space
///   - vmap: map arbitrary physical pages into contiguous virtual space
///   - Statistics: total_mapped, total_ranges, largest_free_gap
///
/// Inspired by: Linux vmalloc (mm/vmalloc.c). All code is original.
use crate::sync::Mutex;

/// vmalloc region start (256 MB, above kernel heap)
pub const VMALLOC_START: usize = 0x1000_0000;

/// vmalloc region end (1 GB)
pub const VMALLOC_END: usize = 0x4000_0000;

/// vmalloc region size
pub const VMALLOC_SIZE: usize = VMALLOC_END - VMALLOC_START;

/// Maximum number of vmalloc regions tracked
const MAX_VMALLOC_REGIONS: usize = 256;

/// Maximum number of free VA ranges tracked
const MAX_FREE_RANGES: usize = 512;

/// Guard page between vmalloc regions (prevents overflow into adjacent allocations)
const GUARD_PAGES: usize = 1;

/// Maximum pages per single vmalloc allocation
const MAX_PAGES_PER_REGION: usize = 256;

/// Region type: what kind of mapping this is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionType {
    /// vmalloc: physical pages owned, freed on vfree
    Vmalloc,
    /// ioremap: physical pages NOT owned, just unmapped on iounmap
    Ioremap,
    /// vmap: physical pages passed in by caller, not freed on unmap
    Vmap,
}

/// A vmalloc allocation
#[derive(Clone, Copy)]
struct VmallocRegion {
    /// Virtual start address
    virt_addr: usize,
    /// Number of pages (not including guard)
    num_pages: usize,
    /// Physical page addresses (stored separately)
    phys_pages: [usize; MAX_PAGES_PER_REGION],
    /// Active flag
    active: bool,
    /// Region type (vmalloc, ioremap, or vmap)
    region_type: RegionType,
    /// Caller identifier (for debugging)
    caller_id: u32,
}

impl VmallocRegion {
    const fn empty() -> Self {
        VmallocRegion {
            virt_addr: 0,
            num_pages: 0,
            phys_pages: [0; MAX_PAGES_PER_REGION],
            active: false,
            region_type: RegionType::Vmalloc,
            caller_id: 0,
        }
    }
}

/// A free virtual address range
#[derive(Clone, Copy)]
struct FreeRange {
    /// Start address
    start: usize,
    /// Size in bytes
    size: usize,
}

impl FreeRange {
    const fn empty() -> Self {
        FreeRange { start: 0, size: 0 }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VmallocStats {
    pub alloc_count: u64,
    pub free_count: u64,
    pub pages_mapped: u64,
    pub pages_unmapped: u64,
    pub ioremap_count: u64,
    pub vmap_count: u64,
    pub total_active_regions: usize,
    pub total_mapped_pages: usize,
}

/// vmalloc state
struct VmallocState {
    /// All vmalloc regions
    regions: [VmallocRegion; MAX_VMALLOC_REGIONS],
    /// Number of active regions
    count: usize,
    /// Free VA ranges (sorted by address)
    free_ranges: [FreeRange; MAX_FREE_RANGES],
    /// Number of free ranges
    free_range_count: usize,
    /// Statistics
    stats: VmallocStats,
}

impl VmallocState {
    const fn new() -> Self {
        const EMPTY: VmallocRegion = VmallocRegion::empty();
        const FREE: FreeRange = FreeRange::empty();
        VmallocState {
            regions: [EMPTY; MAX_VMALLOC_REGIONS],
            count: 0,
            free_ranges: [FREE; MAX_FREE_RANGES],
            free_range_count: 0,
            stats: VmallocStats {
                alloc_count: 0,
                free_count: 0,
                pages_mapped: 0,
                pages_unmapped: 0,
                ioremap_count: 0,
                vmap_count: 0,
                total_active_regions: 0,
                total_mapped_pages: 0,
            },
        }
    }

    /// Initialize the free range list with the entire vmalloc space
    fn init(&mut self) {
        self.free_ranges[0] = FreeRange {
            start: VMALLOC_START,
            size: VMALLOC_SIZE,
        };
        self.free_range_count = 1;
    }

    /// Find and allocate a contiguous virtual address range of the given size.
    /// Uses first-fit algorithm on the sorted free range list.
    /// Returns the virtual address or None.
    fn alloc_va_range(&mut self, size: usize) -> Option<usize> {
        let total_size = size + GUARD_PAGES * buddy::PAGE_SIZE;

        // First-fit search in sorted free ranges
        for i in 0..self.free_range_count {
            if self.free_ranges[i].size >= total_size {
                let addr = self.free_ranges[i].start;

                // Shrink or remove this free range
                if self.free_ranges[i].size == total_size {
                    // Exact fit — remove the range
                    self.remove_free_range(i);
                } else {
                    // Partial fit — shrink the range
                    self.free_ranges[i].start += total_size;
                    self.free_ranges[i].size -= total_size;
                }

                return Some(addr);
            }
        }

        None
    }

    /// Return a virtual address range to the free list.
    /// Coalesces with adjacent free ranges.
    fn free_va_range(&mut self, addr: usize, size: usize) {
        let total_size = size + GUARD_PAGES * buddy::PAGE_SIZE;
        let range_end = addr + total_size;

        // Try to coalesce with adjacent free ranges
        let mut coalesced = false;

        for i in 0..self.free_range_count {
            let free_start = self.free_ranges[i].start;
            let free_end = free_start + self.free_ranges[i].size;

            // Check if this range is immediately before us
            if free_end == addr {
                self.free_ranges[i].size += total_size;

                // Also check if the next range can be merged
                for j in 0..self.free_range_count {
                    if j != i
                        && self.free_ranges[j].start
                            == self.free_ranges[i].start + self.free_ranges[i].size
                    {
                        self.free_ranges[i].size += self.free_ranges[j].size;
                        self.remove_free_range(j);
                        break;
                    }
                }
                coalesced = true;
                break;
            }

            // Check if this range is immediately after us
            if free_start == range_end {
                self.free_ranges[i].start = addr;
                self.free_ranges[i].size += total_size;

                // Also check if the previous range can be merged
                for j in 0..self.free_range_count {
                    if j != i {
                        let j_end = self.free_ranges[j].start + self.free_ranges[j].size;
                        if j_end == addr {
                            self.free_ranges[j].size += self.free_ranges[i].size;
                            self.remove_free_range(i);
                            break;
                        }
                    }
                }
                coalesced = true;
                break;
            }
        }

        if !coalesced {
            // Add as a new free range
            self.add_free_range(addr, total_size);
        }

        // Sort free ranges by address (insertion sort is fine for small N)
        self.sort_free_ranges();
    }

    /// Add a free range to the list
    fn add_free_range(&mut self, start: usize, size: usize) {
        if self.free_range_count < MAX_FREE_RANGES {
            self.free_ranges[self.free_range_count] = FreeRange { start, size };
            self.free_range_count += 1;
        }
    }

    /// Remove a free range by index
    fn remove_free_range(&mut self, idx: usize) {
        if idx < self.free_range_count {
            for i in idx..self.free_range_count - 1 {
                self.free_ranges[i] = self.free_ranges[i + 1];
            }
            self.free_range_count -= 1;
        }
    }

    /// Sort free ranges by start address
    fn sort_free_ranges(&mut self) {
        // Simple insertion sort (N is small)
        for i in 1..self.free_range_count {
            let key = self.free_ranges[i];
            let mut j = i;
            while j > 0 && self.free_ranges[j - 1].start > key.start {
                self.free_ranges[j] = self.free_ranges[j - 1];
                j -= 1;
            }
            self.free_ranges[j] = key;
        }
    }

    /// Compute the largest free gap in the VA space
    fn largest_free_gap(&self) -> usize {
        let mut largest = 0;
        for i in 0..self.free_range_count {
            if self.free_ranges[i].size > largest {
                largest = self.free_ranges[i].size;
            }
        }
        largest
    }

    /// Count total mapped pages across all active regions
    fn total_mapped_pages(&self) -> usize {
        let mut total = 0;
        for i in 0..MAX_VMALLOC_REGIONS {
            if self.regions[i].active {
                total += self.regions[i].num_pages;
            }
        }
        total
    }
}

static VMALLOC: Mutex<VmallocState> = Mutex::new(VmallocState::new());

/// Allocate virtually contiguous memory. Returns virtual address.
/// Size is in bytes (rounded up to pages).
pub fn vmalloc(size: usize) -> Option<*mut u8> {
    if size == 0 {
        return None;
    }

    let num_pages = (size + buddy::PAGE_SIZE - 1) / buddy::PAGE_SIZE;
    if num_pages > MAX_PAGES_PER_REGION {
        return None; // too large for one vmalloc region
    }

    let mut state = VMALLOC.lock();

    // Find a free region slot
    let region_idx = (0..MAX_VMALLOC_REGIONS).find(|&i| !state.regions[i].active)?;

    // Allocate virtual address range (with guard pages)
    let total_bytes = num_pages * buddy::PAGE_SIZE;
    let virt_addr = state.alloc_va_range(total_bytes)?;

    // Allocate physical pages and map them
    state.regions[region_idx].virt_addr = virt_addr;
    state.regions[region_idx].num_pages = num_pages;
    state.regions[region_idx].active = true;
    state.regions[region_idx].region_type = RegionType::Vmalloc;

    for i in 0..num_pages {
        let phys = match buddy::alloc_page() {
            Some(p) => p,
            None => {
                // Allocation failed — unmap and free what we've already done
                for j in 0..i {
                    let vaddr = virt_addr + j * buddy::PAGE_SIZE;
                    paging::unmap_page(vaddr);
                    buddy::free_page(state.regions[region_idx].phys_pages[j]);
                }
                state.regions[region_idx].active = false;
                state.free_va_range(virt_addr, total_bytes);
                return None;
            }
        };
        state.regions[region_idx].phys_pages[i] = phys;

        let vaddr = virt_addr + i * buddy::PAGE_SIZE;
        if paging::map_page(vaddr, phys, paging::flags::WRITABLE).is_err() {
            // Mapping failed — clean up
            for j in 0..=i {
                if j < i {
                    let v = virt_addr + j * buddy::PAGE_SIZE;
                    paging::unmap_page(v);
                }
                buddy::free_page(state.regions[region_idx].phys_pages[j]);
            }
            state.regions[region_idx].active = false;
            state.free_va_range(virt_addr, total_bytes);
            return None;
        }

        // Zero the page
        unsafe {
            core::ptr::write_bytes(vaddr as *mut u8, 0, buddy::PAGE_SIZE);
        }

        state.stats.pages_mapped += 1;
    }

    // Install guard page after the allocation
    let guard_vaddr = virt_addr + num_pages * buddy::PAGE_SIZE;
    let _ = paging::install_guard_page(guard_vaddr);

    state.count += 1;
    state.stats.alloc_count += 1;
    state.stats.total_active_regions = state.count;
    state.stats.total_mapped_pages = state.total_mapped_pages();

    Some(virt_addr as *mut u8)
}

/// Free vmalloc'd memory
pub fn vfree(ptr: *mut u8) {
    let addr = ptr as usize;
    let mut state = VMALLOC.lock();

    for i in 0..MAX_VMALLOC_REGIONS {
        if state.regions[i].active && state.regions[i].virt_addr == addr {
            let num_pages = state.regions[i].num_pages;
            let region_type = state.regions[i].region_type;
            let total_bytes = num_pages * buddy::PAGE_SIZE;

            // Unmap each page
            for j in 0..num_pages {
                let vaddr = addr + j * buddy::PAGE_SIZE;
                paging::unmap_page(vaddr);

                // Only free physical pages for vmalloc (not ioremap or vmap)
                if region_type == RegionType::Vmalloc {
                    let phys = state.regions[i].phys_pages[j];
                    buddy::free_page(phys);
                }

                state.stats.pages_unmapped += 1;
            }

            // Unmap guard page
            let guard_vaddr = addr + num_pages * buddy::PAGE_SIZE;
            paging::unmap_page(guard_vaddr);

            // Return VA range to free list
            state.free_va_range(addr, total_bytes);

            state.regions[i].active = false;
            state.count -= 1;
            state.stats.free_count += 1;
            state.stats.total_active_regions = state.count;
            state.stats.total_mapped_pages = state.total_mapped_pages();
            return;
        }
    }
}

/// ioremap — map physical MMIO region into virtual address space
pub fn ioremap(phys_addr: usize, size: usize) -> Option<*mut u8> {
    if size == 0 {
        return None;
    }

    let num_pages = (size + buddy::PAGE_SIZE - 1) / buddy::PAGE_SIZE;
    let phys_base = phys_addr & !(buddy::PAGE_SIZE - 1);

    if num_pages > MAX_PAGES_PER_REGION {
        return None;
    }

    let mut state = VMALLOC.lock();
    let region_idx = (0..MAX_VMALLOC_REGIONS).find(|&i| !state.regions[i].active)?;

    let total_bytes = num_pages * buddy::PAGE_SIZE;
    let virt_addr = state.alloc_va_range(total_bytes)?;

    state.regions[region_idx].virt_addr = virt_addr;
    state.regions[region_idx].num_pages = num_pages;
    state.regions[region_idx].active = true;
    state.regions[region_idx].region_type = RegionType::Ioremap;

    // Map the physical MMIO pages (no cache, write-through)
    for i in 0..num_pages {
        let phys = phys_base + i * buddy::PAGE_SIZE;
        let vaddr = virt_addr + i * buddy::PAGE_SIZE;
        state.regions[region_idx].phys_pages[i] = phys; // don't free these on vfree!

        let flags =
            paging::flags::WRITABLE | paging::flags::NO_CACHE | paging::flags::WRITE_THROUGH;
        if paging::map_page(vaddr, phys, flags).is_err() {
            // Clean up on failure
            for j in 0..i {
                let v = virt_addr + j * buddy::PAGE_SIZE;
                paging::unmap_page(v);
            }
            state.regions[region_idx].active = false;
            state.free_va_range(virt_addr, total_bytes);
            return None;
        }

        state.stats.pages_mapped += 1;
    }

    // Install guard page
    let guard_vaddr = virt_addr + num_pages * buddy::PAGE_SIZE;
    let _ = paging::install_guard_page(guard_vaddr);

    state.count += 1;
    state.stats.alloc_count += 1;
    state.stats.ioremap_count += 1;
    state.stats.total_active_regions = state.count;
    state.stats.total_mapped_pages = state.total_mapped_pages();

    // Return virtual address with sub-page offset preserved
    let offset = phys_addr - phys_base;
    Some((virt_addr + offset) as *mut u8)
}

/// iounmap — unmap ioremap'd region (don't free physical pages)
pub fn iounmap(ptr: *mut u8) {
    let addr = (ptr as usize) & !(buddy::PAGE_SIZE - 1);
    vfree(addr as *mut u8); // vfree handles ioremap type correctly
}

/// vmap — map arbitrary physical pages into contiguous virtual space.
/// The caller provides the physical page addresses. Pages are NOT freed on unmap.
pub fn vmap(phys_pages: &[usize], count: usize) -> Option<*mut u8> {
    if count == 0 || count > MAX_PAGES_PER_REGION {
        return None;
    }

    let mut state = VMALLOC.lock();
    let region_idx = (0..MAX_VMALLOC_REGIONS).find(|&i| !state.regions[i].active)?;

    let total_bytes = count * buddy::PAGE_SIZE;
    let virt_addr = state.alloc_va_range(total_bytes)?;

    state.regions[region_idx].virt_addr = virt_addr;
    state.regions[region_idx].num_pages = count;
    state.regions[region_idx].active = true;
    state.regions[region_idx].region_type = RegionType::Vmap;

    for i in 0..count {
        let phys = phys_pages[i];
        let vaddr = virt_addr + i * buddy::PAGE_SIZE;
        state.regions[region_idx].phys_pages[i] = phys;

        if paging::map_page(vaddr, phys, paging::flags::WRITABLE).is_err() {
            // Clean up
            for j in 0..i {
                let v = virt_addr + j * buddy::PAGE_SIZE;
                paging::unmap_page(v);
            }
            state.regions[region_idx].active = false;
            state.free_va_range(virt_addr, total_bytes);
            return None;
        }
        state.stats.pages_mapped += 1;
    }

    // Install guard page
    let guard_vaddr = virt_addr + count * buddy::PAGE_SIZE;
    let _ = paging::install_guard_page(guard_vaddr);

    state.count += 1;
    state.stats.alloc_count += 1;
    state.stats.vmap_count += 1;
    state.stats.total_active_regions = state.count;
    state.stats.total_mapped_pages = state.total_mapped_pages();

    Some(virt_addr as *mut u8)
}

/// vunmap — unmap vmap'd region (don't free physical pages)
pub fn vunmap(ptr: *mut u8) {
    vfree(ptr); // vfree handles vmap type correctly
}

/// Get information about a vmalloc region by virtual address
pub fn region_info(virt_addr: usize) -> Option<(usize, RegionType)> {
    let state = VMALLOC.lock();
    for i in 0..MAX_VMALLOC_REGIONS {
        if state.regions[i].active && state.regions[i].virt_addr == virt_addr {
            return Some((
                state.regions[i].num_pages * buddy::PAGE_SIZE,
                state.regions[i].region_type,
            ));
        }
    }
    None
}

/// Get statistics
pub fn statistics() -> VmallocStats {
    let state = VMALLOC.lock();
    state.stats
}

/// Get the largest free gap in the vmalloc VA space
pub fn largest_free_gap() -> usize {
    let state = VMALLOC.lock();
    state.largest_free_gap()
}

/// Get vmalloc info (for /proc/vmallocinfo)
pub fn vmallocinfo() -> alloc::string::String {
    use alloc::format;
    let state = VMALLOC.lock();
    let mut s = alloc::string::String::new();

    s.push_str(&format!(
        "vmalloc space: {:#x}-{:#x} ({} MB)\n",
        VMALLOC_START,
        VMALLOC_END,
        VMALLOC_SIZE / (1024 * 1024)
    ));
    s.push_str(&format!(
        "active regions: {}  mapped pages: {}\n",
        state.stats.total_active_regions, state.stats.total_mapped_pages
    ));
    s.push_str(&format!(
        "free ranges: {}  largest gap: {} KB\n",
        state.free_range_count,
        state.largest_free_gap() / 1024
    ));
    s.push_str(&format!(
        "allocs: {}  frees: {}  ioremaps: {}  vmaps: {}\n",
        state.stats.alloc_count,
        state.stats.free_count,
        state.stats.ioremap_count,
        state.stats.vmap_count
    ));
    s.push_str("\nActive regions:\n");

    for i in 0..MAX_VMALLOC_REGIONS {
        if state.regions[i].active {
            let r = &state.regions[i];
            let end = r.virt_addr + r.num_pages * buddy::PAGE_SIZE;
            let type_str = match r.region_type {
                RegionType::Vmalloc => "vmalloc",
                RegionType::Ioremap => "ioremap",
                RegionType::Vmap => "vmap",
            };
            s.push_str(&format!(
                "  {:#010x}-{:#010x} {:>8} pages={} type={}\n",
                r.virt_addr,
                end,
                r.num_pages * buddy::PAGE_SIZE,
                r.num_pages,
                type_str
            ));
        }
    }

    s.push_str("\nFree VA ranges:\n");
    for i in 0..state.free_range_count {
        let r = &state.free_ranges[i];
        s.push_str(&format!(
            "  {:#010x}-{:#010x} {:>8}\n",
            r.start,
            r.start + r.size,
            r.size
        ));
    }

    s
}

/// Initialize vmalloc subsystem
pub fn init() {
    VMALLOC.lock().init();
    crate::serial_println!(
        "  [vmalloc] region: {:#x}-{:#x} ({} MB)",
        VMALLOC_START,
        VMALLOC_END,
        VMALLOC_SIZE / (1024 * 1024)
    );
}
