/// POSIX and System V shared memory (no-heap, fixed-size static arrays)
///
/// Provides both POSIX shm_open/shm_close/shm_unlink/shm_truncate and
/// System V shmget/shmat/shmdt/shmctl APIs over a single fixed array of
/// 32 shared-memory regions, each holding up to 64 KB of data.
///
/// Design constraints (bare-metal Genesis AIOS rules):
///   - NO heap: no Vec, Box, String, alloc::*
///   - NO floats
///   - NO panics: no unwrap(), expect(), panic!()
///   - Counters: saturating_add / saturating_sub only
///   - All array accesses bounds-checked (if idx < N)
///   - Static Mutex over fixed-size Copy array
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_SHM_REGIONS: usize = 32;
pub const SHM_MAX_SIZE: usize = 65536; // 64 KB per region
pub const SHM_NAME_LEN: usize = 64;

// POSIX open flags
pub const SHM_O_CREAT: u32 = 0x40;
pub const SHM_O_EXCL: u32 = 0x80;
pub const SHM_O_RDONLY: u32 = 0;
pub const SHM_O_RDWR: u32 = 0x2;
pub const SHM_O_TRUNC: u32 = 0x200;

// System V IPC control commands
pub const IPC_RMID: u32 = 0;
pub const IPC_SET: u32 = 1;
pub const IPC_STAT: u32 = 2;

// System V IPC_CREAT / IPC_EXCL flags
pub const IPC_CREAT: u32 = 0o001000;
pub const IPC_EXCL: u32 = 0o002000;

/// fd encoding base — shm fds start here to avoid collision with vfs fds.
pub const SHM_FD_BASE: i32 = 7000;

// ---------------------------------------------------------------------------
// ShmRegion — a single shared memory slot (Copy + const fn empty for Mutex)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct ShmRegion {
    /// IPC key passed to shmget (0 = unused).
    pub key: i32,
    /// shmid returned to the caller (index + 1; 0 = invalid).
    pub id: u32,
    /// Requested / active size in bytes (0..=SHM_MAX_SIZE).
    pub size: usize,
    /// Number of processes that have attached this region.
    pub nattach: u32,
    /// PID of the process that created the region.
    pub creator_pid: u32,
    /// In-kernel data buffer.
    pub data: [u8; SHM_MAX_SIZE],
    /// true when this slot is in use.
    pub active: bool,
    /// Null-terminated name (POSIX shm_open names) or synthetic SysV name.
    pub name: [u8; SHM_NAME_LEN],
    /// Open flags recorded at creation time.
    pub flags: u32,
}

