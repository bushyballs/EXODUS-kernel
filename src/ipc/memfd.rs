use crate::sync::Mutex;
/// memfd_create — anonymous memory-backed file descriptors
///
/// Provides `memfd_create(name, flags)` (Linux syscall 319).
///
/// A memfd is an anonymous, in-kernel file that exists entirely in memory.
/// It has no VFS path, can be passed between processes via fd-passing over
/// Unix sockets, and supports sealing to prevent further modifications.
///
/// Typical uses:
///   - Shared memory IPC (no /dev/shm or filesystem needed)
///   - JIT compilers (write code, seal, execute)
///   - Ephemeral file-like storage
///
/// Limits (no-heap, fixed-size arrays):
///   - MAX_MEMFDS = 32 concurrent anonymous fds
///   - MEMFD_DATA_SIZE = 65536 bytes (64 KiB) per fd
///
/// FD numbers are allocated as 5000 + slot_index to avoid collision
/// with regular file-descriptors (which start at 3).
///
/// Rules: no_std, no heap, no float casts, saturating arithmetic.
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

pub const MAX_MEMFDS: usize = 32;
pub const MEMFD_DATA_SIZE: usize = 4096 * 16; // 64 KiB per fd
const FD_BASE: i32 = 5000; // offset for fd numbers

// ---------------------------------------------------------------------------
// Flags for memfd_create(2)
// ---------------------------------------------------------------------------

pub const MFD_CLOEXEC: u32 = 0x0001;
pub const MFD_ALLOW_SEALING: u32 = 0x0002;

// ---------------------------------------------------------------------------
// Seal flags (used with F_ADD_SEALS / F_GET_SEALS via fcntl)
// ---------------------------------------------------------------------------

pub const F_SEAL_SEAL: u32 = 0x0001; // no more seals can be added
pub const F_SEAL_SHRINK: u32 = 0x0002; // cannot truncate smaller
pub const F_SEAL_GROW: u32 = 0x0004; // cannot truncate larger
pub const F_SEAL_WRITE: u32 = 0x0008; // no writes allowed

// ---------------------------------------------------------------------------
// Errno mirrors
// ---------------------------------------------------------------------------

const EBADF: i32 = -9;
const ENOMEM: i32 = -12;
const EFAULT: i32 = -14;
const EINVAL: i32 = -22;
const ENOSPC: i32 = -28;
const EPERM: i32 = -1;
const ENOMEM_ISIZE: isize = -12;
const EBADF_ISIZE: isize = -9;
const EFAULT_ISIZE: isize = -14;
const EINVAL_ISIZE: isize = -22;
const EPERM_ISIZE: isize = -1;

// ---------------------------------------------------------------------------
// MemFd slot
// ---------------------------------------------------------------------------

pub struct MemFd {
    pub fd: i32,
    pub name: [u8; 64],
    pub name_len: usize,
    pub data: [u8; MEMFD_DATA_SIZE],
    pub size: usize, // logical size (≤ MEMFD_DATA_SIZE)
    pub sealed: u32, // active F_SEAL_* flags
    pub flags: u32,  // MFD_* creation flags
    pub refcount: u32,
    pub in_use: bool,
}

