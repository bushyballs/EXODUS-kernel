use crate::serial_println;
/// shm --- shared memory subsystem for Genesis
///
/// Implements System V shared memory semantics (shmget/shmat/shmdt/shmctl).
/// Shared memory segments allow multiple processes to map the same physical
/// pages into their address spaces.
///
/// Architecture:
///   - Segment table: up to MAX_SEGMENTS shared memory segments
///   - Each segment has a unique key, owner PID, permissions, and backing pages
///   - Attach/detach tracks per-process mappings with reference counting
///   - Physical pages are allocated from the buddy allocator
///   - Segments persist until explicitly removed (IPC_RMID) or system shutdown
///
/// Inspired by: Linux SysV SHM (ipc/shm.c). All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum shared memory segments
const MAX_SEGMENTS: usize = 64;

/// Maximum pages per segment (256 MB / 4 KB = 65536 pages)
const MAX_SEGMENT_PAGES: usize = 65536;

/// Maximum attachments per segment
const MAX_ATTACHMENTS: usize = 32;

/// Page size
const PAGE_SIZE: usize = 4096;

/// IPC flags
pub const IPC_CREAT: u32 = 0x0200;
pub const IPC_EXCL: u32 = 0x0400;
pub const IPC_RMID: u32 = 0;
pub const SHM_RDONLY: u32 = 0x1000;

/// Permission bits (owner read/write, group read, other read)
pub const SHM_DEFAULT_MODE: u32 = 0o644;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Shared memory segment descriptor
pub struct ShmSegment {
    /// IPC key (user-provided, 0 = private)
    pub key: u32,
    /// Segment ID (index into table)
    pub id: usize,
    /// Size in bytes (rounded up to pages)
    pub size: usize,
    /// Number of pages
    pub num_pages: usize,
    /// Physical page addresses (allocated from buddy)
    pub pages: [usize; 256], // max 256 pages = 1 MB inline; larger uses metadata
    /// Large segment page tracking (for > 256 pages, stores count of pages allocated)
    pub large_page_count: usize,
    /// Owner PID
    pub owner_pid: u32,
    /// Creator PID
    pub creator_pid: u32,
    /// Permission mode (Unix-style)
    pub mode: u32,
    /// Number of current attachments
    pub attach_count: usize,
    /// Attachment records: (pid, virtual_addr)
    pub attachments: [(u32, usize); MAX_ATTACHMENTS],
    /// Whether this segment is active
    pub active: bool,
    /// Whether marked for deletion (IPC_RMID issued, will be freed when attach_count == 0)
    pub marked_for_delete: bool,
    /// Creation time (tick)
    pub ctime: u64,
    /// Last attach time
    pub atime: u64,
    /// Last detach time
    pub dtime: u64,
}

impl ShmSegment {
    const fn empty() -> Self {
        ShmSegment {
            key: 0,
            id: 0,
            size: 0,
            num_pages: 0,
            pages: [0usize; 256],
            large_page_count: 0,
            owner_pid: 0,
            creator_pid: 0,
            mode: 0,
            attach_count: 0,
            attachments: [(0u32, 0usize); MAX_ATTACHMENTS],
            active: false,
            marked_for_delete: false,
            ctime: 0,
            atime: 0,
            dtime: 0,
        }
    }
}

/// Shared memory statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct ShmStats {
    /// Total segments created
    pub segments_created: u64,
    /// Total segments destroyed
    pub segments_destroyed: u64,
    /// Total attach operations
    pub attaches: u64,
    /// Total detach operations
    pub detaches: u64,
    /// Pages currently in shared memory
    pub pages_in_use: u64,
    /// Peak pages in shared memory
    pub peak_pages: u64,
}

/// Shared memory manager
struct ShmManager {
    /// Segment table
    segments: [ShmSegment; MAX_SEGMENTS],
    /// Number of active segments
    segment_count: usize,
    /// Global tick counter
    tick: u64,
    /// Statistics
    stats: ShmStats,
}

impl ShmManager {
    const fn new() -> Self {
        const EMPTY: ShmSegment = ShmSegment::empty();
        ShmManager {
            segments: [EMPTY; MAX_SEGMENTS],
            segment_count: 0,
            tick: 0,
            stats: ShmStats {
                segments_created: 0,
                segments_destroyed: 0,
                attaches: 0,
                detaches: 0,
                pages_in_use: 0,
                peak_pages: 0,
            },
        }
    }
}

