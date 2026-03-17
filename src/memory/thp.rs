use crate::serial_println;
/// Transparent Huge Page (THP) stubs for Genesis AIOS
///
/// Provides the policy interface and statistics tracking for 2 MB huge pages.
/// Actual 2 MB allocations require an order-9 buddy block; the allocator for
/// that size is not yet wired, so allocation functions return `None` / `false`
/// and log their intent.
///
/// Design rules enforced throughout this module:
///   - NO heap: no Vec, Box, String, alloc::*
///   - NO panics: no unwrap(), expect(), panic!()
///   - NO float casts: no `as f64` / `as f32`
///   - All counters use saturating_add / saturating_sub
///   - All statics: Copy + const-constructible
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Size of a 2 MB huge page in bytes.
pub const HUGEPAGE_SIZE: u64 = 2 * 1024 * 1024;

/// Number of bits to shift to convert a byte address to a 2 MB page index.
pub const HUGEPAGE_SHIFT: u32 = 21;

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// System-wide THP allocation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThpMode {
    /// Always try to allocate huge pages for eligible regions.
    Always,
    /// Only use huge pages when the process explicitly requests them via
    /// madvise(MADV_HUGEPAGE).
    Madvise,
    /// Never use huge pages.
    Never,
}

struct ThpState {
    mode: ThpMode,
    /// Lifetime count of huge pages that have been allocated (best-effort;
    /// not decremented on free because we have no real allocator yet).
    hugepages_allocated: u64,
    /// Lifetime count of promotion attempts (successful or not).
    promotions_attempted: u64,
}

impl ThpState {
    const fn new() -> Self {
        ThpState {
            mode: ThpMode::Never,
            hugepages_allocated: 0,
            promotions_attempted: 0,
        }
    }
}

static THP_STATE: Mutex<ThpState> = Mutex::new(ThpState::new());

/// Atomic counter used to track khugepaged scan rounds.
static SCAN_ROUNDS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Policy accessors
// ---------------------------------------------------------------------------

/// Set the system-wide THP mode.
pub fn thp_set_mode(mode: ThpMode) {
    THP_STATE.lock().mode = mode;
    serial_println!("  [thp] mode set to {:?}", mode);
}

/// Get the current system-wide THP mode.
pub fn thp_get_mode() -> ThpMode {
    THP_STATE.lock().mode
}

/// Return `true` when THP is active (mode is `Always` or `Madvise`).
pub fn thp_enabled() -> bool {
    let mode = THP_STATE.lock().mode;
    mode == ThpMode::Always || mode == ThpMode::Madvise
}

// ---------------------------------------------------------------------------
// Allocation stubs
// ---------------------------------------------------------------------------

/// Attempt to allocate a 2 MB huge page at the given virtual address.
///
/// Stub: logs the attempt and returns `None` because a 2 MB-aligned buddy
/// allocator is not yet implemented.
pub fn thp_alloc_hugepage(addr: u64) -> Option<u64> {
    serial_println!(
        "  [thp] thp_alloc_hugepage addr={:#x} — stub, no 2MB allocator yet",
        addr
    );
    None
}

/// Attempt to split a 2 MB huge page at the given address back into 4 KB
/// pages.
///
/// Stub: returns `false` because no huge pages have been allocated.
pub fn thp_split_hugepage(addr: u64) -> bool {
    serial_println!(
        "  [thp] thp_split_hugepage addr={:#x} — stub, nothing to split",
        addr
    );
    false
}

/// Try to promote the 2 MB-aligned region containing `addr` from 4 KB pages
/// to a single huge page.
///
/// Increments `promotions_attempted` for accounting purposes.  Returns
/// `false` because the physical contiguous 2 MB allocator is not yet
/// available.
pub fn thp_promote(addr: u64) -> bool {
    {
        let mut s = THP_STATE.lock();
        s.promotions_attempted = s.promotions_attempted.saturating_add(1);
    }
    serial_println!(
        "  [thp] thp_promote addr={:#x} — stub, promotion not yet implemented",
        addr
    );
    false
}

/// khugepaged stub: scan and attempt to promote eligible regions.
///
/// Increments the scan-round counter and logs.  No actual scanning is
/// performed until the page-walker and 2 MB allocator are wired.
pub fn thp_scan_and_promote() {
    let round = SCAN_ROUNDS.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    serial_println!("  [thp] thp_scan_and_promote round={} — stub", round);
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return `(hugepages_allocated, promotions_attempted)`.
pub fn thp_get_stats() -> (u64, u64) {
    let s = THP_STATE.lock();
    (s.hugepages_allocated, s.promotions_attempted)
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the THP subsystem.
///
/// Sets mode to `Never` (safe default) and logs readiness.
pub fn init() {
    {
        let mut s = THP_STATE.lock();
        s.mode = ThpMode::Never;
        s.hugepages_allocated = 0;
        s.promotions_attempted = 0;
    }
    SCAN_ROUNDS.store(0, Ordering::Relaxed);
    serial_println!(
        "  [thp] initialized, HUGEPAGE_SIZE={}KB, mode=Never (stub)",
        HUGEPAGE_SIZE / 1024
    );
}
