use crate::serial_println;
/// mmap --- memory-mapped file support for Genesis
///
/// Provides mmap/munmap semantics for mapping files (or anonymous memory)
/// into a process's virtual address space. File-backed mappings go through
/// the page cache; anonymous mappings are zero-filled on demand.
///
/// Architecture:
///   - VMA (Virtual Memory Area) tracking per mapping
///   - File-backed mappings: pages loaded from page cache on demand
///   - Anonymous mappings: zero-filled pages allocated on fault
///   - MAP_SHARED / MAP_PRIVATE (COW) support
///   - Protection: PROT_READ, PROT_WRITE, PROT_EXEC
///   - msync: flush dirty mapped pages back to page cache
///   - munmap: unmap and optionally free pages
///   - Per-process VMA list (simplified: global table)
///
/// Inspired by: Linux mmap (mm/mmap.c, mm/filemap.c). All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum VMAs (memory mappings)
const MAX_VMAS: usize = 256;

/// Page size
const PAGE_SIZE: usize = 4096;

/// Protection flags
pub const PROT_NONE: u32 = 0;
pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

/// Map flags
pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_ANONYMOUS: u32 = 0x20;
pub const MAP_FIXED: u32 = 0x10;

/// msync flags
pub const MS_SYNC: u32 = 1;
pub const MS_ASYNC: u32 = 2;
pub const MS_INVALIDATE: u32 = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Mapping type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapType {
    /// Anonymous mapping (no file, zero-filled)
    Anonymous,
    /// File-backed mapping
    File {
        /// Inode number
        ino: u64,
        /// Offset in the file (page-aligned)
        file_offset: u64,
    },
}

/// A Virtual Memory Area (VMA) describing one mapping
pub struct Vma {
    /// Start virtual address (page-aligned)
    pub start: usize,
    /// End virtual address (exclusive, page-aligned)
    pub end: usize,
    /// Protection flags (PROT_READ | PROT_WRITE | PROT_EXEC)
    pub prot: u32,
    /// Map flags (MAP_SHARED | MAP_PRIVATE | MAP_ANONYMOUS)
    pub flags: u32,
    /// Mapping type
    pub mtype: MmapType,
    /// Owner PID
    pub pid: u32,
    /// Whether this VMA is active
    pub active: bool,
    /// Number of resident (actually mapped) pages
    pub resident_pages: usize,
    /// Whether any page has been written to (dirty)
    pub dirty: bool,
}

impl Vma {
    const fn empty() -> Self {
        Vma {
            start: 0,
            end: 0,
            prot: 0,
            flags: 0,
            mtype: MmapType::Anonymous,
            pid: 0,
            active: false,
            resident_pages: 0,
            dirty: false,
        }
    }

    /// Number of pages in this VMA
    fn num_pages(&self) -> usize {
        (self.end - self.start) / PAGE_SIZE
    }
}

/// Mmap statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct MmapStats {
    /// Total mmap calls
    pub mmap_count: u64,
    /// Total munmap calls
    pub munmap_count: u64,
    /// Total pages mapped (demand-paged on fault)
    pub pages_mapped: u64,
    /// Total pages unmapped
    pub pages_unmapped: u64,
    /// Anonymous mappings
    pub anon_maps: u64,
    /// File-backed mappings
    pub file_maps: u64,
    /// msync calls
    pub msync_count: u64,
    /// Pages synced
    pub pages_synced: u64,
    /// Demand fault fills (anonymous)
    pub demand_zero_fills: u64,
    /// Demand fault file reads
    pub demand_file_reads: u64,
}

/// Mmap manager
struct MmapManager {
    /// VMA table
    vmas: [Vma; MAX_VMAS],
    /// Number of active VMAs
    vma_count: usize,
    /// Next virtual address hint for new mappings
    next_addr: usize,
    /// Statistics
    stats: MmapStats,
}

