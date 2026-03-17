use crate::process;
use crate::serial_println;
/// mlock --- memory locking syscalls for Genesis
///
/// Prevents locked pages from being swapped out. Maintains a table of
/// locked regions per process; checks the region table before allowing
/// the reclaim/swap path to evict a page.
///
/// Kernel rules enforced throughout:
///   - No heap (no Vec / Box / String / alloc)
///   - No float casts (no `as f32` / `as f64`)
///   - No panics (no unwrap / expect / panic!)
///   - All counters use saturating arithmetic
///   - Statics inside Mutex must be Copy + have const fn empty()
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// mlock2 flag: fault and lock pages only as they are accessed.
pub const MLOCK_ONFAULT: u32 = 1;

/// mlockall flags
pub const MCL_CURRENT: u32 = 1;
pub const MCL_FUTURE: u32 = 2;
pub const MCL_ONFAULT: u32 = 4;

/// Maximum locked regions in the global table.
const MAX_REGIONS: usize = 128;

/// Maximum size of a single mlock request (4 MiB).
const MLOCK_MAX_BYTES: u64 = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// LockedRegion
// ---------------------------------------------------------------------------

/// Describes one locked memory region belonging to a process.
#[derive(Clone, Copy)]
pub struct LockedRegion {
    /// Owning process PID
    pub pid: u32,
    /// Start address (page-aligned)
    pub addr: u64,
    /// Length in bytes
    pub len: u64,
    /// MLOCK_* flags
    pub flags: u32,
    /// Slot in use
    pub active: bool,
}