impl ShmRegion {
    pub const fn empty() -> Self {
        Self {
            key: 0,
            id: 0,
            size: 0,
            nattach: 0,
            creator_pid: 0,
            data: [0u8; SHM_MAX_SIZE],
            active: false,
            name: [0u8; SHM_NAME_LEN],
            flags: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static region table
//
// NOTE: ShmRegion is ~65 KB each; 32 entries = ~2 MB in BSS.
// Acceptable for a kernel static data segment; linker script must reserve
// enough BSS.  Do NOT place this in .rodata or stack.
// ---------------------------------------------------------------------------

static SHM_REGIONS: Mutex<[ShmRegion; MAX_SHM_REGIONS]> =
    Mutex::new([ShmRegion::empty(); MAX_SHM_REGIONS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy at most `SHM_NAME_LEN - 1` bytes from `src` into `dst`, null-terminate.
fn copy_name(dst: &mut [u8; SHM_NAME_LEN], src: &[u8]) {
    let len = if src.len() >= SHM_NAME_LEN {
        SHM_NAME_LEN - 1
    } else {
        src.len()
    };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    // Null-terminate remainder
    while i < SHM_NAME_LEN {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

/// Compare a stored name with a byte-slice key; both treated as null-terminated.
fn name_eq(stored: &[u8; SHM_NAME_LEN], key: &[u8]) -> bool {
    let klen = if key.len() >= SHM_NAME_LEN {
        SHM_NAME_LEN - 1
    } else {
        key.len()
    };
    let mut i = 0usize;
    while i < klen {
        if stored[i] != key[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    // Both must terminate at the same position
    stored[klen] == 0
}

/// Find the index of the first active region whose name matches `name_bytes`.
/// Returns `MAX_SHM_REGIONS` if not found.
fn find_by_name(regions: &[ShmRegion; MAX_SHM_REGIONS], name_bytes: &[u8]) -> usize {
    let mut i = 0usize;
    while i < MAX_SHM_REGIONS {
        if regions[i].active && name_eq(&regions[i].name, name_bytes) {
            return i;
        }
        i = i.saturating_add(1);
    }
    MAX_SHM_REGIONS // sentinel: not found
}

/// Find the index of the first inactive (free) slot.
/// Returns `MAX_SHM_REGIONS` if the table is full.
fn find_free(regions: &[ShmRegion; MAX_SHM_REGIONS]) -> usize {
    let mut i = 0usize;
    while i < MAX_SHM_REGIONS {
        if !regions[i].active {
            return i;
        }
        i = i.saturating_add(1);
    }
    MAX_SHM_REGIONS
}

/// Convert an shmid (fd) back to an array index.
/// shmid = SHM_FD_BASE + index
fn shmid_to_idx(shmid: i32) -> usize {
    if shmid < SHM_FD_BASE {
        return MAX_SHM_REGIONS;
    } // invalid sentinel
    let idx = (shmid - SHM_FD_BASE) as usize;
    if idx >= MAX_SHM_REGIONS {
        MAX_SHM_REGIONS
    } else {
        idx
    }
}

// ---------------------------------------------------------------------------
// System V shmget — get or create a shared memory segment
// ---------------------------------------------------------------------------

/// shmget — create or open a System V shared memory segment.
///
/// - If `key == 0` (IPC_PRIVATE): always create a new anonymous segment.
/// - Otherwise: create if `flags & IPC_CREAT` is set and it doesn't exist,
///   or return the existing segment's id.
///
/// Returns the shmid (>= SHM_FD_BASE) on success, or -1 on failure.
pub fn shm_get(key: i32, size: usize, flags: i32) -> i32 {
    if size > SHM_MAX_SIZE && size != 0 {
        return -1;
    }

    let mut regions = SHM_REGIONS.lock();

    // Build synthetic name from key for lookup/storage
    let mut name_buf = [0u8; SHM_NAME_LEN];
    if key == 0 {
        // IPC_PRIVATE: always new; pick a synthetic name based on first free slot
        let idx = find_free(&regions);
        if idx >= MAX_SHM_REGIONS {
            return -1;
        } // table full
          // name = "__priv_NNN"
        write_sysv_name(&mut name_buf, b"__priv_", idx as u32);
        let region = &mut regions[idx];
        region.active = true;
        region.key = 0;
        region.id = (idx as u32).saturating_add(1);
        region.size = if size == 0 { 1 } else { size };
        region.nattach = 0;
        region.creator_pid = 0;
        region.flags = flags as u32;
        copy_name(&mut region.name, &name_buf[..name_len(&name_buf)]);
        zero_data(region, 0, region.size);
        return SHM_FD_BASE.saturating_add(idx as i32);
    }

    write_sysv_name(&mut name_buf, b"__sysv_", key as u32);
    let nlen = name_len(&name_buf);

    let existing = find_by_name(&regions, &name_buf[..nlen]);
    let do_create = (flags as u32) & IPC_CREAT != 0;
    let do_excl = (flags as u32) & IPC_EXCL != 0;

    if existing < MAX_SHM_REGIONS {
        // Already exists
        if do_create && do_excl {
            return -1;
        } // EEXIST
          // Existing segment: validate size fits
        if size > 0 && size > regions[existing].size {
            return -1;
        }
        return SHM_FD_BASE.saturating_add(existing as i32);
    }

    // Does not exist
    if !do_create {
        return -1;
    } // ENOENT

    let idx = find_free(&regions);
    if idx >= MAX_SHM_REGIONS {
        return -1;
    }

    let actual_size = if size == 0 { 1 } else { size };
    let region = &mut regions[idx];
    region.active = true;
    region.key = key;
    region.id = (idx as u32).saturating_add(1);
    region.size = actual_size;
    region.nattach = 0;
    region.creator_pid = 0;
    region.flags = flags as u32;
    copy_name(&mut region.name, &name_buf[..nlen]);
    zero_data(region, 0, actual_size);

    SHM_FD_BASE.saturating_add(idx as i32)
}

// ---------------------------------------------------------------------------
// shm_attach — attach a shared memory segment
// ---------------------------------------------------------------------------

/// shm_attach — attach a shared memory segment.
///
/// In a bare-metal context without per-process address spaces, this returns
/// a pointer directly into the kernel-static data buffer and increments the
/// attach count.
///
/// Returns a raw pointer to the data buffer, or null on error.
pub fn shm_attach(shmid: i32) -> *const u8 {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return core::ptr::null();
    }
    let mut regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return core::ptr::null();
    }
    regions[idx].nattach = regions[idx].nattach.saturating_add(1);
    regions[idx].data.as_ptr()
}

// ---------------------------------------------------------------------------
// shm_detach — detach a shared memory segment
// ---------------------------------------------------------------------------

/// shm_detach — detach a shared memory segment.
///
/// Decrements the attach count (saturating; never underflows).
/// Returns true on success, false if the shmid is invalid.
pub fn shm_detach(shmid: i32) -> bool {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return false;
    }
    let mut regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return false;
    }
    regions[idx].nattach = regions[idx].nattach.saturating_sub(1);
    true
}

// ---------------------------------------------------------------------------
// shm_ctl_rm — remove a shared memory region
// ---------------------------------------------------------------------------

/// shm_ctl_rm — mark a region as inactive (remove it).
///
/// Returns true on success, false if shmid is invalid or inactive.
pub fn shm_ctl_rm(shmid: i32) -> bool {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return false;
    }
    let mut regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return false;
    }
    regions[idx] = ShmRegion::empty();
    true
}

// ---------------------------------------------------------------------------
// shm_read / shm_write — data access
// ---------------------------------------------------------------------------

/// shm_read — copy bytes from the shared region into `buf`.
///
/// Reads starting at `offset`; copies `min(buf.len(), available)` bytes.
/// Returns the number of bytes actually copied.
pub fn shm_read(shmid: i32, offset: usize, buf: &mut [u8]) -> usize {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return 0;
    }
    let regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return 0;
    }
    let size = regions[idx].size;
    if offset >= size {
        return 0;
    }
    let avail = size.saturating_sub(offset);
    let to_copy = if buf.len() < avail { buf.len() } else { avail };
    let mut i = 0usize;
    while i < to_copy {
        buf[i] = regions[idx].data[offset.saturating_add(i)];
        i = i.saturating_add(1);
    }
    to_copy
}

/// shm_write — copy bytes from `data` into the shared region at `offset`.
///
/// Returns the number of bytes actually written (may be less than `data.len()`
/// if the write would exceed `SHM_MAX_SIZE`).
pub fn shm_write(shmid: i32, offset: usize, data: &[u8]) -> usize {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return 0;
    }
    let mut regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return 0;
    }
    let size = regions[idx].size;
    if offset >= size {
        return 0;
    }
    let avail = size.saturating_sub(offset);
    let to_copy = if data.len() < avail {
        data.len()
    } else {
        avail
    };
    let mut i = 0usize;
    while i < to_copy {
        regions[idx].data[offset.saturating_add(i)] = data[i];
        i = i.saturating_add(1);
    }
    to_copy
}