impl MmapManager {
    const fn new() -> Self {
        const EMPTY: Vma = Vma::empty();
        MmapManager {
            vmas: [EMPTY; MAX_VMAS],
            vma_count: 0,
            next_addr: 0x0000_4000_0000_0000, // user mmap region start
            stats: MmapStats {
                mmap_count: 0,
                munmap_count: 0,
                pages_mapped: 0,
                pages_unmapped: 0,
                anon_maps: 0,
                file_maps: 0,
                msync_count: 0,
                pages_synced: 0,
                demand_zero_fills: 0,
                demand_file_reads: 0,
            },
        }
    }
}

static MMAP: Mutex<MmapManager> = Mutex::new(MmapManager::new());

/// Global counters
pub static MMAP_TOTAL: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert mmap protection flags to page table flags
fn prot_to_page_flags(prot: u32, flags: u32) -> u64 {
    let mut page_flags =
        crate::memory::paging::flags::PRESENT | crate::memory::paging::flags::USER_ACCESSIBLE;

    if prot & PROT_WRITE != 0 {
        // MAP_PRIVATE with PROT_WRITE: will use COW, initially map read-only
        if flags & MAP_PRIVATE != 0 {
            // Don't set writable yet --- COW will handle it on write fault
        } else {
            page_flags |= crate::memory::paging::flags::WRITABLE;
        }
    }

    if prot & PROT_EXEC == 0 {
        page_flags |= crate::memory::paging::flags::NO_EXECUTE;
    }

    page_flags
}

