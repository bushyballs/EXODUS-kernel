use crate::serial_println;
/// ramfs — pure memory-backed filesystem for Genesis AIOS
///
/// Unlike tmpfs, ramfs has no size limit and pages are never evicted or
/// swapped out.  It is used for:
///   - /sys     (sysfs layer mounts here)
///   - /run     (early-boot transient state)
///   - initrd   (boot-time file injection)
///   - any data that must survive until the system powers off but
///     is intentionally volatile (lost on shutdown/reboot).
///
/// Design:
///   - A single static pool of `RAMFS_MAX_NODES` nodes (avoids heap
///     allocation; `no_std` safe).
///   - Each node carries up to `RAMFS_DATA_SIZE` bytes of inline data.
///     For files larger than this limit, writes are truncated.
///   - Inode numbers are allocated from a monotonically-incrementing
///     counter starting at `RAMFS_INO_BASE`.
///   - Parent-child relationships are tracked by `parent_inode`; a
///     directory's "contents" are found by scanning for nodes whose
///     `parent_inode` matches.
///   - `name` is a fixed-length `[u8; RAMFS_NAME_MAX]` to avoid `alloc`.
///
/// Limitations (intentional, for bare-metal simplicity):
///   - No per-node permissions enforcement (all callers are kernel-mode).
///   - No hard-link support (nlink is tracked but always 1 for files).
///   - Name lookup is O(n) linear scan over the node pool.
///   - Maximum RAMFS_MAX_NODES entries total across all mounted ramfs
///     instances (single global pool).
///
/// Inspired by: Linux ramfs (fs/ramfs/), Plan 9 ramfs.  All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Maximum number of nodes (files + directories) in the global ramfs pool.
pub const RAMFS_MAX_NODES: usize = 4096;

/// Maximum inline data bytes per node.
pub const RAMFS_DATA_SIZE: usize = 4096;

/// Maximum filename length (including null terminator, not counted).
pub const RAMFS_NAME_MAX: usize = 256;

/// Inode numbers for ramfs start here to avoid collisions with memfs/procfs.
const RAMFS_INO_BASE: u64 = 0x0010_0000;

/// Root inode number for the ramfs root directory.
pub const RAMFS_ROOT_INO: u64 = RAMFS_INO_BASE;

/// Null / unused inode sentinel.
const RAMFS_INO_NONE: u64 = 0;

// ---------------------------------------------------------------------------
// Node definition
// ---------------------------------------------------------------------------

/// A single ramfs node (file or directory).
///
/// Stored inline in a global static array — no heap allocation required.
pub struct RamfsNode {
    /// Inode number.  `RAMFS_INO_NONE` means this slot is free.
    pub inode: u64,
    /// Inode number of the containing directory.
    pub parent_inode: u64,
    /// Entry name (UTF-8, null-padded, NOT null-terminated in the array).
    pub name: [u8; RAMFS_NAME_MAX],
    /// Actual length of the name (bytes).
    pub name_len: usize,
    /// Inline file data.
    pub data: [u8; RAMFS_DATA_SIZE],
    /// Number of valid bytes in `data`.
    pub size: usize,
    /// `true` → directory, `false` → regular file.
    pub is_dir: bool,
    /// Permission mode bits (Unix-style, e.g. 0o755).
    pub mode: u16,
    /// Hard-link count (1 for regular files, 2 for directories).
    pub nlink: u32,
    /// Last access time (Unix timestamp).
    pub atime: u64,
    /// Last modification time.
    pub mtime: u64,
    /// Last status-change time.
    pub ctime: u64,
    /// Slot is in use.
    pub valid: bool,
}

impl RamfsNode {
    /// Return a blank, un-used node.
    const fn empty() -> Self {
        RamfsNode {
            inode: RAMFS_INO_NONE,
            parent_inode: RAMFS_INO_NONE,
            name: [0u8; RAMFS_NAME_MAX],
            name_len: 0,
            data: [0u8; RAMFS_DATA_SIZE],
            size: 0,
            is_dir: false,
            mode: 0,
            nlink: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            valid: false,
        }
    }

    /// Return the name as a `&str` (best-effort; empty string on UTF-8 error).
    #[inline]
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// Write `name` bytes into the fixed-length field.
    fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let n = bytes.len().min(RAMFS_NAME_MAX);
        self.name[..n].copy_from_slice(&bytes[..n]);
        // Zero-pad the rest
        for b in self.name[n..].iter_mut() {
            *b = 0;
        }
        self.name_len = n;
    }
}