// ---------------------------------------------------------------------------
// POSIX shm_open / shm_close / shm_unlink / shm_truncate
// ---------------------------------------------------------------------------

/// shm_open — open or create a POSIX shared memory object.
///
/// Returns an fd (>= SHM_FD_BASE) on success, or a negative errno
/// (-2 = ENOENT, -17 = EEXIST, -12 = ENOMEM, -22 = EINVAL).
pub fn shm_open(name: &[u8], flags: u32, _mode: u16) -> i32 {
    let do_create = flags & SHM_O_CREAT != 0;
    let do_excl = flags & SHM_O_EXCL != 0;
    let do_trunc = flags & SHM_O_TRUNC != 0;

    let mut regions = SHM_REGIONS.lock();
    let existing = find_by_name(&regions, name);

    if do_create {
        if existing < MAX_SHM_REGIONS && do_excl {
            return -17;
        } // EEXIST
        if existing >= MAX_SHM_REGIONS {
            // Create new with 1-byte placeholder (caller uses shm_truncate)
            let idx = find_free(&regions);
            if idx >= MAX_SHM_REGIONS {
                return -12;
            } // ENOMEM
            let region = &mut regions[idx];
            region.active = true;
            region.key = 0;
            region.id = (idx as u32).saturating_add(1);
            region.size = 1;
            region.nattach = 0;
            region.creator_pid = 0;
            region.flags = flags;
            copy_name(&mut region.name, name);
            zero_data(region, 0, 1);
            return SHM_FD_BASE.saturating_add(idx as i32);
        }
        // Exists and no EXCL: fall through to open
    } else if existing >= MAX_SHM_REGIONS {
        return -2; // ENOENT
    }

    let idx = find_by_name(&regions, name);
    if idx >= MAX_SHM_REGIONS {
        return -2;
    }

    if do_trunc {
        // Truncate to zero content (keep 1-byte minimum size)
        let sz = regions[idx].size;
        zero_data(&mut regions[idx], 0, sz);
    }
    regions[idx].flags = flags;
    SHM_FD_BASE.saturating_add(idx as i32)
}

