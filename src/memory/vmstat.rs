use crate::serial_println;
/// vmstat --- detailed VM statistics for Genesis (no-heap, no-float, no-panic)
///
/// Provides page-level counters equivalent to Linux /proc/vmstat and
/// /proc/meminfo.  All counters live in a single Mutex-guarded array of
/// u64 values indexed by the NR_* / PG* constants defined here.
///
/// Rules enforced throughout:
///   - No Vec, Box, String, or any alloc::* usage
///   - No float casts (as f32 / as f64)
///   - No unwrap(), expect(), or panic!()
///   - Counter additions: saturating_add
///   - Counter subtractions: saturating_sub
///
/// Inspired by Linux vmstat (mm/vmstat.c). All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Counter index constants
// ---------------------------------------------------------------------------

/// Number of free pages
pub const NR_FREE_PAGES: usize = 0;
/// Anonymous pages in use
pub const NR_ANON_PAGES: usize = 1;
/// File-backed pages in use
pub const NR_FILE_PAGES: usize = 2;
/// Reclaimable slab pages
pub const NR_SLAB_RECLAIMABLE: usize = 3;
/// Unreclaimable slab pages
pub const NR_SLAB_UNRECLAIMABLE: usize = 4;
/// Pages used for page tables
pub const NR_PAGE_TABLE_PAGES: usize = 5;
/// Dirty pages (modified, not yet written back)
pub const NR_DIRTY: usize = 6;
/// Pages under writeback
pub const NR_WRITEBACK: usize = 7;
/// Pages mapped into process address spaces
pub const NR_MAPPED: usize = 8;
/// Pages in mmap regions
pub const NR_MMAP_PAGES: usize = 9;
/// Active anonymous pages
pub const NR_ACTIVE_ANON: usize = 10;
/// Inactive anonymous pages
pub const NR_INACTIVE_ANON: usize = 11;
/// Active file-backed pages
pub const NR_ACTIVE_FILE: usize = 12;
/// Inactive file-backed pages
pub const NR_INACTIVE_FILE: usize = 13;
/// Minor page faults
pub const PGFAULT: usize = 14;
/// Major page faults (page-in from swap / disk)
pub const PGMAJFAULT: usize = 15;
/// Total page allocations
pub const PGALLOC: usize = 16;
/// Total page frees
pub const PGFREE: usize = 17;
/// Pages read in (paged in from disk)
pub const PGPGIN: usize = 18;
/// Pages written out (paged out to disk)
pub const PGPGOUT: usize = 19;
/// Swap pages read in
pub const PSWPIN: usize = 20;
/// Swap pages written out
pub const PSWPOUT: usize = 21;

/// Total number of tracked vmstat items
pub const NR_VMSTAT_ITEMS: usize = 22;

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// All vmstat counters in one fixed-size array protected by a single Mutex.
///
/// We store raw `u64` values rather than `AtomicU64` because `AtomicU64` is
/// not `Copy`, which prevents it from being used directly in const-initialised
/// static arrays in no_std.  The Mutex provides the necessary mutual exclusion.
static VMSTAT_COUNTS: Mutex<[u64; NR_VMSTAT_ITEMS]> = Mutex::new([0u64; NR_VMSTAT_ITEMS]);

// ---------------------------------------------------------------------------
// Integer-to-ASCII helper (no format!, no alloc)
// ---------------------------------------------------------------------------

/// Write the decimal representation of `val` into `buf` starting at `pos`.
///
/// Returns the new position (one past the last digit written).
/// Does not write a NUL terminator.
/// If the buffer would overflow, the value is truncated (safety first).
fn write_u64(buf: &mut [u8; 4096], pos: usize, mut val: u64) -> usize {
    // Special-case zero
    if val == 0 {
        if pos < buf.len() {
            buf[pos] = b'0';
            return pos.saturating_add(1);
        }
        return pos;
    }

    // Determine number of digits (max 20 for u64)
    let mut tmp = [0u8; 20];
    let mut n: usize = 0;
    while val > 0 && n < 20 {
        tmp[n] = b'0' + (val % 10) as u8;
        val /= 10;
        n = n.saturating_add(1);
    }

    // Write digits in reverse (most-significant first)
    let mut p = pos;
    let mut i = n;
    while i > 0 {
        i = i.saturating_sub(1);
        if p < buf.len() {
            buf[p] = tmp[i];
            p = p.saturating_add(1);
        }
    }
    p
}