impl LockedRegion {
    pub const fn empty() -> Self {
        LockedRegion {
            pid: 0,
            addr: 0,
            len: 0,
            flags: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static region table
// ---------------------------------------------------------------------------

static LOCKED_REGIONS: Mutex<[LockedRegion; MAX_REGIONS]> = Mutex::new({
    const EMPTY: LockedRegion = LockedRegion::empty();
    [EMPTY; MAX_REGIONS]
});

/// Global mlockall intent flags (indexed by PID, simplified: one global flag).
/// A real implementation would track per-process state in the PCB.
static MLOCKALL_FLAGS: Mutex<u32> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the first free slot index in the region table.
fn find_free_slot(regions: &[LockedRegion; MAX_REGIONS]) -> Option<usize> {
    for (i, r) in regions.iter().enumerate() {
        if !r.active {
            return Some(i);
        }
    }
    None
}

/// Count active regions and compute total locked bytes.
fn count_active(regions: &[LockedRegion; MAX_REGIONS]) -> (u32, u64) {
    let mut count: u32 = 0;
    let mut total: u64 = 0;
    for r in regions.iter() {
        if r.active {
            count = count.saturating_add(1);
            total = total.saturating_add(r.len);
        }
    }
    (count, total)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `sys_mlock(addr, len) -> i64`
///
/// Lock all pages in `[addr, addr+len)` into physical memory.
///
/// Returns:
///   0    — success
///  -22   — EINVAL (addr not page-aligned, len == 0 or len > 4 MiB)
///  -12   — ENOMEM (region table full)
pub fn sys_mlock(addr: u64, len: u64) -> i64 {
    if addr & 0xFFF != 0 {
        return -22; // EINVAL
    }
    if len == 0 || len > MLOCK_MAX_BYTES {
        return -22; // EINVAL
    }

    let pid = process::getpid();
    let mut regions = LOCKED_REGIONS.lock();

    match find_free_slot(&regions) {
        None => {
            serial_println!("  [mlock] region table full — ENOMEM (pid={})", pid);
            -12 // ENOMEM
        }
        Some(slot) => {
            regions[slot] = LockedRegion {
                pid,
                addr,
                len,
                flags: 0,
                active: true,
            };
            serial_println!("  [mlock] locked {:#x} len={} pid={}", addr, len, pid);
            0
        }
    }
}

/// `sys_munlock(addr, len) -> i64`
///
/// Unlock pages in `[addr, addr+len)`.  Removes matching entries from the
/// region table.
///
/// Returns 0 (success, idempotent even if no matching entry exists).
pub fn sys_munlock(addr: u64, len: u64) -> i64 {
    let pid = process::getpid();
    let mut regions = LOCKED_REGIONS.lock();
    for r in regions.iter_mut() {
        if r.active && r.pid == pid && r.addr == addr && r.len == len {
            *r = LockedRegion::empty();
            serial_println!("  [mlock] unlocked {:#x} len={} pid={}", addr, len, pid);
            break;
        }
    }
    0
}

/// `sys_mlockall(flags) -> i64`
///
/// Record the intent to lock all current and/or future mappings.
///
/// Flags:
///   MCL_CURRENT  (1) — lock all currently mapped pages
///   MCL_FUTURE   (2) — lock all future mappings
///   MCL_ONFAULT  (4) — lock on fault rather than immediately
///
/// Returns 0 on success, -22 on invalid flags.
pub fn sys_mlockall(flags: u32) -> i64 {
    let valid = MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT;
    if flags & !valid != 0 {
        return -22; // EINVAL
    }
    let mut f = MLOCKALL_FLAGS.lock();
    *f = flags;
    serial_println!("  [mlock] mlockall flags={:#x}", flags);
    0
}

/// `sys_munlockall() -> i64`
///
/// Remove all locked regions belonging to the calling process and clear
/// the global mlockall intent.
///
/// Returns 0.
pub fn sys_munlockall() -> i64 {
    let pid = process::getpid();
    let mut regions = LOCKED_REGIONS.lock();
    for r in regions.iter_mut() {
        if r.active && r.pid == pid {
            *r = LockedRegion::empty();
        }
    }
    let mut f = MLOCKALL_FLAGS.lock();
    *f = 0;
    serial_println!("  [mlock] munlockall pid={}", pid);
    0
}

/// `sys_mlock2(addr, len, flags) -> i64`
///
/// Like `sys_mlock` but accepts `MLOCK_ONFAULT` flag which defers page
/// locking until the page is first accessed.
///
/// Returns 0 on success, -22 on invalid arguments, -12 if table full.
pub fn sys_mlock2(addr: u64, len: u64, flags: u32) -> i64 {
    if addr & 0xFFF != 0 {
        return -22; // EINVAL
    }
    if len == 0 || len > MLOCK_MAX_BYTES {
        return -22; // EINVAL
    }
    // Only MLOCK_ONFAULT is defined; reject unknown flag bits.
    if flags & !MLOCK_ONFAULT != 0 {
        return -22; // EINVAL
    }

    let pid = process::getpid();
    let mut regions = LOCKED_REGIONS.lock();

    match find_free_slot(&regions) {
        None => {
            serial_println!("  [mlock2] region table full — ENOMEM (pid={})", pid);
            -12 // ENOMEM
        }
        Some(slot) => {
            regions[slot] = LockedRegion {
                pid,
                addr,
                len,
                flags,
                active: true,
            };
            serial_println!(
                "  [mlock2] locked {:#x} len={} flags={:#x} pid={}",
                addr,
                len,
                flags,
                pid
            );
            0
        }
    }
}

/// Return `true` if `addr` falls within any locked region.
pub fn is_region_locked(addr: u64) -> bool {
    let regions = LOCKED_REGIONS.lock();
    for r in regions.iter() {
        if r.active && addr >= r.addr && addr < r.addr.saturating_add(r.len) {
            return true;
        }
    }
    false
}

/// Return `(locked_region_count, total_locked_bytes)`.
pub fn mlock_get_stats() -> (u32, u64) {
    let regions = LOCKED_REGIONS.lock();
    count_active(&regions)
}

/// Initialise the mlock subsystem.
pub fn init() {
    serial_println!("  [mlock] region table ready (max {} entries)", MAX_REGIONS);
}