/// shm_close — release an fd (no-op in our model; regions persist until unlinked).
pub fn shm_close(fd: i32) {
    let idx = shmid_to_idx(fd);
    if idx >= MAX_SHM_REGIONS {
        return;
    }
    // Mark detach without removing: region stays alive until shm_unlink
    let mut regions = SHM_REGIONS.lock();
    if regions[idx].active {
        regions[idx].nattach = regions[idx].nattach.saturating_sub(1);
    }
}

/// shm_unlink — remove the named shared memory object.
///
/// Returns 0 on success, -2 (ENOENT) if not found.
pub fn shm_unlink(name: &[u8]) -> i32 {
    let mut regions = SHM_REGIONS.lock();
    let idx = find_by_name(&regions, name);
    if idx >= MAX_SHM_REGIONS {
        return -2;
    }
    regions[idx] = ShmRegion::empty();
    0
}

/// shm_truncate — resize a shared memory object identified by fd.
///
/// The new size must be > 0 and <= SHM_MAX_SIZE.
/// Returns 0 on success, or a negative errno (-9 = EBADF, -22 = EINVAL).
pub fn shm_truncate(fd: i32, new_size: usize) -> i32 {
    if new_size == 0 || new_size > SHM_MAX_SIZE {
        return -22;
    } // EINVAL
    let idx = shmid_to_idx(fd);
    if idx >= MAX_SHM_REGIONS {
        return -9;
    } // EBADF
    let mut regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return -9;
    }
    // Zero out the newly exposed area if expanding
    let old_size = regions[idx].size;
    if new_size > old_size {
        zero_data(
            &mut regions[idx],
            old_size,
            new_size.saturating_sub(old_size),
        );
    }
    regions[idx].size = new_size;
    0
}

/// shm_size — return the current size of the shared memory object.
pub fn shm_size(fd: i32) -> usize {
    let idx = shmid_to_idx(fd);
    if idx >= MAX_SHM_REGIONS {
        return 0;
    }
    let regions = SHM_REGIONS.lock();
    if !regions[idx].active {
        return 0;
    }
    regions[idx].size
}

// ---------------------------------------------------------------------------
// System V shmat / shmdt / shmctl
// ---------------------------------------------------------------------------

/// shmat — attach a System V shared memory segment.
///
/// Returns the kernel-virtual address of the data buffer as u64,
/// or u64::MAX on error.
pub fn shmat(shmid: i32, _addr: u64, _flags: u32) -> u64 {
    shm_attach(shmid) as u64
}

/// shmdt — detach a System V shared memory segment by address.
///
/// In the bare-metal model, all regions are kernel-static; we search for the
/// region whose data pointer matches `addr`.
/// Returns 0 on success, -22 (EINVAL) if not found.
pub fn shmdt(addr: u64) -> i32 {
    if addr == 0 || addr == u64::MAX {
        return -22;
    }
    let ptr = addr as *const u8;
    let mut regions = SHM_REGIONS.lock();
    let mut i = 0usize;
    while i < MAX_SHM_REGIONS {
        if regions[i].active && regions[i].data.as_ptr() == ptr {
            regions[i].nattach = regions[i].nattach.saturating_sub(1);
            return 0;
        }
        i = i.saturating_add(1);
    }
    -22 // EINVAL
}

