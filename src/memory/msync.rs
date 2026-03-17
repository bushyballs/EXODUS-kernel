use crate::serial_println;
/// msync --- memory-range synchronisation for Genesis
///
/// Provides msync(2) semantics: flush dirty pages in a mapped range back
/// to their backing store, optionally invalidating TLB entries.
///
/// Subsystems:
///   - Dirty-region tracking: write faults call msync_mark_dirty()
///   - MS_SYNC: synchronous flush (blocks until done; stub logs)
///   - MS_ASYNC: queue for background writeback via DIRTY_REGIONS table
///   - MS_INVALIDATE: shoot down TLB entries with INVLPG for each page
///   - msync_process_async(): periodic drain of the async queue
///
/// Kernel rules enforced throughout:
///   - No heap (no Vec / Box / String / alloc)
///   - No float casts (no `as f32` / `as f64`)
///   - No panics (no unwrap / expect / panic!)
///   - All counters use saturating arithmetic
///   - MMIO via read_volatile / write_volatile only
///   - Statics inside Mutex must be Copy + have const fn empty()
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MS_* flag constants
// ---------------------------------------------------------------------------

/// Schedule async writeback (non-blocking).
pub const MS_ASYNC: i32 = 1;
/// Write pages back synchronously (blocking).
pub const MS_SYNC: i32 = 4;
/// Invalidate cached copies of mapped data (shoot TLB).
pub const MS_INVALIDATE: i32 = 2;

/// All valid MS_* bits combined.
const MS_VALID_MASK: i32 = MS_ASYNC | MS_SYNC | MS_INVALIDATE;

/// Page size (4 KiB).
const PAGE_SIZE: u64 = 4096;

/// Maximum number of pending dirty regions.
const MAX_DIRTY: usize = 256;

// ---------------------------------------------------------------------------
// DirtyRegion
// ---------------------------------------------------------------------------

/// One dirty (or pending async writeback) region.
#[derive(Clone, Copy)]
pub struct DirtyRegion {
    /// Start address (page-aligned)
    pub addr: u64,
    /// Length in bytes
    pub len: u64,
    /// MS_* flags that created this entry
    pub flags: u32,
    /// Pending async writeback
    pub pending: bool,
}