// ---------------------------------------------------------------------------
// Global pool
// ---------------------------------------------------------------------------

/// The global node pool.  A `Mutex<...>` guards concurrent access.
static RAMFS_NODES: Mutex<[RamfsNode; RAMFS_MAX_NODES]> =
    Mutex::new([const { RamfsNode::empty() }; RAMFS_MAX_NODES]);

/// Inode counter.
static RAMFS_NEXT_INO: Mutex<u64> = Mutex::new(RAMFS_INO_BASE + 1);

/// Whether `ramfs_init()` has been called.
static RAMFS_READY: Mutex<bool> = Mutex::new(false);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate a fresh inode number.
fn alloc_ino() -> u64 {
    let mut ctr = RAMFS_NEXT_INO.lock();
    let ino = *ctr;
    *ctr = ctr.saturating_add(1);
    ino
}

/// Find a free slot index in the pool.  Returns `None` if full.
fn find_free_slot(pool: &[RamfsNode]) -> Option<usize> {
    pool.iter().position(|n| !n.valid)
}

/// Find the slot index for `inode`.  Returns `None` if not found.
fn find_by_ino(pool: &[RamfsNode], inode: u64) -> Option<usize> {
    if inode == RAMFS_INO_NONE {
        return None;
    }
    pool.iter().position(|n| n.valid && n.inode == inode)
}

/// Find a child of `parent_inode` with the given `name`.
fn find_child(pool: &[RamfsNode], parent_inode: u64, name: &str) -> Option<usize> {
    let name_bytes = name.as_bytes();
    let name_len = name_bytes.len();
    pool.iter().position(|n| {
        n.valid
            && n.parent_inode == parent_inode
            && n.name_len == name_len
            && &n.name[..n.name_len] == name_bytes
    })
}