static SHM: Mutex<ShmManager> = Mutex::new(ShmManager::new());

/// Global counters
pub static SHM_TOTAL_PAGES: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create or get a shared memory segment (shmget semantics).
///
/// - `key`: IPC key (0 = IPC_PRIVATE, creates a new private segment)
/// - `size`: requested size in bytes (rounded up to page boundary)
/// - `flags`: IPC_CREAT, IPC_EXCL, permission mode
/// - `pid`: requesting process PID
///
/// Returns segment ID or None on failure.
pub fn shmget(key: u32, size: usize, flags: u32, pid: u32) -> Option<usize> {
    let mut mgr = SHM.lock();
    mgr.tick += 1;
    let tick = mgr.tick;

    // If key != 0, look for existing segment
    if key != 0 {
        for i in 0..MAX_SEGMENTS {
            if mgr.segments[i].active && mgr.segments[i].key == key {
                if flags & IPC_EXCL != 0 {
                    // IPC_EXCL + IPC_CREAT: fail if exists
                    return None;
                }
                // Check size compatibility
                if mgr.segments[i].size < size {
                    return None; // existing segment too small
                }
                return Some(mgr.segments[i].id);
            }
        }
    }

    // Need IPC_CREAT to create
    if key != 0 && flags & IPC_CREAT == 0 {
        return None;
    }

    // Find a free slot
    let slot = (0..MAX_SEGMENTS).find(|&i| !mgr.segments[i].active)?;

    // Calculate pages needed
    let num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    if num_pages == 0 || num_pages > MAX_SEGMENT_PAGES {
        return None;
    }

    // Allocate physical pages
    let max_inline = 256;
    let alloc_count = num_pages.min(max_inline);

    // Need to drop the lock to call buddy allocator
    drop(mgr);

    let mut page_addrs = [0usize; 256];
    let mut allocated = 0;
    for i in 0..alloc_count {
        if let Some(addr) = crate::memory::buddy::alloc_page() {
            page_addrs[i] = addr;
            // Zero the page
            unsafe {
                core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE);
            }
            allocated += 1;
        } else {
            // Allocation failed --- free what we got
            for j in 0..i {
                crate::memory::buddy::free_page(page_addrs[j]);
            }
            return None;
        }
    }

    let mut mgr = SHM.lock();
    // Re-check slot (might have been taken)
    if mgr.segments[slot].active {
        drop(mgr);
        for i in 0..allocated {
            crate::memory::buddy::free_page(page_addrs[i]);
        }
        return None;
    }

    let mode = flags & 0o777;
    mgr.segments[slot] = ShmSegment {
        key,
        id: slot,
        size,
        num_pages,
        pages: page_addrs,
        large_page_count: allocated,
        owner_pid: pid,
        creator_pid: pid,
        mode: if mode == 0 { SHM_DEFAULT_MODE } else { mode },
        attach_count: 0,
        attachments: [(0u32, 0usize); MAX_ATTACHMENTS],
        active: true,
        marked_for_delete: false,
        ctime: tick,
        atime: 0,
        dtime: 0,
    };
    mgr.segment_count += 1;
    mgr.stats.segments_created += 1;
    mgr.stats.pages_in_use += allocated as u64;
    if mgr.stats.pages_in_use > mgr.stats.peak_pages {
        mgr.stats.peak_pages = mgr.stats.pages_in_use;
    }
    SHM_TOTAL_PAGES.fetch_add(allocated as u64, Ordering::Relaxed);

    serial_println!(
        "  [shm] created segment {} (key={}, {} pages, owner={})",
        slot,
        key,
        num_pages,
        pid
    );
    Some(slot)
}