impl MemFd {
    const fn empty() -> Self {
        MemFd {
            fd: 0,
            name: [0u8; 64],
            name_len: 0,
            data: [0u8; MEMFD_DATA_SIZE],
            size: 0,
            sealed: 0,
            flags: 0,
            refcount: 1,
            in_use: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global table — fixed-size, no heap
// ---------------------------------------------------------------------------

// We cannot have [Option<MemFd>; 32] as a const because MemFd contains a
// large array, so we store the slots directly and use `in_use` as the
// discriminant.  The Mutex wraps a flat array.

struct MemFdTable {
    slots: [MemFd; MAX_MEMFDS],
}

impl MemFdTable {
    const fn new() -> Self {
        // SAFETY: all-zero is valid for MemFd (in_use=false, everything 0)
        MemFdTable {
            slots: [
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
                MemFd::empty(),
            ],
        }
    }

    fn find_free(&self) -> Option<usize> {
        for i in 0..MAX_MEMFDS {
            if !self.slots[i].in_use {
                return Some(i);
            }
        }
        None
    }

    fn find_slot(&self, fd: i32) -> Option<usize> {
        let idx = fd.wrapping_sub(FD_BASE) as usize;
        if idx < MAX_MEMFDS && self.slots[idx].in_use && self.slots[idx].fd == fd {
            Some(idx)
        } else {
            // Linear scan as fallback (handles any fd aliasing)
            for i in 0..MAX_MEMFDS {
                if self.slots[i].in_use && self.slots[i].fd == fd {
                    return Some(i);
                }
            }
            None
        }
    }
}

static MEMFD_TABLE: Mutex<MemFdTable> = Mutex::new(MemFdTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new anonymous memory fd.
///
/// `name` is a human-readable label (≤ 63 bytes, truncated silently).
/// Returns fd number (≥ FD_BASE) on success, or a negative errno on error.
pub fn memfd_create(name: &[u8], flags: u32) -> i32 {
    // Only defined flags are allowed
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING) != 0 {
        return EINVAL;
    }

    let mut table = MEMFD_TABLE.lock();
    let slot = match table.find_free() {
        Some(s) => s,
        None => return ENOMEM,
    };

    let fd = FD_BASE.wrapping_add(slot as i32);

    // Copy name (truncate to 63 bytes; always NUL-terminate)
    let copy_len = if name.len() < 63 { name.len() } else { 63 };
    let mut name_buf = [0u8; 64];
    name_buf[..copy_len].copy_from_slice(&name[..copy_len]);

    let mfd = &mut table.slots[slot];
    mfd.fd = fd;
    mfd.name = name_buf;
    mfd.name_len = copy_len;
    mfd.size = 0;
    mfd.sealed = 0;
    mfd.flags = flags;
    mfd.refcount = 1;
    mfd.in_use = true;
    // Zero the data buffer
    for b in mfd.data.iter_mut() {
        *b = 0;
    }

    fd
}

/// Read `buf.len()` bytes from a memfd starting at `offset`.
///
/// Returns the number of bytes actually read, or a negative errno.
pub fn memfd_read(fd: i32, offset: usize, buf: &mut [u8]) -> isize {
    if buf.is_empty() {
        return 0;
    }

    let table = MEMFD_TABLE.lock();
    let slot = match table.find_slot(fd) {
        Some(s) => s,
        None => return EBADF_ISIZE,
    };
    let mfd = &table.slots[slot];

    if offset >= mfd.size {
        return 0; // EOF
    }

    let available = mfd.size.saturating_sub(offset);
    let take = if buf.len() < available {
        buf.len()
    } else {
        available
    };

    buf[..take].copy_from_slice(&mfd.data[offset..offset.saturating_add(take)]);
    take as isize
}

/// Write `data` into a memfd at `offset`, extending size as needed.
///
/// Returns the number of bytes written, or a negative errno.
pub fn memfd_write(fd: i32, offset: usize, data: &[u8]) -> isize {
    if data.is_empty() {
        return 0;
    }

    let mut table = MEMFD_TABLE.lock();
    let slot = match table.find_slot(fd) {
        Some(s) => s,
        None => return EBADF_ISIZE,
    };
    let mfd = &mut table.slots[slot];

    // Sealed for writing?
    if mfd.sealed & F_SEAL_WRITE != 0 {
        return EPERM_ISIZE;
    }

    let end = offset.saturating_add(data.len());
    if end > MEMFD_DATA_SIZE {
        return EFAULT_ISIZE; // would overflow fixed buffer
    }

    // Seal: no grow?
    if end > mfd.size && mfd.sealed & F_SEAL_GROW != 0 {
        return EPERM_ISIZE;
    }

    mfd.data[offset..end].copy_from_slice(data);
    if end > mfd.size {
        mfd.size = end;
    }
    data.len() as isize
}

/// Resize a memfd to `new_size` bytes (≤ MEMFD_DATA_SIZE).
///
/// Growing zero-fills the new region.  Returns 0 on success, negative errno
/// on error.
pub fn memfd_truncate(fd: i32, new_size: usize) -> i32 {
    if new_size > MEMFD_DATA_SIZE {
        return EINVAL;
    }

    let mut table = MEMFD_TABLE.lock();
    let slot = match table.find_slot(fd) {
        Some(s) => s,
        None => return EBADF,
    };
    let mfd = &mut table.slots[slot];

    // Seal checks
    if new_size < mfd.size && mfd.sealed & F_SEAL_SHRINK != 0 {
        return EPERM;
    }
    if new_size > mfd.size && mfd.sealed & F_SEAL_GROW != 0 {
        return EPERM;
    }
    if mfd.sealed & F_SEAL_WRITE != 0 {
        return EPERM;
    }

    if new_size > mfd.size {
        // Zero-fill the extension
        for b in mfd.data[mfd.size..new_size].iter_mut() {
            *b = 0;
        }
    }
    mfd.size = new_size;
    0
}

/// Add seals to a memfd (F_ADD_SEALS via fcntl).
///
/// Seals are cumulative and cannot be removed.  If F_SEAL_SEAL is already
/// set, no new seals may be added.  Returns 0 on success, negative errno
/// on error.
pub fn memfd_add_seals(fd: i32, seals: u32) -> i32 {
    // Validate flags
    let valid = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;
    if seals & !valid != 0 {
        return EINVAL;
    }

    let mut table = MEMFD_TABLE.lock();
    let slot = match table.find_slot(fd) {
        Some(s) => s,
        None => return EBADF,
    };
    let mfd = &mut table.slots[slot];

    // Sealing must have been enabled at creation
    if mfd.flags & MFD_ALLOW_SEALING == 0 {
        return EPERM;
    }
    // No new seals once F_SEAL_SEAL is set
    if mfd.sealed & F_SEAL_SEAL != 0 {
        return EPERM;
    }

    mfd.sealed |= seals;
    0
}

/// Query current seals on a memfd (F_GET_SEALS via fcntl).
///
/// Returns the seal bitmask, or 0 if the fd is not found.
pub fn memfd_get_seals(fd: i32) -> u32 {
    let table = MEMFD_TABLE.lock();
    match table.find_slot(fd) {
        Some(s) => table.slots[s].sealed,
        None => 0,
    }
}

/// Decrement the refcount and free the slot when it reaches zero.
pub fn memfd_close(fd: i32) {
    let mut table = MEMFD_TABLE.lock();
    let slot = match table.find_slot(fd) {
        Some(s) => s,
        None => return,
    };
    let mfd = &mut table.slots[slot];
    if mfd.refcount > 1 {
        mfd.refcount = mfd.refcount.saturating_sub(1);
    } else {
        mfd.in_use = false;
        mfd.refcount = 0;
        mfd.size = 0;
        mfd.sealed = 0;
        mfd.flags = 0;
    }
}

/// Return the logical size of a memfd in bytes, or 0 if not found.
pub fn memfd_size(fd: i32) -> usize {
    let table = MEMFD_TABLE.lock();
    match table.find_slot(fd) {
        Some(s) => table.slots[s].size,
        None => 0,
    }
}

/// Increment the reference count (e.g. when passing fd over a Unix socket).
pub fn memfd_addref(fd: i32) -> bool {
    let mut table = MEMFD_TABLE.lock();
    match table.find_slot(fd) {
        Some(s) => {
            table.slots[s].refcount = table.slots[s].refcount.saturating_add(1);
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Syscall entry point (called from syscall dispatch)
// ---------------------------------------------------------------------------

/// Handler for `SYS_MEMFD_CREATE` (nr 319).
///
/// `name_ptr` is a NUL-terminated user-space string pointer.
/// `name_len` is its byte length (without the NUL, ≤ 255).
///
/// # Safety
/// `name_ptr` must be valid and point to at least `name_len` readable bytes.
/// The caller (syscall dispatch) must validate the pointer before calling.
pub fn sys_memfd_create(name_ptr: *const u8, name_len: usize, flags: u32) -> i64 {
    if name_ptr.is_null() || name_len > 255 {
        return EINVAL as i64;
    }
    let name_bytes = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    memfd_create(name_bytes, flags) as i64
}

// ---------------------------------------------------------------------------
// Initialise
// ---------------------------------------------------------------------------

pub fn init() {
    // Table is already initialised by const constructor; nothing to do at runtime.
    serial_println!(
        "    [memfd] anonymous memfd table ready (max={}, data_per_fd={}B)",
        MAX_MEMFDS,
        MEMFD_DATA_SIZE
    );
}