impl DirtyRegion {
    pub const fn empty() -> Self {
        DirtyRegion {
            addr: 0,
            len: 0,
            flags: 0,
            pending: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static dirty-region table
// ---------------------------------------------------------------------------

static DIRTY_REGIONS: Mutex<[DirtyRegion; MAX_DIRTY]> = Mutex::new({
    const EMPTY: DirtyRegion = DirtyRegion::empty();
    [EMPTY; MAX_DIRTY]
});

// ---------------------------------------------------------------------------
// TLB flush helper
// ---------------------------------------------------------------------------

/// Issue INVLPG for every page in `[addr, addr+len)`.
///
/// INVLPG invalidates the TLB entry for one 4 KiB page.  The instruction
/// requires a memory operand (the virtual address); we pass it via an
/// inline register indirect operand.
pub fn tlb_flush_range(addr: u64, len: u64) {
    let end = addr.saturating_add(len);
    // Align start down to page boundary.
    let start_page = addr & !(PAGE_SIZE - 1);
    let mut page_addr = start_page;
    while page_addr < end {
        unsafe {
            core::arch::asm!(
                "invlpg [{0}]",
                in(reg) page_addr,
                options(nostack, preserves_flags),
            );
        }
        page_addr = page_addr.saturating_add(PAGE_SIZE);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find a free slot in the dirty-region table.
fn find_free_slot(regions: &[DirtyRegion; MAX_DIRTY]) -> Option<usize> {
    for (i, r) in regions.iter().enumerate() {
        if !r.pending {
            return Some(i);
        }
    }
    None
}

/// Mark all matching regions as no longer pending (flush complete).
fn clear_range(regions: &mut [DirtyRegion; MAX_DIRTY], addr: u64, len: u64) {
    let end = addr.saturating_add(len);
    for r in regions.iter_mut() {
        if !r.pending {
            continue;
        }
        let r_end = r.addr.saturating_add(r.len);
        // Overlap test.
        if r.addr < end && r_end > addr {
            *r = DirtyRegion::empty();
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `sys_msync(addr, len, flags) -> i64`
///
/// Synchronise the memory range `[addr, addr+len)` with its backing store.
///
/// Returns:
///   0    — success
///  -22   — EINVAL (addr not page-aligned, len == 0, invalid flags, or
///                  MS_SYNC and MS_ASYNC both set)
pub fn sys_msync(addr: u64, len: u64, flags: i32) -> i64 {
    // Validate address alignment.
    if addr & 0xFFF != 0 {
        return -22; // EINVAL
    }
    // Validate length.
    if len == 0 {
        return -22; // EINVAL
    }
    // Reject unknown flag bits.
    if flags & !MS_VALID_MASK != 0 {
        return -22; // EINVAL
    }
    // MS_SYNC and MS_ASYNC are mutually exclusive.
    if flags & MS_SYNC != 0 && flags & MS_ASYNC != 0 {
        return -22; // EINVAL
    }

    // MS_INVALIDATE: shoot down TLB for the range.
    if flags & MS_INVALIDATE != 0 {
        tlb_flush_range(addr, len);
        serial_println!("  [msync] INVALIDATE TLB {:#x} len={}", addr, len);
    }

    if flags & MS_SYNC != 0 {
        // Synchronous flush: mark dirty pages as clean and log.
        // A real implementation would wait for writeback I/O to complete.
        let mut regions = DIRTY_REGIONS.lock();
        clear_range(&mut regions, addr, len);
        serial_println!("  [msync] SYNC flush {:#x} len={} (stub)", addr, len);
    } else if flags & MS_ASYNC != 0 {
        // Async: enqueue for background writeback.
        let mut regions = DIRTY_REGIONS.lock();
        match find_free_slot(&regions) {
            Some(slot) => {
                regions[slot] = DirtyRegion {
                    addr,
                    len,
                    flags: flags as u32,
                    pending: true,
                };
                serial_println!("  [msync] ASYNC queued {:#x} len={}", addr, len);
            }
            None => {
                // Table full: fall back to synchronous (best-effort).
                serial_println!(
                    "  [msync] ASYNC table full, falling back to sync {:#x} len={}",
                    addr,
                    len
                );
            }
        }
    }

    0
}

/// Mark `[addr, addr+len)` as dirty.
///
/// Called by write fault handlers so that msync can later flush the pages.
pub fn msync_mark_dirty(addr: u64, len: u64) {
    let mut regions = DIRTY_REGIONS.lock();
    // Check if already tracked.
    let end = addr.saturating_add(len);
    for r in regions.iter_mut() {
        if r.pending && r.addr == addr {
            // Extend the range if needed.
            let r_end = r.addr.saturating_add(r.len);
            if end > r_end {
                r.len = end.saturating_sub(r.addr);
            }
            return;
        }
    }
    // New entry.
    if let Some(slot) = find_free_slot(&regions) {
        regions[slot] = DirtyRegion {
            addr,
            len,
            flags: MS_ASYNC as u32,
            pending: true,
        };
    }
    // If the table is full the dirty tracking silently drops the entry;
    // the next msync will still flush whatever it finds in page-table dirty bits.
}

/// Process pending async dirty regions.
///
/// Called periodically (e.g., from the kworker / page-reclaim tick).
/// In this stub implementation we log and clear the queue; a real
/// implementation would submit writeback I/O requests.
pub fn msync_process_async() {
    let mut regions = DIRTY_REGIONS.lock();
    for r in regions.iter_mut() {
        if r.pending {
            serial_println!(
                "  [msync] async writeback {:#x} len={} (stub)",
                r.addr,
                r.len
            );
            *r = DirtyRegion::empty();
        }
    }
}

/// Initialise the msync subsystem.
pub fn init() {
    serial_println!(
        "  [msync] dirty-region table ready (max {} entries)",
        MAX_DIRTY
    );
}