/// Attach a shared memory segment to a process's address space (shmat semantics).
///
/// - `shmid`: segment ID
/// - `virt_addr`: virtual address to map at (0 = kernel chooses)
/// - `flags`: SHM_RDONLY for read-only
/// - `pid`: attaching process PID
///
/// Returns the virtual address where the segment was attached, or None.
pub fn shmat(shmid: usize, virt_addr: usize, flags: u32, pid: u32) -> Option<usize> {
    let mut mgr = SHM.lock();
    if shmid >= MAX_SEGMENTS || !mgr.segments[shmid].active {
        return None;
    }
    if mgr.segments[shmid].marked_for_delete {
        return None;
    }

    let seg = &mut mgr.segments[shmid];
    if seg.attach_count >= MAX_ATTACHMENTS {
        return None;
    }

    // Choose virtual address if not specified
    // In a real kernel, we'd consult the process's VM map
    let map_addr = if virt_addr == 0 {
        // Simple heuristic: 0x1_0000_0000 + shmid * 16MB
        0x1_0000_0000 + shmid * 16 * 1024 * 1024
    } else {
        virt_addr
    };

    let num_pages = seg.num_pages.min(256);
    let read_only = flags & SHM_RDONLY != 0;

    // Map pages into the process address space
    // We use the current page tables (simplified)
    let page_flags = if read_only {
        crate::memory::paging::flags::USER_RO
    } else {
        crate::memory::paging::flags::USER_RW
    };

    // Store pages before we drop lock for mapping
    let mut page_addrs = [0usize; 256];
    for i in 0..num_pages {
        page_addrs[i] = seg.pages[i];
    }

    // Record attachment
    let attach_idx = seg.attach_count;
    seg.attachments[attach_idx] = (pid, map_addr);
    seg.attach_count += 1;
    mgr.tick += 1;
    let tick = mgr.tick;
    mgr.segments[shmid].atime = tick;
    mgr.stats.attaches += 1;
    drop(mgr);

    // Map physical pages to virtual addresses
    for i in 0..num_pages {
        let v = map_addr + i * PAGE_SIZE;
        let p = page_addrs[i];
        if p != 0 {
            let _ = crate::memory::paging::map_page(v, p, page_flags);
            // Increment refcount since multiple processes share this page
            crate::memory::frame_allocator::inc_refcount(
                crate::memory::frame_allocator::PhysFrame::from_addr(p),
            );
        }
    }

    serial_println!(
        "  [shm] attached segment {} at {:#x} for pid {} ({} pages)",
        shmid,
        map_addr,
        pid,
        num_pages
    );
    Some(map_addr)
}

/// Detach a shared memory segment from a process (shmdt semantics).
///
/// - `shmid`: segment ID
/// - `pid`: detaching process PID
pub fn shmdt(shmid: usize, pid: u32) -> bool {
    let mut mgr = SHM.lock();
    if shmid >= MAX_SEGMENTS || !mgr.segments[shmid].active {
        return false;
    }

    // Find the attachment for this pid
    let mut found = false;
    let mut map_addr = 0;
    let mut found_idx = 0;
    let attach_count_before = mgr.segments[shmid].attach_count;
    for i in 0..attach_count_before {
        if mgr.segments[shmid].attachments[i].0 == pid {
            map_addr = mgr.segments[shmid].attachments[i].1;
            found_idx = i;
            found = true;
            break;
        }
    }

    if !found {
        return false;
    }

    let num_pages = mgr.segments[shmid].num_pages.min(256);

    // Remove attachment (swap with last)
    let last_idx = mgr.segments[shmid].attach_count - 1;
    let last_attachment = mgr.segments[shmid].attachments[last_idx];
    mgr.segments[shmid].attachments[found_idx] = last_attachment;
    mgr.segments[shmid].attachments[last_idx] = (0, 0);
    mgr.segments[shmid].attach_count -= 1;
    mgr.tick += 1;
    let tick = mgr.tick;
    mgr.segments[shmid].dtime = tick;
    mgr.stats.detaches += 1;

    let marked_for_delete = mgr.segments[shmid].marked_for_delete;
    let attach_count = mgr.segments[shmid].attach_count;
    drop(mgr);

    // Unmap pages (but don't free physical pages --- other processes may have them)
    for i in 0..num_pages {
        let v = map_addr + i * PAGE_SIZE;
        crate::memory::paging::unmap_page(v);
    }

    serial_println!("  [shm] detached segment {} from pid {}", shmid, pid);

    // If marked for delete and no more attachments, destroy
    if marked_for_delete && attach_count == 0 {
        destroy_segment(shmid);
    }

    true
}

