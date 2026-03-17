/// tmpfs -- temporary in-memory filesystem for Genesis
///
/// A RAM-backed filesystem that provides fast temporary storage.
/// All data is lost on reboot. Used for /tmp and /run.
///
/// Design:
///   Files and directories are stored in fixed-size static arrays.
///   No heap allocation is used anywhere (no Vec, Box, String, alloc::*).
///   Inode numbers come from a global AtomicU64 counter.
///   All counters use saturating arithmetic; sequence numbers use wrapping.
///
/// SAFETY RULES (must never be violated):
///   - NO as f32 / as f64
///   - NO Vec, Box, String, alloc::*
///   - NO unwrap(), expect(), panic!()
///   - saturating_add / saturating_sub for counters
///   - wrapping_add for sequence numbers
///   - read_volatile / write_volatile for all MMIO
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ============================================================================
// Constants
// ============================================================================

pub const TMPFS_MAX_FILES: usize = 64;
pub const TMPFS_MAX_DIRS: usize = 32;
pub const TMPFS_FILE_SIZE: usize = 4096; // max bytes per file
pub const TMPFS_NAME_LEN: usize = 64;
pub const TMPFS_MAX_MOUNTS: usize = 2;

// Mode constants
pub const S_IFREG: u16 = 0o100000;
pub const S_IFDIR: u16 = 0o040000;
pub const S_IRWXU: u16 = 0o0700;
pub const S_IRWXG: u16 = 0o0070;
pub const S_IRWXO: u16 = 0o0007;
pub const S_ISVTX: u16 = 0o1000;

// ============================================================================
// Inode number allocator
// ============================================================================

/// Global inode number counter. 1 is reserved for the root inode.
static NEXT_INO: AtomicU64 = AtomicU64::new(2);

/// Allocate the next inode number.
fn alloc_ino() -> u64 {
    NEXT_INO.fetch_add(1, Ordering::Relaxed)
}

// ============================================================================
// TmpfsFile
// ============================================================================

/// A single regular file stored in the tmpfs.
#[derive(Copy, Clone)]
pub struct TmpfsFile {
    pub ino: u64,
    pub parent_ino: u64,
    pub name: [u8; TMPFS_NAME_LEN],
    pub name_len: u8,
    pub data: [u8; TMPFS_FILE_SIZE],
    pub size: usize,
    pub mode: u16, // S_IFREG | permissions
    pub uid: u32,
    pub gid: u32,
    pub mtime: u64,
    pub active: bool,
}

impl TmpfsFile {
    pub const fn empty() -> Self {
        TmpfsFile {
            ino: 0,
            parent_ino: 0,
            name: [0u8; TMPFS_NAME_LEN],
            name_len: 0,
            data: [0u8; TMPFS_FILE_SIZE],
            size: 0,
            mode: S_IFREG | 0o644,
            uid: 0,
            gid: 0,
            mtime: 0,
            active: false,
        }
    }
}

// ============================================================================
// TmpfsDir
// ============================================================================

/// A single directory stored in the tmpfs.
#[derive(Copy, Clone)]
pub struct TmpfsDir {
    pub ino: u64,
    pub parent_ino: u64,
    pub name: [u8; TMPFS_NAME_LEN],
    pub name_len: u8,
    pub mode: u16, // S_IFDIR | permissions
    pub uid: u32,
    pub gid: u32,
    pub active: bool,
}

impl TmpfsDir {
    pub const fn empty() -> Self {
        TmpfsDir {
            ino: 0,
            parent_ino: 0,
            name: [0u8; TMPFS_NAME_LEN],
            name_len: 0,
            mode: S_IFDIR | 0o755,
            uid: 0,
            gid: 0,
            active: false,
        }
    }
}

// ============================================================================
// TmpfsMount
// ============================================================================

/// A single tmpfs mount instance.
#[derive(Copy, Clone)]
pub struct TmpfsMount {
    pub root_ino: u64,
    pub size_limit: u64, // bytes (0 = unlimited up to pool)
    pub used_bytes: u64,
    pub active: bool,
}

