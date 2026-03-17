use crate::serial_println;
/// madvise --- memory advisory hints for Genesis
///
/// Provides madvise(2) semantics: the process advises the kernel about
/// expected access patterns for a virtual address range so the kernel
/// can optimise prefetch, eviction and THP behaviour.
///
/// All advice is recorded in a fixed-size hint table (256 entries).
/// MADV_DONTNEED performs an immediate zero-and-remove; all other advice
/// values record the hint for use by the page-reclaim and readahead paths.
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
// MADV_* constants (matches Linux ABI)
// ---------------------------------------------------------------------------

pub const MADV_NORMAL: i32 = 0;
pub const MADV_RANDOM: i32 = 1;
pub const MADV_SEQUENTIAL: i32 = 2;
pub const MADV_WILLNEED: i32 = 3;
pub const MADV_DONTNEED: i32 = 4;
pub const MADV_FREE: i32 = 8;
pub const MADV_REMOVE: i32 = 9;
pub const MADV_DONTFORK: i32 = 10;
pub const MADV_DOFORK: i32 = 11;
pub const MADV_MERGEABLE: i32 = 12;
pub const MADV_UNMERGEABLE: i32 = 13;
pub const MADV_HUGEPAGE: i32 = 14;
pub const MADV_NOHUGEPAGE: i32 = 15;
pub const MADV_DONTDUMP: i32 = 16;
pub const MADV_DODUMP: i32 = 17;
pub const MADV_WIPEONFORK: i32 = 18;
pub const MADV_KEEPONFORK: i32 = 19;

/// Page size (4 KiB)
const PAGE_SIZE: u64 = 4096;

/// Maximum number of concurrent advisory hints
const MAX_HINTS: usize = 256;

// ---------------------------------------------------------------------------
// MadviseHint
// ---------------------------------------------------------------------------

/// A single advisory hint covering an address range.
#[derive(Clone, Copy)]
pub struct MadviseHint {
    /// Base address of the range (page-aligned)
    pub addr: u64,
    /// Length of the range in bytes
    pub len: u64,
    /// MADV_* advice constant
    pub advice: i32,
    /// Slot is in use
    pub active: bool,
}