/// Write a string slice into `buf` starting at `pos`.
///
/// Returns the new position. Truncates silently if the buffer is full.
fn write_str(buf: &mut [u8; 4096], pos: usize, s: &[u8]) -> usize {
    let mut p = pos;
    for &b in s {
        if p < buf.len() {
            buf[p] = b;
            p = p.saturating_add(1);
        }
    }
    p
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add `delta` to counter `item` (saturating).
///
/// Silently does nothing if `item >= NR_VMSTAT_ITEMS`.
pub fn vmstat_add(item: usize, delta: u64) {
    if item >= NR_VMSTAT_ITEMS {
        return;
    }
    let mut counts = VMSTAT_COUNTS.lock();
    counts[item] = counts[item].saturating_add(delta);
}

/// Subtract `delta` from counter `item` (saturating, floors at 0).
///
/// Silently does nothing if `item >= NR_VMSTAT_ITEMS`.
pub fn vmstat_sub(item: usize, delta: u64) {
    if item >= NR_VMSTAT_ITEMS {
        return;
    }
    let mut counts = VMSTAT_COUNTS.lock();
    counts[item] = counts[item].saturating_sub(delta);
}

/// Set counter `item` to `val` directly.
///
/// Silently does nothing if `item >= NR_VMSTAT_ITEMS`.
pub fn vmstat_set(item: usize, val: u64) {
    if item >= NR_VMSTAT_ITEMS {
        return;
    }
    let mut counts = VMSTAT_COUNTS.lock();
    counts[item] = val;
}

/// Read the current value of counter `item`.
///
/// Returns 0 if `item >= NR_VMSTAT_ITEMS`.
pub fn vmstat_get(item: usize) -> u64 {
    if item >= NR_VMSTAT_ITEMS {
        return 0;
    }
    let counts = VMSTAT_COUNTS.lock();
    counts[item]
}

/// Copy all counters into the caller-supplied array.
pub fn vmstat_snapshot(out: &mut [u64; NR_VMSTAT_ITEMS]) {
    let counts = VMSTAT_COUNTS.lock();
    for i in 0..NR_VMSTAT_ITEMS {
        out[i] = counts[i];
    }
}

/// Write a /proc/meminfo-style report into `buf`.
///
/// All values are expressed in kB (1 kB = 1024 bytes = 1/4 page).
/// Uses only integer arithmetic and the in-module write_u64/write_str
/// helpers — no format!, no alloc.
///
/// Returns the number of bytes written.
pub fn vmstat_format_meminfo(buf: &mut [u8; 4096]) -> usize {
    // Take a snapshot under the lock so we present a consistent view.
    let mut snap = [0u64; NR_VMSTAT_ITEMS];
    vmstat_snapshot(&mut snap);

    // Convert page counts to kB (1 page = 4 kB)
    let to_kb = |pages: u64| -> u64 { pages.saturating_mul(4) };

    // Derive MemTotal and MemFree from the frame allocator for accuracy.
    let total_kb: u64 = (crate::memory::frame_allocator::MAX_MEMORY / 1024) as u64;
    let free_kb: u64 = to_kb(snap[NR_FREE_PAGES]);

    // MemAvailable: free + reclaimable slab (simplified)
    let available_kb: u64 = free_kb.saturating_add(to_kb(snap[NR_SLAB_RECLAIMABLE]));

    // Buffers: file pages not in active/inactive lists (approximation)
    let active_file_kb = to_kb(snap[NR_ACTIVE_FILE]);
    let inactive_file_kb = to_kb(snap[NR_INACTIVE_FILE]);
    let cached_kb: u64 = to_kb(snap[NR_FILE_PAGES]);
    let buffers_kb: u64 = cached_kb
        .saturating_sub(active_file_kb)
        .saturating_sub(inactive_file_kb);

    let active_kb = to_kb(snap[NR_ACTIVE_ANON].saturating_add(snap[NR_ACTIVE_FILE]));
    let inactive_kb = to_kb(snap[NR_INACTIVE_ANON].saturating_add(snap[NR_INACTIVE_FILE]));
    let pagetables_kb = to_kb(snap[NR_PAGE_TABLE_PAGES]);
    let mapped_kb = to_kb(snap[NR_MAPPED]);
    let slab_kb: u64 = to_kb(snap[NR_SLAB_RECLAIMABLE].saturating_add(snap[NR_SLAB_UNRECLAIMABLE]));

    let mut pos: usize = 0;

    // Helper macro-style closure (avoids macro syntax complexity in no_std)
    // Each line: "LabelXXXX:    <value> kB\n"
    let fields: [(&[u8], u64); 10] = [
        (b"MemTotal:          ", total_kb),
        (b"MemFree:           ", free_kb),
        (b"MemAvailable:      ", available_kb),
        (b"Buffers:           ", buffers_kb),
        (b"Cached:            ", cached_kb),
        (b"Active:            ", active_kb),
        (b"Inactive:          ", inactive_kb),
        (b"PageTables:        ", pagetables_kb),
        (b"Mapped:            ", mapped_kb),
        (b"Slab:              ", slab_kb),
    ];

    for &(label, val) in &fields {
        pos = write_str(buf, pos, label);
        pos = write_u64(buf, pos, val);
        pos = write_str(buf, pos, b" kB\n");
    }

    pos
}

/// Refresh `NR_FREE_PAGES` from the live frame allocator count.
///
/// Should be called periodically (e.g., from a scheduler tick or on every
/// significant allocation/free event).
pub fn vmstat_tick() {
    let free_pages: u64 = {
        let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
        fa.free_count() as u64
    };
    vmstat_set(NR_FREE_PAGES, free_pages);
}

/// Initialize the vmstat subsystem.
///
/// Counters are already zero-initialised by the static. This function
/// seeds `NR_FREE_PAGES` from the current frame-allocator state and prints
/// a boot banner.
pub fn init() {
    // Seed NR_FREE_PAGES from stats module (avoids re-importing frame alloc
    // directly here for the initial value; also validates stats::free_kb()).
    let free_kb = crate::memory::stats::free_kb();
    // Convert kB -> pages (1 page = 4 kB).
    let free_pages = (free_kb / 4) as u64;
    vmstat_set(NR_FREE_PAGES, free_pages);

    serial_println!("  [vmstat] VM statistics initialized");
}