/// Find a free virtual address range of `num_pages` pages
fn find_free_range(mgr: &MmapManager, num_pages: usize) -> Option<usize> {
    let size = num_pages * PAGE_SIZE;
    let mut candidate = mgr.next_addr;

    // Simple first-fit: check that no existing VMA overlaps
    'outer: loop {
        if candidate + size > 0x0000_7FFF_FFFF_F000 {
            return None; // out of user address space
        }

        let cand_end = candidate + size;
        for i in 0..MAX_VMAS {
            if mgr.vmas[i].active {
                let vma_start = mgr.vmas[i].start;
                let vma_end = mgr.vmas[i].end;
                // Check overlap
                if candidate < vma_end && cand_end > vma_start {
                    // Overlap --- move past this VMA
                    candidate = (vma_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                    continue 'outer;
                }
            }
        }

        // No overlap found
        return Some(candidate);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Map a region of virtual memory (mmap semantics).
///
/// - `addr`: hint address (0 = kernel chooses, MAP_FIXED = must be this address)
/// - `length`: mapping size in bytes (rounded up to page boundary)
/// - `prot`: PROT_READ | PROT_WRITE | PROT_EXEC
/// - `flags`: MAP_SHARED | MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED
/// - `ino`: inode number (0 for anonymous)
/// - `offset`: file offset (must be page-aligned, 0 for anonymous)
/// - `pid`: requesting process PID
///
/// Returns the virtual address of the mapping, or None.
pub fn mmap(
    addr: usize,
    length: usize,
    prot: u32,
    flags: u32,
    ino: u64,
    offset: u64,
    pid: u32,
) -> Option<usize> {
    if length == 0 {
        return None;
    }

    let num_pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;
    let aligned_size = num_pages * PAGE_SIZE;

    let mut mgr = MMAP.lock();
    if mgr.vma_count >= MAX_VMAS {
        return None;
    }

    // Determine mapping address
    let map_addr = if flags & MAP_FIXED != 0 && addr != 0 {
        // MAP_FIXED: use exact address (must be page-aligned)
        if addr & (PAGE_SIZE - 1) != 0 {
            return None;
        }
        addr
    } else if addr != 0 {
        // Hint address: try it, fall back to auto
        let a = addr & !(PAGE_SIZE - 1);
        // Check if hint is available
        let hint_end = a + aligned_size;
        let mut ok = true;
        for i in 0..MAX_VMAS {
            if mgr.vmas[i].active {
                if a < mgr.vmas[i].end && hint_end > mgr.vmas[i].start {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            a
        } else {
            find_free_range(&mgr, num_pages)?
        }
    } else {
        find_free_range(&mgr, num_pages)?
    };

    // Find free VMA slot
    let slot = (0..MAX_VMAS).find(|&i| !mgr.vmas[i].active)?;

    let mtype = if flags & MAP_ANONYMOUS != 0 || ino == 0 {
        MmapType::Anonymous
    } else {
        MmapType::File {
            ino,
            file_offset: offset,
        }
    };

    mgr.vmas[slot] = Vma {
        start: map_addr,
        end: map_addr + aligned_size,
        prot,
        flags,
        mtype,
        pid,
        active: true,
        resident_pages: 0,
        dirty: false,
    };
    mgr.vma_count += 1;

    match mtype {
        MmapType::Anonymous => mgr.stats.anon_maps += 1,
        MmapType::File { .. } => mgr.stats.file_maps += 1,
    }
    mgr.stats.mmap_count += 1;

    // Update next_addr hint
    let after = map_addr + aligned_size;
    if after > mgr.next_addr {
        mgr.next_addr = after;
    }

    drop(mgr);

    MMAP_TOTAL.fetch_add(1, Ordering::Relaxed);

    serial_println!(
        "  [mmap] mapped {:#x}-{:#x} ({} pages) prot={:#x} flags={:#x} pid={}",
        map_addr,
        map_addr + aligned_size,
        num_pages,
        prot,
        flags,
        pid
    );

    Some(map_addr)
}

/// Unmap a region (munmap semantics).
///
/// - `addr`: start address (must be page-aligned)
/// - `length`: length to unmap
/// - `pid`: requesting process PID
pub fn munmap(addr: usize, length: usize, pid: u32) -> bool {
    if addr & (PAGE_SIZE - 1) != 0 || length == 0 {
        return false;
    }

    let num_pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;
    let end = addr + num_pages * PAGE_SIZE;

    let mut mgr = MMAP.lock();

    // Find the VMA covering this range
    let mut found = false;
    for i in 0..MAX_VMAS {
        if !mgr.vmas[i].active || mgr.vmas[i].pid != pid {
            continue;
        }
        if mgr.vmas[i].start <= addr && mgr.vmas[i].end >= end {
            let vma_pages = mgr.vmas[i].num_pages();
            let resident = mgr.vmas[i].resident_pages;

            // If the unmap covers the entire VMA, remove it
            if addr == mgr.vmas[i].start && end == mgr.vmas[i].end {
                mgr.vmas[i].active = false;
                mgr.vma_count -= 1;
            } else {
                // Partial unmap: adjust VMA boundaries
                // For simplicity, handle start-trim and end-trim
                if addr == mgr.vmas[i].start {
                    mgr.vmas[i].start = end;
                } else if end == mgr.vmas[i].end {
                    mgr.vmas[i].end = addr;
                }
                // Middle split not implemented for simplicity
            }

            mgr.stats.munmap_count += 1;
            mgr.stats.pages_unmapped += vma_pages as u64;
            let _ = resident;
            drop(mgr);

            // Unmap pages from page tables
            for p in 0..num_pages {
                let virt = addr + p * PAGE_SIZE;
                crate::memory::paging::unmap_page_free(virt);
            }

            found = true;
            break;
        }
    }

    if !found {
        return false;
    }

    serial_println!(
        "  [mmap] unmapped {:#x}-{:#x} ({} pages) pid={}",
        addr,
        end,
        num_pages,
        pid
    );
    true
}

/// Handle a page fault in a mapped region (demand paging).
///
/// Called from the page fault handler when the faulting address falls
/// within an mmap'd VMA.
///
/// Returns true if the fault was handled.
pub fn handle_fault(fault_addr: usize, _error_code: u64, pid: u32) -> bool {
    let fault_page = fault_addr & !(PAGE_SIZE - 1);

    let mgr = MMAP.lock();

    // Find the VMA containing this address
    let mut vma_idx = None;
    for i in 0..MAX_VMAS {
        if mgr.vmas[i].active
            && mgr.vmas[i].pid == pid
            && fault_page >= mgr.vmas[i].start
            && fault_page < mgr.vmas[i].end
        {
            vma_idx = Some(i);
            break;
        }
    }

    let idx = match vma_idx {
        Some(i) => i,
        None => return false,
    };

    let mtype = mgr.vmas[idx].mtype;
    let prot = mgr.vmas[idx].prot;
    let flags = mgr.vmas[idx].flags;
    let page_flags = prot_to_page_flags(prot, flags);

    match mtype {
        MmapType::Anonymous => {
            // Allocate a zero page
            drop(mgr);
            if let Some(phys) = crate::memory::buddy::alloc_page() {
                // Zero the page
                unsafe {
                    core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
                }
                if crate::memory::paging::map_page(fault_page, phys, page_flags).is_ok() {
                    let mut mgr = MMAP.lock();
                    mgr.vmas[idx].resident_pages += 1;
                    mgr.stats.demand_zero_fills += 1;
                    mgr.stats.pages_mapped += 1;
                    return true;
                }
                crate::memory::buddy::free_page(phys);
            }
        }
        MmapType::File { ino, file_offset } => {
            // Calculate the page offset within the file
            let page_in_vma = (fault_page - mgr.vmas[idx].start) / PAGE_SIZE;
            let file_page_offset = file_offset + page_in_vma as u64;
            drop(mgr);

            // Try page cache first
            if let Some(phys) = crate::memory::page_cache::read_page(ino, file_page_offset) {
                if crate::memory::paging::map_page(fault_page, phys, page_flags).is_ok() {
                    let mut mgr = MMAP.lock();
                    mgr.vmas[idx].resident_pages += 1;
                    mgr.stats.demand_file_reads += 1;
                    mgr.stats.pages_mapped += 1;
                    return true;
                }
            }

            // Page not in cache --- would need to read from disk here
            // For now, allocate a zero page as placeholder
            if let Some(phys) = crate::memory::buddy::alloc_page() {
                unsafe {
                    core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
                }
                // Insert into page cache
                crate::memory::page_cache::insert_page(ino, file_page_offset, phys);

                if crate::memory::paging::map_page(fault_page, phys, page_flags).is_ok() {
                    let mut mgr = MMAP.lock();
                    mgr.vmas[idx].resident_pages += 1;
                    mgr.stats.demand_file_reads += 1;
                    mgr.stats.pages_mapped += 1;
                    return true;
                }
                crate::memory::buddy::free_page(phys);
            }
        }
    }

    false
}

/// Sync dirty pages in a mapped region back to the page cache (msync).
///
/// - `addr`: start address (page-aligned)
/// - `length`: length to sync
/// - `flags`: MS_SYNC, MS_ASYNC, MS_INVALIDATE
/// - `pid`: requesting process PID
pub fn msync(addr: usize, length: usize, flags: u32, pid: u32) -> bool {
    let num_pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;

    let mut mgr = MMAP.lock();

    // Find the VMA
    let mut vma_idx = None;
    for i in 0..MAX_VMAS {
        if mgr.vmas[i].active
            && mgr.vmas[i].pid == pid
            && addr >= mgr.vmas[i].start
            && addr < mgr.vmas[i].end
        {
            vma_idx = Some(i);
            break;
        }
    }

    let idx = match vma_idx {
        Some(i) => i,
        None => return false,
    };

    let mtype = mgr.vmas[idx].mtype;

    match mtype {
        MmapType::File { ino, file_offset } => {
            let vma_start = mgr.vmas[idx].start;
            mgr.stats.msync_count += 1;
            drop(mgr);

            let mut synced = 0;
            for p in 0..num_pages {
                let virt = addr + p * PAGE_SIZE;
                let page_in_vma = (virt - vma_start) / PAGE_SIZE;
                let file_page = file_offset + page_in_vma as u64;

                // Check if the page is dirty (via PTE dirty bit)
                if let Some(pte) = crate::memory::paging::get_pte(virt) {
                    if pte & crate::memory::paging::flags::DIRTY != 0 {
                        // Mark in page cache as dirty
                        crate::memory::page_cache::PAGE_CACHE
                            .lock()
                            .mark_dirty(ino, file_page);
                        synced += 1;
                    }
                }
            }

            if flags & MS_SYNC != 0 {
                // Synchronous: flush page cache for this inode
                crate::memory::page_cache::sync_inode(ino);
            }

            let mut mgr = MMAP.lock();
            mgr.stats.pages_synced += synced;
            mgr.vmas[idx].dirty = false;

            serial_println!(
                "  [mmap] msync {:#x} {} pages synced for ino {}",
                addr,
                synced,
                ino
            );
            true
        }
        MmapType::Anonymous => {
            // Anonymous mappings have nothing to sync
            true
        }
    }
}

/// Find the VMA for a given virtual address and PID
pub fn find_vma(addr: usize, pid: u32) -> Option<(usize, usize, u32, u32)> {
    let mgr = MMAP.lock();
    for i in 0..MAX_VMAS {
        if mgr.vmas[i].active
            && mgr.vmas[i].pid == pid
            && addr >= mgr.vmas[i].start
            && addr < mgr.vmas[i].end
        {
            return Some((
                mgr.vmas[i].start,
                mgr.vmas[i].end,
                mgr.vmas[i].prot,
                mgr.vmas[i].flags,
            ));
        }
    }
    None
}

/// Get statistics
pub fn mmap_stats() -> MmapStats {
    MMAP.lock().stats
}

/// Get a summary string
pub fn summary() -> alloc::string::String {
    use alloc::format;
    let mgr = MMAP.lock();
    let mut s = alloc::string::String::from("Memory Mappings:\n");
    s.push_str(&format!("  Active VMAs:     {}\n", mgr.vma_count));
    s.push_str(&format!("  mmap calls:      {}\n", mgr.stats.mmap_count));
    s.push_str(&format!("  munmap calls:    {}\n", mgr.stats.munmap_count));
    s.push_str(&format!("  Pages mapped:    {}\n", mgr.stats.pages_mapped));
    s.push_str(&format!(
        "  Pages unmapped:  {}\n",
        mgr.stats.pages_unmapped
    ));
    s.push_str(&format!("  Anon maps:       {}\n", mgr.stats.anon_maps));
    s.push_str(&format!("  File maps:       {}\n", mgr.stats.file_maps));
    s.push_str(&format!(
        "  Demand zero:     {}\n",
        mgr.stats.demand_zero_fills
    ));
    s.push_str(&format!(
        "  Demand file:     {}\n",
        mgr.stats.demand_file_reads
    ));
    s.push_str(&format!("  msync calls:     {}\n", mgr.stats.msync_count));

    // List active VMAs
    for i in 0..MAX_VMAS {
        if mgr.vmas[i].active {
            let v = &mgr.vmas[i];
            let type_str = match v.mtype {
                MmapType::Anonymous => alloc::string::String::from("anon"),
                MmapType::File { ino, file_offset } => {
                    format!("file(ino={},off={})", ino, file_offset)
                }
            };
            s.push_str(&format!(
                "  {:#x}-{:#x} {} prot={:#x} flags={:#x} pid={} res={}\n",
                v.start, v.end, type_str, v.prot, v.flags, v.pid, v.resident_pages
            ));
        }
    }
    s
}

/// Initialize mmap subsystem
pub fn init() {
    serial_println!(
        "  [mmap] memory mapping subsystem initialized (max {} VMAs)",
        MAX_VMAS
    );
}