impl MadviseHint {
    pub const fn empty() -> Self {
        MadviseHint {
            addr: 0,
            len: 0,
            advice: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static hint table
// ---------------------------------------------------------------------------

static MADVISE_HINTS: Mutex<[MadviseHint; MAX_HINTS]> = Mutex::new({
    const EMPTY: MadviseHint = MadviseHint::empty();
    [EMPTY; MAX_HINTS]
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `advice` is a recognised MADV_* constant.
fn is_valid_advice(advice: i32) -> bool {
    matches!(
        advice,
        MADV_NORMAL
            | MADV_RANDOM
            | MADV_SEQUENTIAL
            | MADV_WILLNEED
            | MADV_DONTNEED
            | MADV_FREE
            | MADV_REMOVE
            | MADV_DONTFORK
            | MADV_DOFORK
            | MADV_MERGEABLE
            | MADV_UNMERGEABLE
            | MADV_HUGEPAGE
            | MADV_NOHUGEPAGE
            | MADV_DONTDUMP
            | MADV_DODUMP
            | MADV_WIPEONFORK
            | MADV_KEEPONFORK
    )
}

/// Record `advice` for `[addr, addr+len)`.  Returns `false` if the table
/// is full.
fn record_hint(hints: &mut [MadviseHint; MAX_HINTS], addr: u64, len: u64, advice: i32) -> bool {
    // Overwrite an existing hint for the same addr if present.
    for h in hints.iter_mut() {
        if h.active && h.addr == addr {
            h.len = len;
            h.advice = advice;
            return true;
        }
    }
    // Find a free slot.
    for h in hints.iter_mut() {
        if !h.active {
            h.addr = addr;
            h.len = len;
            h.advice = advice;
            h.active = true;
            return true;
        }
    }
    false
}

/// Remove the hint whose base address equals `addr`.
fn remove_hint(hints: &mut [MadviseHint; MAX_HINTS], addr: u64) {
    for h in hints.iter_mut() {
        if h.active && h.addr == addr {
            *h = MadviseHint::empty();
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `sys_madvise(addr, len, advice) -> i64`
///
/// Advise the kernel about the expected memory access pattern for the
/// range `[addr, addr+len)`.
///
/// Returns 0 on success, or a negative errno:
///   -22 (EINVAL) — invalid argument (bad alignment, zero length, bad advice)
pub fn sys_madvise(addr: u64, len: u64, advice: i32) -> i64 {
    // Validate: addr must be page-aligned.
    if addr & 0xFFF != 0 {
        return -22; // EINVAL
    }
    // Validate: len must be non-zero.
    if len == 0 {
        return -22; // EINVAL
    }
    // Validate: advice must be a known constant.
    if !is_valid_advice(advice) {
        return -22; // EINVAL
    }

    match advice {
        MADV_DONTNEED => {
            // Zero every byte in [addr, addr+len) using 8-byte write_volatile
            // steps, then remove any recorded hint.
            let end = addr.saturating_add(len);
            let mut cursor = addr;
            while cursor.saturating_add(8) <= end {
                // Safety: bare-metal kernel — we own the address space.
                // We use write_volatile to prevent the compiler from optimising
                // away the zeroing (important for security / memory reclaim).
                unsafe {
                    core::ptr::write_volatile(cursor as *mut u64, 0u64);
                }
                cursor = cursor.saturating_add(8);
            }
            // Handle any trailing bytes (< 8) individually.
            while cursor < end {
                unsafe {
                    core::ptr::write_volatile(cursor as *mut u8, 0u8);
                }
                cursor = cursor.saturating_add(1);
            }
            let mut hints = MADVISE_HINTS.lock();
            remove_hint(&mut hints, addr);
            serial_println!("  [madvise] DONTNEED {:#x} len={}", addr, len);
            0
        }

        MADV_WILLNEED => {
            // Record a prefetch hint; actual prefetch is handled by the
            // page-fault path which can consult madvise_get_hint().
            let mut hints = MADVISE_HINTS.lock();
            if !record_hint(&mut hints, addr, len, advice) {
                serial_println!("  [madvise] WILLNEED hint table full (addr={:#x})", addr);
            } else {
                serial_println!("  [madvise] WILLNEED {:#x} len={}", addr, len);
            }
            0
        }

        MADV_SEQUENTIAL | MADV_RANDOM | MADV_NORMAL => {
            // Update readahead hint.
            let mut hints = MADVISE_HINTS.lock();
            if !record_hint(&mut hints, addr, len, advice) {
                serial_println!(
                    "  [madvise] readahead hint table full (addr={:#x} advice={})",
                    addr,
                    advice
                );
            } else {
                serial_println!(
                    "  [madvise] readahead hint {} addr={:#x} len={}",
                    advice,
                    addr,
                    len
                );
            }
            0
        }

        MADV_FREE => {
            // Mark pages as freeable — the reclaim path (madvise_tick) may
            // reclaim them under memory pressure.
            let mut hints = MADVISE_HINTS.lock();
            if !record_hint(&mut hints, addr, len, advice) {
                serial_println!("  [madvise] FREE hint table full (addr={:#x})", addr);
            } else {
                serial_println!("  [madvise] FREE {:#x} len={}", addr, len);
            }
            0
        }

        MADV_HUGEPAGE | MADV_NOHUGEPAGE => {
            // THP policy stub — log the intent, return success.
            serial_println!(
                "  [madvise] THP hint {} addr={:#x} len={} (stub)",
                advice,
                addr,
                len
            );
            let mut hints = MADVISE_HINTS.lock();
            // Best-effort record; ignore table-full.
            let _ = record_hint(&mut hints, addr, len, advice);
            0
        }

        _ => {
            // All other valid advice constants: record the hint.
            let mut hints = MADVISE_HINTS.lock();
            if !record_hint(&mut hints, addr, len, advice) {
                serial_println!(
                    "  [madvise] hint table full (addr={:#x} advice={})",
                    addr,
                    advice
                );
            } else {
                serial_println!("  [madvise] hint {} addr={:#x} len={}", advice, addr, len);
            }
            0
        }
    }
}

/// Return the recorded advice for the page containing `addr`, or
/// `MADV_NORMAL` if no hint is present.
pub fn madvise_get_hint(addr: u64) -> i32 {
    let hints = MADVISE_HINTS.lock();
    for h in hints.iter() {
        if h.active && addr >= h.addr && addr < h.addr.saturating_add(h.len) {
            return h.advice;
        }
    }
    MADV_NORMAL
}

/// Remove all hints whose range overlaps `[addr, addr+len)`.
pub fn madvise_clear_range(addr: u64, len: u64) {
    let end = addr.saturating_add(len);
    let mut hints = MADVISE_HINTS.lock();
    for h in hints.iter_mut() {
        if !h.active {
            continue;
        }
        let h_end = h.addr.saturating_add(h.len);
        // Overlap test: hint.addr < end && hint.end > addr
        if h.addr < end && h_end > addr {
            *h = MadviseHint::empty();
        }
    }
}

/// Periodically process `MADV_FREE` pages.
///
/// Under memory pressure the reclaim engine would call this to actually
/// zero and return MADV_FREE pages to the allocator. This is a stub that
/// logs the pending free hints; real reclaim integration is done via the
/// `reclaim` module.
pub fn madvise_tick() {
    let hints = MADVISE_HINTS.lock();
    for h in hints.iter() {
        if h.active && h.advice == MADV_FREE {
            serial_println!(
                "  [madvise] tick: MADV_FREE pending {:#x} len={} (stub)",
                h.addr,
                h.len
            );
        }
    }
}

/// Round a byte count up to the next multiple of PAGE_SIZE.
#[allow(dead_code)]
fn pages_for(len: u64) -> u64 {
    (len.saturating_add(PAGE_SIZE - 1)) / PAGE_SIZE
}

/// Initialise the madvise subsystem.
pub fn init() {
    serial_println!("  [madvise] hint table ready (max {} entries)", MAX_HINTS);
}