/// Control a shared memory segment (shmctl semantics).
///
/// Currently supports IPC_RMID (mark for deletion).
pub fn shmctl(shmid: usize, cmd: u32, _pid: u32) -> bool {
    match cmd {
        0 => {
            // IPC_RMID
            let mut mgr = SHM.lock();
            if shmid >= MAX_SEGMENTS || !mgr.segments[shmid].active {
                return false;
            }
            mgr.segments[shmid].marked_for_delete = true;
            let attach_count = mgr.segments[shmid].attach_count;
            drop(mgr);

            if attach_count == 0 {
                destroy_segment(shmid);
            } else {
                serial_println!(
                    "  [shm] segment {} marked for deletion ({} attachments remain)",
                    shmid,
                    attach_count
                );
            }
            true
        }
        _ => false,
    }
}

/// Destroy a segment: free all physical pages and clear the slot
fn destroy_segment(shmid: usize) {
    let mut mgr = SHM.lock();
    if shmid >= MAX_SEGMENTS || !mgr.segments[shmid].active {
        return;
    }

    let num_pages = mgr.segments[shmid].num_pages.min(256);
    let mut pages_to_free = [0usize; 256];
    for i in 0..num_pages {
        pages_to_free[i] = mgr.segments[shmid].pages[i];
    }

    mgr.segments[shmid] = ShmSegment::empty();
    mgr.segment_count -= 1;
    mgr.stats.segments_destroyed += 1;
    let freed = num_pages as u64;
    mgr.stats.pages_in_use = mgr.stats.pages_in_use.saturating_sub(freed);
    drop(mgr);

    SHM_TOTAL_PAGES.fetch_sub(
        freed.min(SHM_TOTAL_PAGES.load(Ordering::Relaxed)),
        Ordering::Relaxed,
    );

    // Free physical pages
    for i in 0..num_pages {
        if pages_to_free[i] != 0 {
            crate::memory::buddy::free_page(pages_to_free[i]);
        }
    }

    serial_println!(
        "  [shm] destroyed segment {} ({} pages freed)",
        shmid,
        num_pages
    );
}

/// Get segment information
pub fn segment_info(shmid: usize) -> Option<(u32, usize, usize, u32, usize)> {
    let mgr = SHM.lock();
    if shmid >= MAX_SEGMENTS || !mgr.segments[shmid].active {
        return None;
    }
    let s = &mgr.segments[shmid];
    Some((s.key, s.size, s.num_pages, s.owner_pid, s.attach_count))
}

/// Get statistics
pub fn stats() -> ShmStats {
    SHM.lock().stats
}

/// Get a summary string
pub fn summary() -> alloc::string::String {
    use alloc::format;
    let mgr = SHM.lock();
    let mut s = alloc::string::String::from("Shared Memory:\n");
    s.push_str(&format!("  Active segments: {}\n", mgr.segment_count));
    s.push_str(&format!("  Pages in use:    {}\n", mgr.stats.pages_in_use));
    s.push_str(&format!("  Peak pages:      {}\n", mgr.stats.peak_pages));
    s.push_str(&format!(
        "  Created:         {}\n",
        mgr.stats.segments_created
    ));
    s.push_str(&format!(
        "  Destroyed:       {}\n",
        mgr.stats.segments_destroyed
    ));
    s.push_str(&format!("  Attaches:        {}\n", mgr.stats.attaches));
    s.push_str(&format!("  Detaches:        {}\n", mgr.stats.detaches));

    for i in 0..MAX_SEGMENTS {
        if mgr.segments[i].active {
            let seg = &mgr.segments[i];
            s.push_str(&format!(
                "  [{}] key={} size={} pages={} owner={} attaches={}{}\n",
                i,
                seg.key,
                seg.size,
                seg.num_pages,
                seg.owner_pid,
                seg.attach_count,
                if seg.marked_for_delete { " (RMID)" } else { "" }
            ));
        }
    }
    s
}

/// Initialize shared memory subsystem
pub fn init() {
    serial_println!(
        "  [shm] shared memory subsystem initialized (max {} segments)",
        MAX_SEGMENTS
    );
}