/// Return the current Unix timestamp (falls back to 0 if clock unavailable).
#[inline]
fn now() -> u64 {
    crate::time::clock::unix_time()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the ramfs subsystem.
///
/// Creates the root directory node.  Must be called once before any other
/// ramfs function.  Calling it a second time is a no-op.
pub fn ramfs_init() {
    let mut ready = RAMFS_READY.lock();
    if *ready {
        return;
    }
    let mut pool = RAMFS_NODES.lock();
    let slot = match find_free_slot(&*pool) {
        Some(s) => s,
        None => {
            serial_println!("  ramfs: init failed — node pool exhausted");
            return;
        }
    };
    let ts = now();
    pool[slot].inode = RAMFS_ROOT_INO;
    pool[slot].parent_inode = RAMFS_ROOT_INO; // root is its own parent
    pool[slot].set_name("/");
    pool[slot].size = 0;
    pool[slot].is_dir = true;
    pool[slot].mode = 0o755;
    pool[slot].nlink = 2;
    pool[slot].atime = ts;
    pool[slot].mtime = ts;
    pool[slot].ctime = ts;
    pool[slot].valid = true;
    *ready = true;
    serial_println!(
        "  ramfs: initialized (pool={} nodes, {} bytes/node)",
        RAMFS_MAX_NODES,
        RAMFS_DATA_SIZE
    );
}

/// Return the mount-point handle (root inode number).
///
/// `ramfs_init()` must have been called first.
#[inline]
pub fn ramfs_mount() -> u64 {
    RAMFS_ROOT_INO
}

/// Create a regular file inside `parent` directory.
///
/// Returns the new inode number on success, or a negative errno.
///   -17 (EEXIST)  — name already exists in parent
///   -28 (ENOSPC)  — node pool is full
///   -20 (ENOTDIR) — parent inode is not a directory
///   -2  (ENOENT)  — parent inode does not exist
pub fn ramfs_create(parent: u64, name: &str, mode: u16) -> Result<u64, i32> {
    if name.is_empty() || name.len() >= RAMFS_NAME_MAX {
        return Err(-22); // EINVAL
    }
    let mut pool = RAMFS_NODES.lock();

    // Verify parent exists and is a directory.
    let parent_idx = find_by_ino(&*pool, parent).ok_or(-2i32)?;
    if !pool[parent_idx].is_dir {
        return Err(-20); // ENOTDIR
    }

    // Check for name collision.
    if find_child(&*pool, parent, name).is_some() {
        return Err(-17); // EEXIST
    }

    let slot = find_free_slot(&*pool).ok_or(-28i32)?; // ENOSPC
    let ino = alloc_ino();
    let ts = now();

    pool[slot].inode = ino;
    pool[slot].parent_inode = parent;
    pool[slot].set_name(name);
    pool[slot].size = 0;
    pool[slot].is_dir = false;
    pool[slot].mode = mode;
    pool[slot].nlink = 1;
    pool[slot].atime = ts;
    pool[slot].mtime = ts;
    pool[slot].ctime = ts;
    pool[slot].valid = true;

    // Increment parent link count (each child file increments it by 0
    // for files; only subdirs would increment by 1 — we track nlink
    // on dirs as 2 + number of subdirectories).
    pool[parent_idx].mtime = ts;

    Ok(ino)
}

/// Create a directory inside `parent`.
///
/// Returns the new inode number on success, or a negative errno.
pub fn ramfs_mkdir(parent: u64, name: &str, mode: u16) -> Result<u64, i32> {
    if name.is_empty() || name.len() >= RAMFS_NAME_MAX {
        return Err(-22); // EINVAL
    }
    let mut pool = RAMFS_NODES.lock();

    let parent_idx = find_by_ino(&*pool, parent).ok_or(-2i32)?;
    if !pool[parent_idx].is_dir {
        return Err(-20); // ENOTDIR
    }

    if find_child(&*pool, parent, name).is_some() {
        return Err(-17); // EEXIST
    }

    let slot = find_free_slot(&*pool).ok_or(-28i32)?;
    let ino = alloc_ino();
    let ts = now();

    pool[slot].inode = ino;
    pool[slot].parent_inode = parent;
    pool[slot].set_name(name);
    pool[slot].size = 0;
    pool[slot].is_dir = true;
    pool[slot].mode = mode;
    pool[slot].nlink = 2; // "." and ".."
    pool[slot].atime = ts;
    pool[slot].mtime = ts;
    pool[slot].ctime = ts;
    pool[slot].valid = true;

    // Parent gains one more hard-link (..) from the new subdir.
    pool[parent_idx].nlink = pool[parent_idx].nlink.saturating_add(1);
    pool[parent_idx].mtime = ts;

    Ok(ino)
}

/// Read up to `buf.len()` bytes from a file inode starting at `offset`.
///
/// Returns the number of bytes actually copied.  Returns 0 if:
///   - `offset` is at or past EOF
///   - `buf` is empty
///   - `inode` does not exist or is a directory
pub fn ramfs_read(inode: u64, offset: usize, buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let pool = RAMFS_NODES.lock();
    let idx = match find_by_ino(&*pool, inode) {
        Some(i) => i,
        None => return 0,
    };
    if pool[idx].is_dir {
        return 0;
    }

    let size = pool[idx].size;
    if offset >= size {
        return 0;
    }

    let available = size - offset;
    let n = available.min(buf.len());
    buf[..n].copy_from_slice(&pool[idx].data[offset..offset + n]);
    n
}

/// Write `data` into a file inode starting at `offset`.
///
/// The file is extended if `offset + data.len() > current size`, up to
/// `RAMFS_DATA_SIZE`.  Any bytes that would exceed `RAMFS_DATA_SIZE` are
/// silently dropped.
///
/// Returns the number of bytes actually written (may be less than `data.len()`
/// if the node is full).
pub fn ramfs_write(inode: u64, offset: usize, data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    let mut pool = RAMFS_NODES.lock();
    let idx = match find_by_ino(&*pool, inode) {
        Some(i) => i,
        None => return 0,
    };
    if pool[idx].is_dir {
        return 0;
    }

    // Clamp to available capacity.
    if offset >= RAMFS_DATA_SIZE {
        return 0;
    }
    let capacity = RAMFS_DATA_SIZE - offset;
    let n = data.len().min(capacity);
    if n == 0 {
        return 0;
    }

    pool[idx].data[offset..offset + n].copy_from_slice(&data[..n]);

    let new_end = offset + n;
    if new_end > pool[idx].size {
        pool[idx].size = new_end;
    }

    let ts = now();
    pool[idx].mtime = ts;
    pool[idx].atime = ts;
    n
}

/// Look up a name inside a directory inode.
///
/// Returns the child's inode number, or `None` if not found.
pub fn ramfs_lookup(parent: u64, name: &str) -> Option<u64> {
    let pool = RAMFS_NODES.lock();
    find_child(&*pool, parent, name).map(|idx| pool[idx].inode)
}

/// Remove a regular file from its parent directory.
///
/// Returns `true` on success.  Does NOT support removing non-empty
/// directories (use `ramfs_rmdir` instead).
pub fn ramfs_unlink(parent: u64, name: &str) -> bool {
    let mut pool = RAMFS_NODES.lock();
    let child_idx = match find_child(&*pool, parent, name) {
        Some(i) => i,
        None => return false,
    };
    if pool[child_idx].is_dir {
        return false;
    } // use rmdir for directories

    pool[child_idx].valid = false;
    pool[child_idx].inode = RAMFS_INO_NONE;
    pool[child_idx].size = 0;

    // Update parent mtime.
    if let Some(pidx) = find_by_ino(&*pool, parent) {
        pool[pidx].mtime = now();
    }
    true
}

/// Remove an empty directory from its parent.
///
/// Returns `true` on success, `false` if the directory is non-empty,
/// does not exist, or the named entry is not a directory.
pub fn ramfs_rmdir(parent: u64, name: &str) -> bool {
    let mut pool = RAMFS_NODES.lock();
    let child_idx = match find_child(&*pool, parent, name) {
        Some(i) => i,
        None => return false,
    };
    if !pool[child_idx].is_dir {
        return false;
    }

    let child_ino = pool[child_idx].inode;

    // Ensure directory is empty (no child nodes with parent_inode == child_ino).
    let has_children = pool.iter().any(|n| n.valid && n.parent_inode == child_ino);
    if has_children {
        return false;
    }

    pool[child_idx].valid = false;
    pool[child_idx].inode = RAMFS_INO_NONE;

    // Decrement parent link count (loses the ".." back-reference).
    if let Some(pidx) = find_by_ino(&*pool, parent) {
        pool[pidx].nlink = pool[pidx].nlink.saturating_sub(1);
        pool[pidx].mtime = now();
    }
    true
}

/// Read directory entries for `inode`.
///
/// Fills `out` with `(child_inode, name_array)` pairs and returns the
/// number of entries written.  At most 64 entries are returned per call.
pub fn ramfs_readdir(inode: u64, out: &mut [(u64, [u8; RAMFS_NAME_MAX]); 64]) -> usize {
    let pool = RAMFS_NODES.lock();

    // Verify inode is a valid directory.
    match find_by_ino(&*pool, inode) {
        Some(idx) if pool[idx].is_dir => {}
        _ => return 0,
    }

    let mut count = 0usize;
    for node in pool.iter() {
        if count >= 64 {
            break;
        }
        if !node.valid {
            continue;
        }
        if node.parent_inode != inode {
            continue;
        }
        // Skip the root's self-reference.
        if node.inode == RAMFS_ROOT_INO && inode == RAMFS_ROOT_INO {
            continue;
        }

        out[count].0 = node.inode;
        out[count].1 = node.name;
        count += 1;
    }
    count
}

/// Truncate a file to `new_size` bytes.
///
/// If `new_size` is larger than the current size the file is zero-extended
/// (up to `RAMFS_DATA_SIZE`).  Returns the new size.
pub fn ramfs_truncate(inode: u64, new_size: usize) -> usize {
    let mut pool = RAMFS_NODES.lock();
    let idx = match find_by_ino(&*pool, inode) {
        Some(i) => i,
        None => return 0,
    };
    if pool[idx].is_dir {
        return 0;
    }

    let clamped = new_size.min(RAMFS_DATA_SIZE);
    let cur_size = pool[idx].size;
    if clamped > cur_size {
        // Zero-fill the gap.
        for b in pool[idx].data[cur_size..clamped].iter_mut() {
            *b = 0;
        }
    }
    pool[idx].size = clamped;
    let ts = now();
    pool[idx].mtime = ts;
    pool[idx].ctime = ts;
    clamped
}

/// Return basic metadata for an inode: `(is_dir, size, mode, nlink)`.
///
/// Returns `None` if the inode does not exist.
pub fn ramfs_stat(inode: u64) -> Option<(bool, usize, u16, u32)> {
    let pool = RAMFS_NODES.lock();
    find_by_ino(&*pool, inode).map(|idx| {
        (
            pool[idx].is_dir,
            pool[idx].size,
            pool[idx].mode,
            pool[idx].nlink,
        )
    })
}

/// Change the permission mode of an inode.
pub fn ramfs_chmod(inode: u64, mode: u16) {
    let mut pool = RAMFS_NODES.lock();
    if let Some(idx) = find_by_ino(&*pool, inode) {
        pool[idx].mode = mode;
        pool[idx].ctime = now();
    }
}

/// Return the number of free node slots remaining.
pub fn ramfs_free_nodes() -> usize {
    let pool = RAMFS_NODES.lock();
    pool.iter().filter(|n| !n.valid).count()
}