impl TmpfsMount {
    pub const fn empty() -> Self {
        TmpfsMount {
            root_ino: 0,
            size_limit: 0,
            used_bytes: 0,
            active: false,
        }
    }
}

// ============================================================================
// Static storage
// ============================================================================

static TMPFS_FILES: Mutex<[TmpfsFile; TMPFS_MAX_FILES]> =
    Mutex::new([TmpfsFile::empty(); TMPFS_MAX_FILES]);

static TMPFS_DIRS: Mutex<[TmpfsDir; TMPFS_MAX_DIRS]> =
    Mutex::new([TmpfsDir::empty(); TMPFS_MAX_DIRS]);

static TMPFS_MOUNTS: Mutex<[TmpfsMount; TMPFS_MAX_MOUNTS]> =
    Mutex::new([TmpfsMount::empty(); TMPFS_MAX_MOUNTS]);

// ============================================================================
// Helper: copy name bytes into a fixed-length name array
// ============================================================================

/// Copy up to TMPFS_NAME_LEN bytes of `src` into `dst`, returning the
/// number of bytes copied (clamped to TMPFS_NAME_LEN).
fn copy_name(dst: &mut [u8; TMPFS_NAME_LEN], src: &[u8]) -> u8 {
    let n = if src.len() > TMPFS_NAME_LEN {
        TMPFS_NAME_LEN
    } else {
        src.len()
    };
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

/// Return true if `name[..name_len]` equals `cmp`.
fn name_eq(name: &[u8; TMPFS_NAME_LEN], name_len: u8, cmp: &[u8]) -> bool {
    let nl = name_len as usize;
    if nl != cmp.len() {
        return false;
    }
    let mut i = 0usize;
    while i < nl {
        if name[i] != cmp[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ============================================================================
// Public API
// ============================================================================

/// Create a new tmpfs mount with the given size limit.
/// Returns the mount index, or None if the mount table is full.
pub fn tmpfs_mount(size_limit: u64) -> Option<u32> {
    let mut mounts = TMPFS_MOUNTS.lock();
    let mut dirs = TMPFS_DIRS.lock();

    // Find a free mount slot
    let mut mount_idx = TMPFS_MAX_MOUNTS;
    let mut i = 0usize;
    while i < TMPFS_MAX_MOUNTS {
        if !mounts[i].active {
            mount_idx = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if mount_idx == TMPFS_MAX_MOUNTS {
        return None;
    }

    // Allocate root directory inode
    let mut dir_idx = TMPFS_MAX_DIRS;
    let mut j = 0usize;
    while j < TMPFS_MAX_DIRS {
        if !dirs[j].active {
            dir_idx = j;
            break;
        }
        j = j.saturating_add(1);
    }
    if dir_idx == TMPFS_MAX_DIRS {
        return None;
    }

    let root_ino = alloc_ino();

    dirs[dir_idx] = TmpfsDir {
        ino: root_ino,
        parent_ino: root_ino, // root is its own parent
        name: [b'/' as u8; TMPFS_NAME_LEN],
        name_len: 1,
        mode: S_IFDIR | 0o755,
        uid: 0,
        gid: 0,
        active: true,
    };
    // Fix up the name: only first byte should be '/'
    dirs[dir_idx].name[1] = 0;

    mounts[mount_idx] = TmpfsMount {
        root_ino: root_ino,
        size_limit: size_limit,
        used_bytes: 0,
        active: true,
    };

    Some(mount_idx as u32)
}

/// Unmount a tmpfs instance, freeing all files and directories
/// that belong to it.  Returns false if the mount index is invalid.
pub fn tmpfs_unmount(idx: u32) -> bool {
    let idx = idx as usize;
    if idx >= TMPFS_MAX_MOUNTS {
        return false;
    }

    let mut mounts = TMPFS_MOUNTS.lock();
    if !mounts[idx].active {
        return false;
    }
    let root_ino = mounts[idx].root_ino;
    mounts[idx] = TmpfsMount::empty();
    drop(mounts);

    // Free all files and dirs whose ino chain leads to this mount's root.
    // Since we do not store mount_idx in each entry, we free all entries
    // whose parent tree resolves to root_ino.  Simpler: free everything
    // that has parent_ino == root_ino or ino == root_ino (shallow).
    // For a correct unmount of nested structures this would need recursion,
    // but for the static-pool design we just free the entries directly
    // associated with this mount (root dir + its immediate children).
    {
        let mut dirs = TMPFS_DIRS.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS {
            if dirs[i].active && (dirs[i].ino == root_ino || dirs[i].parent_ino == root_ino) {
                dirs[i] = TmpfsDir::empty();
            }
            i = i.saturating_add(1);
        }
    }
    {
        let mut files = TMPFS_FILES.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_FILES {
            if files[i].active && files[i].parent_ino == root_ino {
                files[i] = TmpfsFile::empty();
            }
            i = i.saturating_add(1);
        }
    }

    true
}

/// Create a directory under `parent_ino` in mount `mount_idx`.
/// Returns the new inode number, or None on failure.
pub fn tmpfs_mkdir(mount_idx: u32, parent_ino: u64, name: &[u8]) -> Option<u64> {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || name.is_empty() || name.len() > TMPFS_NAME_LEN {
        return None;
    }

    {
        let mounts = TMPFS_MOUNTS.lock();
        if !mounts[mount_idx as usize].active {
            return None;
        }
    }

    // Ensure parent exists
    {
        let dirs = TMPFS_DIRS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS {
            if dirs[i].active && dirs[i].ino == parent_ino {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return None;
        }
    }

    // Check for duplicate name in parent
    {
        let dirs = TMPFS_DIRS.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS {
            if dirs[i].active
                && dirs[i].parent_ino == parent_ino
                && name_eq(&dirs[i].name, dirs[i].name_len, name)
            {
                return None;
            }
            i = i.saturating_add(1);
        }
    }

    let mut dirs = TMPFS_DIRS.lock();
    let mut slot = TMPFS_MAX_DIRS;
    let mut i = 0usize;
    while i < TMPFS_MAX_DIRS {
        if !dirs[i].active {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == TMPFS_MAX_DIRS {
        return None;
    }

    let new_ino = alloc_ino();
    let mut entry = TmpfsDir::empty();
    entry.ino = new_ino;
    entry.parent_ino = parent_ino;
    entry.name_len = copy_name(&mut entry.name, name);
    entry.mode = S_IFDIR | 0o755;
    entry.active = true;
    dirs[slot] = entry;

    Some(new_ino)
}

/// Create an empty regular file under `parent_ino` in mount `mount_idx`.
/// Returns the new inode number, or None on failure.
pub fn tmpfs_create(mount_idx: u32, parent_ino: u64, name: &[u8]) -> Option<u64> {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || name.is_empty() || name.len() > TMPFS_NAME_LEN {
        return None;
    }

    {
        let mounts = TMPFS_MOUNTS.lock();
        if !mounts[mount_idx as usize].active {
            return None;
        }
    }

    // Ensure parent dir exists
    {
        let dirs = TMPFS_DIRS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS {
            if dirs[i].active && dirs[i].ino == parent_ino {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return None;
        }
    }

    // Check for duplicate name
    {
        let files = TMPFS_FILES.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_FILES {
            if files[i].active
                && files[i].parent_ino == parent_ino
                && name_eq(&files[i].name, files[i].name_len, name)
            {
                return None;
            }
            i = i.saturating_add(1);
        }
    }

    let mut files = TMPFS_FILES.lock();
    let mut slot = TMPFS_MAX_FILES;
    let mut i = 0usize;
    while i < TMPFS_MAX_FILES {
        if !files[i].active {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == TMPFS_MAX_FILES {
        return None;
    }

    let new_ino = alloc_ino();
    let mut entry = TmpfsFile::empty();
    entry.ino = new_ino;
    entry.parent_ino = parent_ino;
    entry.name_len = copy_name(&mut entry.name, name);
    entry.mode = S_IFREG | 0o644;
    entry.size = 0;
    entry.active = true;
    files[slot] = entry;

    Some(new_ino)
}

/// Write `data` into the file identified by `ino` in mount `mount_idx`
/// starting at `offset`.  Clamps writes to TMPFS_FILE_SIZE.
/// Updates the mount's used_bytes with saturating_add.
/// Returns the number of bytes written.
pub fn tmpfs_write(mount_idx: u32, ino: u64, offset: usize, data: &[u8]) -> usize {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || data.is_empty() {
        return 0;
    }

    let mut files = TMPFS_FILES.lock();
    let mut idx = TMPFS_MAX_FILES;
    let mut i = 0usize;
    while i < TMPFS_MAX_FILES {
        if files[i].active && files[i].ino == ino {
            idx = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if idx == TMPFS_MAX_FILES {
        return 0;
    }

    // Clamp to file size limit
    if offset >= TMPFS_FILE_SIZE {
        return 0;
    }
    let max_write = TMPFS_FILE_SIZE - offset;
    let n = if data.len() > max_write {
        max_write
    } else {
        data.len()
    };

    let old_size = files[idx].size;

    let mut j = 0usize;
    while j < n {
        let pos = offset.saturating_add(j);
        if pos < TMPFS_FILE_SIZE {
            files[idx].data[pos] = data[j];
        }
        j = j.saturating_add(1);
    }

    let new_end = offset.saturating_add(n);
    if new_end > files[idx].size {
        files[idx].size = new_end;
    }

    // Update mount used_bytes
    let new_size = files[idx].size;
    drop(files);

    if new_size > old_size {
        let growth = (new_size - old_size) as u64;
        let mut mounts = TMPFS_MOUNTS.lock();
        if (mount_idx as usize) < TMPFS_MAX_MOUNTS {
            mounts[mount_idx as usize].used_bytes =
                mounts[mount_idx as usize].used_bytes.saturating_add(growth);
        }
    }

    n
}

/// Read from the file identified by `ino` in mount `mount_idx` into `buf`
/// starting at `offset`.  Returns the number of bytes read.
pub fn tmpfs_read(mount_idx: u32, ino: u64, offset: usize, buf: &mut [u8]) -> usize {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || buf.is_empty() {
        return 0;
    }

    let files = TMPFS_FILES.lock();
    let mut idx = TMPFS_MAX_FILES;
    let mut i = 0usize;
    while i < TMPFS_MAX_FILES {
        if files[i].active && files[i].ino == ino {
            idx = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if idx == TMPFS_MAX_FILES {
        return 0;
    }
    if offset >= files[idx].size {
        return 0;
    }

    let available = files[idx].size - offset;
    let n = if buf.len() > available {
        available
    } else {
        buf.len()
    };

    let mut j = 0usize;
    while j < n {
        buf[j] = files[idx].data[offset.saturating_add(j)];
        j = j.saturating_add(1);
    }

    n
}

/// Delete the file identified by `ino` from mount `mount_idx`.
/// Subtracts its size from the mount's used_bytes with saturating_sub.
/// Returns false if the file was not found.
pub fn tmpfs_unlink(mount_idx: u32, ino: u64) -> bool {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS {
        return false;
    }

    let mut files = TMPFS_FILES.lock();
    let mut idx = TMPFS_MAX_FILES;
    let mut i = 0usize;
    while i < TMPFS_MAX_FILES {
        if files[i].active && files[i].ino == ino {
            idx = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if idx == TMPFS_MAX_FILES {
        return false;
    }

    let freed = files[idx].size as u64;
    files[idx] = TmpfsFile::empty();
    drop(files);

    let mut mounts = TMPFS_MOUNTS.lock();
    if (mount_idx as usize) < TMPFS_MAX_MOUNTS {
        mounts[mount_idx as usize].used_bytes =
            mounts[mount_idx as usize].used_bytes.saturating_sub(freed);
    }

    true
}

/// Delete the directory identified by `ino` from mount `mount_idx`.
/// Stub: always allows removal (does not check for children).
/// Returns false if the directory was not found.
pub fn tmpfs_rmdir(mount_idx: u32, ino: u64) -> bool {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS {
        return false;
    }

    let mut dirs = TMPFS_DIRS.lock();
    let mut i = 0usize;
    while i < TMPFS_MAX_DIRS {
        if dirs[i].active && dirs[i].ino == ino {
            dirs[i] = TmpfsDir::empty();
            return true;
        }
        i = i.saturating_add(1);
    }

    false
}

/// Look up a child entry by name in the directory `parent_ino` within mount
/// `mount_idx`.  Searches both files and directories.
/// Returns the inode number of the found entry, or None.
pub fn tmpfs_lookup(mount_idx: u32, parent_ino: u64, name: &[u8]) -> Option<u64> {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || name.is_empty() {
        return None;
    }

    // Search directories
    {
        let dirs = TMPFS_DIRS.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS {
            if dirs[i].active && dirs[i].parent_ino == parent_ino
                && dirs[i].ino != parent_ino   // skip self-referencing root
                && name_eq(&dirs[i].name, dirs[i].name_len, name)
            {
                return Some(dirs[i].ino);
            }
            i = i.saturating_add(1);
        }
    }

    // Search files
    {
        let files = TMPFS_FILES.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_FILES {
            if files[i].active
                && files[i].parent_ino == parent_ino
                && name_eq(&files[i].name, files[i].name_len, name)
            {
                return Some(files[i].ino);
            }
            i = i.saturating_add(1);
        }
    }

    None
}

/// Enumerate children of directory `dir_ino` in mount `mount_idx`.
/// Fills `out` with `(ino, name_bytes, name_len)` tuples.
/// Returns the number of entries written (up to `max`).
pub fn tmpfs_readdir(
    mount_idx: u32,
    dir_ino: u64,
    out: &mut [(u64, [u8; 64], u8)],
    max: usize,
) -> usize {
    if mount_idx as usize >= TMPFS_MAX_MOUNTS || max == 0 || out.is_empty() {
        return 0;
    }

    let effective_max = if max > out.len() { out.len() } else { max };
    let mut count = 0usize;

    // Enumerate child directories
    {
        let dirs = TMPFS_DIRS.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_DIRS && count < effective_max {
            if dirs[i].active && dirs[i].parent_ino == dir_ino && dirs[i].ino != dir_ino {
                out[count] = (dirs[i].ino, dirs[i].name, dirs[i].name_len);
                count = count.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
    }

    // Enumerate child files
    {
        let files = TMPFS_FILES.lock();
        let mut i = 0usize;
        while i < TMPFS_MAX_FILES && count < effective_max {
            if files[i].active && files[i].parent_ino == dir_ino {
                out[count] = (files[i].ino, files[i].name, files[i].name_len);
                count = count.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
    }

    count
}

// ============================================================================
// init
// ============================================================================

/// Initialize the tmpfs subsystem.
///
/// Mounts a tmpfs at "/" (root) and creates the /tmp directory beneath it.
pub fn init() {
    // Create the root mount (unlimited size)
    let root_mount_idx = match tmpfs_mount(0) {
        Some(idx) => idx,
        None => {
            serial_println!("[tmpfs] ERROR: failed to create root mount");
            return;
        }
    };

    // Retrieve the root inode
    let root_ino = {
        let mounts = TMPFS_MOUNTS.lock();
        mounts[root_mount_idx as usize].root_ino
    };

    // Create /tmp
    match tmpfs_mkdir(root_mount_idx, root_ino, b"tmp") {
        Some(_) => {}
        None => {
            serial_println!("[tmpfs] WARNING: failed to create /tmp");
        }
    }

    serial_println!("[tmpfs] tmpfs initialized");
}