/// shmctl — System V shared memory control.
///
/// Supported commands:
///   IPC_RMID (0) — remove the segment
///   IPC_STAT (2) — stub (no-op, returns 0)
///   IPC_SET  (1) — stub (no-op, returns 0)
///
/// Returns 0 on success, or a negative errno.
pub fn shmctl(shmid: i32, cmd: u32, _buf_ptr: u64) -> i32 {
    let idx = shmid_to_idx(shmid);
    if idx >= MAX_SHM_REGIONS {
        return -9;
    } // EBADF
    match cmd {
        IPC_RMID => {
            let mut regions = SHM_REGIONS.lock();
            if !regions[idx].active {
                return -9;
            }
            regions[idx] = ShmRegion::empty();
            0
        }
        IPC_STAT => 0, // stub
        IPC_SET => 0,  // stub
        _ => -22,      // EINVAL
    }
}

// ---------------------------------------------------------------------------
// Convenience: string-based region existence check
// ---------------------------------------------------------------------------

/// Returns true if a POSIX shared memory region with the given name exists.
pub fn exists(name: &[u8]) -> bool {
    let regions = SHM_REGIONS.lock();
    find_by_name(&regions, name) < MAX_SHM_REGIONS
}

/// Returns the total number of active shared memory regions.
pub fn region_count() -> usize {
    let regions = SHM_REGIONS.lock();
    let mut count = 0usize;
    let mut i = 0usize;
    while i < MAX_SHM_REGIONS {
        if regions[i].active {
            count = count.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    count
}

// ---------------------------------------------------------------------------
// Module initialisation
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("    [shm] POSIX shared memory initialized");
}

// ---------------------------------------------------------------------------
// Internal utility functions (no heap, no alloc)
// ---------------------------------------------------------------------------

/// Write a null-terminated synthetic name into `dst`:
///   prefix (e.g. b"__sysv_") followed by decimal representation of `n`.
fn write_sysv_name(dst: &mut [u8; SHM_NAME_LEN], prefix: &[u8], n: u32) {
    let mut i = 0usize;
    // Copy prefix
    let plen = if prefix.len() < SHM_NAME_LEN {
        prefix.len()
    } else {
        SHM_NAME_LEN - 1
    };
    while i < plen {
        dst[i] = prefix[i];
        i = i.saturating_add(1);
    }
    // Append decimal digits of n
    // Max u32 decimal = "4294967295" (10 digits)
    let mut digits = [0u8; 10];
    let mut dcount = 0usize;
    let mut val = n;
    if val == 0 {
        if i < SHM_NAME_LEN - 1 {
            dst[i] = b'0';
            i = i.saturating_add(1);
        }
    } else {
        while val > 0 && dcount < 10 {
            digits[dcount] = b'0' + (val % 10) as u8;
            val /= 10;
            dcount = dcount.saturating_add(1);
        }
        // digits are in reverse order
        let mut d = dcount;
        while d > 0 {
            d = d.saturating_sub(1);
            if i < SHM_NAME_LEN - 1 {
                dst[i] = digits[d];
                i = i.saturating_add(1);
            }
        }
    }
    // Null-terminate
    if i < SHM_NAME_LEN {
        dst[i] = 0;
    }
}

/// Return the length of the null-terminated content in `name` (not counting null).
fn name_len(name: &[u8; SHM_NAME_LEN]) -> usize {
    let mut i = 0usize;
    while i < SHM_NAME_LEN {
        if name[i] == 0 {
            return i;
        }
        i = i.saturating_add(1);
    }
    SHM_NAME_LEN
}

/// Zero `count` bytes of `region.data` starting at `offset`.
/// Silently clamps to the data buffer bounds.
fn zero_data(region: &mut ShmRegion, offset: usize, count: usize) {
    let end = offset.saturating_add(count);
    let end = if end > SHM_MAX_SIZE {
        SHM_MAX_SIZE
    } else {
        end
    };
    let mut i = offset;
    while i < end {
        region.data[i] = 0;
        i = i.saturating_add(1);
    }
}
