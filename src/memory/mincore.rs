/// mincore --- page-residency query for Genesis
///
/// Provides mincore(2) semantics: for each page in `[addr, addr+len)`,
/// write a byte to `vec_out` indicating whether the page is resident in
/// physical memory (1) or not (0).
///
/// On a bare-metal kernel with no swap all mapped pages are always
/// resident, so this implementation writes 1 for every page.
///
/// Kernel rules enforced throughout:
///   - No heap (no Vec / Box / String / alloc)
///   - No float casts (no `as f32` / `as f64`)
///   - No panics (no unwrap / expect / panic!)
///   - All counters use saturating arithmetic
///   - MMIO via read_volatile / write_volatile only
use crate::serial_println;

/// Page size (4 KiB).
const PAGE_SIZE: u64 = 4096;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// `sys_mincore(addr, len, vec_out) -> i64`
///
/// Writes one byte per page in `[addr, addr+len)` to the buffer at
/// `vec_out`.  A byte value of 1 means the page is resident; 0 means it
/// has been swapped out.  On Genesis all mapped pages are always resident.
///
/// Returns:
///    0   — success
///   -22  — EINVAL (addr not page-aligned or len == 0)
///   -14  — EFAULT (vec_out is null)
pub fn sys_mincore(addr: u64, len: u64, vec_out: u64) -> i64 {
    // Validate address alignment.
    if addr & 0xFFF != 0 {
        return -22; // EINVAL
    }
    // Validate length.
    if len == 0 {
        return -22; // EINVAL
    }
    // Validate output pointer.
    if vec_out == 0 {
        return -14; // EFAULT
    }

    // Number of pages whose status we must report.
    // Round up: a partial final page still gets a status byte.
    let num_pages = len.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE;

    // Write residency byte for each page.
    // On Genesis (no swap) every mapped page is resident → value = 1.
    for i in 0..num_pages {
        unsafe {
            // Safety: bare-metal kernel.  The caller is responsible for
            // providing a valid buffer of at least `num_pages` bytes.
            // We use write_volatile so the compiler cannot elide the stores.
            (vec_out as *mut u8).add(i as usize).write_volatile(1u8);
        }
    }

    serial_println!(
        "  [mincore] addr={:#x} len={} pages={} vec_out={:#x}",
        addr,
        len,
        num_pages,
        vec_out
    );

    0
}

/// Initialise the mincore subsystem.
pub fn init() {
    serial_println!("  [mincore] subsystem ready");
}
