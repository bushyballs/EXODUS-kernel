/// SquashFS read-only filesystem driver for Genesis
///
/// SquashFS is a compressed, read-only filesystem used in live-CDs, embedded
/// systems, and container images (snap packages, Docker layers, initramfs).
///
/// On-disk format (version 4.0 only):
///   Superblock (96 bytes at offset 0)
///   Compressed data blocks
///   Fragment table (compressed tail fragments)
///   Inode table (compressed inode metadata)
///   Directory table (compressed directory entries)
///   UID/GID lookup table
///   (optional xattr, export tables)
///
/// Kernel constraints obeyed:
///   - No heap: no Vec, Box, String, alloc::* — fixed-size static arrays only
///   - No floats: no f32/f64 literals or casts
///   - No panics: no unwrap/expect/panic — return Option/bool
///   - Saturating arithmetic on counters
///   - Division guarded (divisor != 0 checks)
///   - Structs in Mutex must be Copy + const fn empty()
///
/// Inspired by: Linux squashfs driver, mksquashfs. All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// SquashFS magic number (little-endian bytes: 's','q','s','h' = 0x73717368).
pub const SQUASHFS_MAGIC: u32 = 0x7371_7368;

/// Default data block size (128 KiB).
pub const SQUASHFS_BLOCK_SIZE: usize = 131072;

/// Maximum number of simultaneously mounted SquashFS images.
pub const SQUASHFS_MAX_MOUNTS: usize = 2;

/// Maximum number of inodes tracked per mount.
pub const SQUASHFS_MAX_INODES: usize = 128;

/// Maximum number of directory entries tracked per mount.
pub const SQUASHFS_MAX_DIRENTS: usize = 256;

/// Maximum file/directory name length (including null terminator space).
pub const SQUASHFS_NAME_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Inode type constants (NIST SquashFS 4.0)
// ---------------------------------------------------------------------------

/// Inode type: directory.
pub const SQ_ITYPE_DIR: u16 = 1;
/// Inode type: regular file.
pub const SQ_ITYPE_FILE: u16 = 2;
/// Inode type: symbolic link.
pub const SQ_ITYPE_SYMLINK: u16 = 3;
/// Inode type: block device.
pub const SQ_ITYPE_BLKDEV: u16 = 4;
/// Inode type: character device.
pub const SQ_ITYPE_CHRDEV: u16 = 5;
/// Inode type: FIFO (named pipe).
pub const SQ_ITYPE_FIFO: u16 = 6;
/// Inode type: Unix socket.
pub const SQ_ITYPE_SOCKET: u16 = 7;

// ---------------------------------------------------------------------------
// Superblock
// ---------------------------------------------------------------------------

/// SquashFS 4.0 superblock (mirrors the on-disk layout, key fields only).
///
/// The actual on-disk superblock is 96 bytes; we store the fields we need.
#[derive(Copy, Clone)]
pub struct SquashfsSuperblock {
    /// Must equal `SQUASHFS_MAGIC` (0x73717368) for a valid image.
    pub s_magic: u32,
    /// Total number of inodes in the image.
    pub inodes: u32,
    /// Image creation timestamp (seconds since Unix epoch).
    pub mkfs_time: u32,
    /// Data block size in bytes (typically 131072 = 128 KiB).
    pub block_size: u32,
    /// Number of fragment table entries.
    pub fragments: u32,
    /// Compression algorithm ID: 1=gzip, 2=lzma, 3=lzo, 4=xz, 5=lz4, 6=zstd.
    pub compression: u16,
    /// log2(block_size); e.g. 17 for 128 KiB blocks.
    pub block_log: u16,
    /// Feature flags bitmap.
    pub flags: u16,
    /// Number of unique (uid, gid) pairs in the ID table.
    pub no_ids: u16,
    /// Filesystem major version — must be 4 for SquashFS 4.0.
    pub version_major: u16,
    /// Filesystem minor version — must be 0 for SquashFS 4.0.
    pub version_minor: u16,
    /// Packed reference to the root directory inode (block << 16 | offset).
    pub root_inode: u64,
    /// Total number of bytes used by the image (including superblock).
    pub bytes_used: u64,
}

impl SquashfsSuperblock {
    /// Return a zeroed-out, inactive superblock (required for static Mutex).
    pub const fn empty() -> Self {
        SquashfsSuperblock {
            s_magic: 0,
            inodes: 0,
            mkfs_time: 0,
            block_size: 0,
            fragments: 0,
            compression: 0,
            block_log: 0,
            flags: 0,
            no_ids: 0,
            version_major: 0,
            version_minor: 0,
            root_inode: 0,
            bytes_used: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Inode
// ---------------------------------------------------------------------------

/// Simplified in-memory inode for a SquashFS entry.
#[derive(Copy, Clone)]
pub struct SqInode {
    /// Inode number (1-based; 0 = unused slot).
    pub ino_num: u32,
    /// Inode type (SQ_ITYPE_*).
    pub inode_type: u16,
    /// POSIX permission bits (e.g. 0o755).
    pub mode: u16,
    /// Index into the UID table.
    pub uid_idx: u16,
    /// Index into the GID table.
    pub gid_idx: u16,
    /// Modification timestamp (seconds since Unix epoch).
    pub mtime: u32,
    /// File size in bytes (meaningful only for SQ_ITYPE_FILE).
    pub file_size: u64,
    /// Slot is in use.
    pub active: bool,
}

impl SqInode {
    /// Return an empty, inactive inode slot (required for static Mutex arrays).
    pub const fn empty() -> Self {
        SqInode {
            ino_num: 0,
            inode_type: 0,
            mode: 0,
            uid_idx: 0,
            gid_idx: 0,
            mtime: 0,
            file_size: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Directory entry
// ---------------------------------------------------------------------------

/// A single directory entry inside a SquashFS directory.
#[derive(Copy, Clone)]
pub struct SqDirent {
    /// Inode number of the referenced file/directory.
    pub ino_num: u32,
    /// UTF-8 (or raw byte) name, zero-terminated or up to `name_len`.
    pub name: [u8; SQUASHFS_NAME_LEN],
    /// Meaningful bytes in `name` (not counting any null terminator).
    pub name_len: u16,
    /// Entry type (SQ_ITYPE_*).
    pub entry_type: u16,
    /// Slot is in use.
    pub active: bool,
}

impl SqDirent {
    /// Return an empty, inactive directory-entry slot.
    pub const fn empty() -> Self {
        SqDirent {
            ino_num: 0,
            name: [0u8; SQUASHFS_NAME_LEN],
            name_len: 0,
            entry_type: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Mount descriptor
// ---------------------------------------------------------------------------

/// State for a single mounted SquashFS image.
#[derive(Copy, Clone)]
pub struct SqMount {
    /// Parsed superblock.
    pub sb: SquashfsSuperblock,
    /// Fixed pool of parsed inodes.
    pub inodes: [SqInode; SQUASHFS_MAX_INODES],
    /// Flat pool of directory entries for all directories in this mount.
    pub dirents: [SqDirent; SQUASHFS_MAX_DIRENTS],
    /// Number of active inode slots.
    pub ninode: usize,
    /// Number of active directory-entry slots.
    pub ndirent: usize,
    /// Mount slot is occupied.
    pub active: bool,
}

impl SqMount {
    /// Return an empty, inactive mount slot.
    pub const fn empty() -> Self {
        SqMount {
            sb: SquashfsSuperblock::empty(),
            inodes: [SqInode::empty(); SQUASHFS_MAX_INODES],
            dirents: [SqDirent::empty(); SQUASHFS_MAX_DIRENTS],
            ninode: 0,
            ndirent: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global mount table
// ---------------------------------------------------------------------------

static SQFS_MOUNTS: Mutex<[SqMount; SQUASHFS_MAX_MOUNTS]> =
    Mutex::new([SqMount::empty(); SQUASHFS_MAX_MOUNTS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy at most `SQUASHFS_NAME_LEN - 1` bytes from a byte slice into a fixed
/// name array and return the number of bytes copied.
fn copy_name(dst: &mut [u8; SQUASHFS_NAME_LEN], src: &[u8]) -> u16 {
    let max = if src.len() < SQUASHFS_NAME_LEN {
        src.len()
    } else {
        SQUASHFS_NAME_LEN - 1
    };
    let mut i = 0usize;
    while i < max {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    max as u16
}

/// Compare a fixed name array (first `name_len` bytes) with a byte slice.
/// Returns `true` if they are equal.
fn name_eq(name: &[u8; SQUASHFS_NAME_LEN], name_len: u16, other: &[u8]) -> bool {
    if name_len as usize != other.len() {
        return false;
    }
    let mut i = 0usize;
    while i < name_len as usize {
        if name[i] != other[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

/// Add an inode to a mount, returning the inode number on success, 0 on full.
fn mount_add_inode(
    m: &mut SqMount,
    ino_num: u32,
    inode_type: u16,
    mode: u16,
    file_size: u64,
) -> u32 {
    if m.ninode >= SQUASHFS_MAX_INODES {
        return 0;
    }
    let idx = m.ninode;
    m.inodes[idx] = SqInode {
        ino_num,
        inode_type,
        mode,
        uid_idx: 0,
        gid_idx: 0,
        mtime: 0,
        file_size,
        active: true,
    };
    m.ninode = m.ninode.saturating_add(1);
    ino_num
}

/// Add a directory entry to a mount. Returns `true` on success.
fn mount_add_dirent(m: &mut SqMount, ino_num: u32, name: &[u8], entry_type: u16) -> bool {
    if m.ndirent >= SQUASHFS_MAX_DIRENTS {
        return false;
    }
    let idx = m.ndirent;
    let mut d = SqDirent::empty();
    d.ino_num = ino_num;
    d.name_len = copy_name(&mut d.name, name);
    d.entry_type = entry_type;
    d.active = true;
    m.dirents[idx] = d;
    m.ndirent = m.ndirent.saturating_add(1);
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount a SquashFS image identified by a device path byte-string.
///
/// This implementation is a structural stub — it validates a synthetic
/// superblock and populates a minimal filesystem tree (root, /lib, /lib/libc.so)
/// to demonstrate the driver framework. Real block-device I/O would decode the
/// actual on-disk structures.
///
/// # Returns
/// `Some(mount_index)` on success, `None` if the mount table is full or the
/// device path is empty.
pub fn squashfs_mount(dev_path: &[u8]) -> Option<u32> {
    if dev_path.is_empty() {
        return None;
    }

    let mut mounts = SQFS_MOUNTS.lock();

    // Find a free slot.
    let mut slot = SQUASHFS_MAX_MOUNTS;
    let mut i = 0usize;
    while i < SQUASHFS_MAX_MOUNTS {
        if !mounts[i].active {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == SQUASHFS_MAX_MOUNTS {
        return None; // All slots occupied.
    }

    let m = &mut mounts[slot];
    // Zero in-place to avoid materialising a ~72 KB SqMount on the stack.
    unsafe {
        core::ptr::write_bytes(m as *mut SqMount, 0, 1);
    }

    // Populate a synthetic SquashFS 4.0 superblock.
    m.sb = SquashfsSuperblock {
        s_magic: SQUASHFS_MAGIC,
        inodes: 3,
        mkfs_time: 0,
        block_size: 131072,
        fragments: 0,
        compression: 1, // gzip
        block_log: 17,
        flags: 0,
        no_ids: 1,
        version_major: 4,
        version_minor: 0,
        root_inode: 1,
        bytes_used: 4096,
    };

    // inode 1: root directory "/"
    mount_add_inode(m, 1, SQ_ITYPE_DIR, 0o755, 0);
    // inode 2: directory "/lib"
    mount_add_inode(m, 2, SQ_ITYPE_DIR, 0o755, 0);
    // inode 3: regular file "/lib/libc.so"
    mount_add_inode(m, 3, SQ_ITYPE_FILE, 0o644, 0);

    // Directory entries for root (inode 1): contains "lib"
    mount_add_dirent(m, 2, b"lib", SQ_ITYPE_DIR);
    // Directory entries for /lib (inode 2): contains "libc.so"
    mount_add_dirent(m, 3, b"libc.so", SQ_ITYPE_FILE);

    m.active = true;

    serial_println!(
        "    [squashfs] mounted slot {} ({} inodes, {} dirents)",
        slot,
        m.ninode,
        m.ndirent
    );

    Some(slot as u32)
}

/// Unmount a SquashFS image by its mount index.
///
/// # Returns
/// `true` if the slot was active and has been cleared; `false` otherwise.
pub fn squashfs_unmount(idx: u32) -> bool {
    if idx as usize >= SQUASHFS_MAX_MOUNTS {
        return false;
    }
    let mut mounts = SQFS_MOUNTS.lock();
    if !mounts[idx as usize].active {
        return false;
    }
    mounts[idx as usize] = SqMount::empty();
    serial_println!("    [squashfs] unmounted slot {}", idx);
    true
}

/// Resolve an absolute path (e.g. `/lib/libc.so`) to its inode number.
///
/// Path components are separated by `/`. An empty path or bare `/` resolves
/// to the root inode (always inode 1 in this driver).
///
/// # Returns
/// `Some(ino_num)` on success, `None` if any component is not found.
pub fn squashfs_lookup(idx: u32, path: &[u8]) -> Option<u32> {
    if idx as usize >= SQUASHFS_MAX_MOUNTS {
        return None;
    }
    let mounts = SQFS_MOUNTS.lock();
    let m = &mounts[idx as usize];
    if !m.active {
        return None;
    }

    // Empty path or "/" → root inode (1).
    if path.is_empty() || (path.len() == 1 && path[0] == b'/') {
        return Some(1);
    }

    // Walk path components split on '/'.
    // We track the current inode number through each directory step.
    let mut current_ino: u32 = 1; // start at root

    // Iterate over path bytes, extracting slash-delimited components.
    let mut comp_start = 0usize;
    // Skip leading slash.
    if comp_start < path.len() && path[comp_start] == b'/' {
        comp_start = comp_start.saturating_add(1);
    }

    while comp_start < path.len() {
        // Find end of this component.
        let mut comp_end = comp_start;
        while comp_end < path.len() && path[comp_end] != b'/' {
            comp_end = comp_end.saturating_add(1);
        }
        if comp_end == comp_start {
            // Double slash or trailing slash — skip.
            comp_start = comp_end.saturating_add(1);
            continue;
        }

        let component = &path[comp_start..comp_end];

        // Search directory entries whose parent would be current_ino.
        // In our flat model we use a heuristic: entries follow root at idx 0
        // and /lib at idx 1. We find the entry matching the name.
        // The driver maps: root (ino 1) contains dirent[0] ("lib", ino 2).
        //                  /lib (ino 2) contains dirent[1] ("libc.so", ino 3).
        //
        // For a real driver we would look up the directory inode and scan its
        // associated entries. Here we use the flat dirent pool with a simple
        // policy: dirents belonging to `current_ino` are those whose ino_num
        // is the *next* inode in sequence from what the parent has.  To keep
        // this stub self-consistent, we scan ALL dirents for a name match and
        // ensure the result is reachable from current_ino.

        let found_ino = find_dirent_in_dir(m, current_ino, component);
        match found_ino {
            Some(ino) => current_ino = ino,
            None => return None,
        }

        comp_start = comp_end.saturating_add(1);
    }

    Some(current_ino)
}

/// Internal: search the flat dirent pool for a name within directory `dir_ino`.
///
/// Because this is a minimal stub with a simple tree (root → lib → libc.so),
/// we apply a straightforward reachability rule:
///   - inode 1 (root): may reach dirent with name "lib"   (inode 2)
///   - inode 2 (/lib): may reach dirent with name "libc.so" (inode 3)
/// In a full driver this would decode the on-disk directory table.
fn find_dirent_in_dir(m: &SqMount, dir_ino: u32, name: &[u8]) -> Option<u32> {
    // Determine the dirent range that belongs to this directory.
    // In our stub: root (1) owns dirent[0], /lib (2) owns dirent[1].
    // We search all active dirents and pick one whose ino_num is consistent.
    let mut i = 0usize;
    while i < m.ndirent {
        let d = &m.dirents[i];
        if d.active && name_eq(&d.name, d.name_len, name) {
            // Verify that the parent directory is `dir_ino`.
            // Our simple heuristic: dirent at index (dir_ino - 1) belongs to dir_ino.
            // dir_ino 1 → index 0, dir_ino 2 → index 1.
            let expected_idx = dir_ino.wrapping_sub(1) as usize;
            if i == expected_idx {
                return Some(d.ino_num);
            }
        }
        i = i.saturating_add(1);
    }
    None
}

/// Read directory entries for a given directory inode.
///
/// Fills up to `max` entries into `out` and returns the actual count written.
/// Returns 0 if the mount index is invalid, the mount is inactive, or the
/// inode is not a directory.
pub fn squashfs_readdir(idx: u32, dir_ino: u32, out: &mut [SqDirent], max: usize) -> usize {
    if idx as usize >= SQUASHFS_MAX_MOUNTS || max == 0 {
        return 0;
    }
    let mounts = SQFS_MOUNTS.lock();
    let m = &mounts[idx as usize];
    if !m.active {
        return 0;
    }

    // Verify that dir_ino is actually a directory.
    if squashfs_inode_type(m, dir_ino) != SQ_ITYPE_DIR {
        return 0;
    }

    // Collect dirents whose parent directory is dir_ino.
    // Using the same heuristic as find_dirent_in_dir.
    let expected_idx = dir_ino.wrapping_sub(1) as usize;
    let mut count = 0usize;

    // Only one dirent per directory in this stub.
    if expected_idx < m.ndirent && count < max && count < out.len() {
        let d = &m.dirents[expected_idx];
        if d.active {
            out[count] = *d;
            count = count.saturating_add(1);
        }
    }
    count
}

/// Return the inode type for the given ino_num within a mount, or 0 if not found.
fn squashfs_inode_type(m: &SqMount, ino_num: u32) -> u16 {
    let mut i = 0usize;
    while i < m.ninode {
        if m.inodes[i].active && m.inodes[i].ino_num == ino_num {
            return m.inodes[i].inode_type;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Retrieve inode metadata for a given inode number.
///
/// # Returns
/// `Some(SqInode)` on success, `None` if the mount or inode is not found.
pub fn squashfs_getattr(idx: u32, ino: u32) -> Option<SqInode> {
    if idx as usize >= SQUASHFS_MAX_MOUNTS {
        return None;
    }
    let mounts = SQFS_MOUNTS.lock();
    let m = &mounts[idx as usize];
    if !m.active {
        return None;
    }
    let mut i = 0usize;
    while i < m.ninode {
        if m.inodes[i].active && m.inodes[i].ino_num == ino {
            return Some(m.inodes[i]);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Read file data from a SquashFS file inode into `buf` starting at byte `offset`.
///
/// This stub returns 0 because compressed data blocks are not stored in the
/// in-memory mount structure (decompression requires a working compression
/// library and raw disk I/O, neither of which is available in the kernel stub).
///
/// In a full implementation this function would:
///   1. Locate the inode's start_block and block_size_list.
///   2. Read and decompress each required data block.
///   3. Copy the requested byte range into `buf`.
///
/// # Returns
/// Number of bytes written to `buf` (always 0 in this stub).
pub fn squashfs_read(idx: u32, ino: u32, buf: &mut [u8], offset: u64) -> usize {
    // Silence unused-variable warnings for stub parameters.
    let _ = (idx, ino, buf, offset);
    0
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the SquashFS driver.
///
/// Mounts a synthetic SquashFS image to demonstrate the subsystem:
///   /         (directory, inode 1)
///   /lib       (directory, inode 2)
///   /lib/libc.so (regular file, inode 3)
pub fn init() {
    match squashfs_mount(b"/dev/squashfs0") {
        Some(idx) => {
            // Verify /lib/libc.so is reachable.
            let ino = squashfs_lookup(idx, b"/lib/libc.so");
            match ino {
                Some(n) => {
                    serial_println!(
                        "    [squashfs] SquashFS read-only filesystem initialized (mount={}, /lib/libc.so=ino{})",
                        idx, n
                    );
                }
                None => {
                    serial_println!(
                        "    [squashfs] SquashFS read-only filesystem initialized (mount={}, lookup failed)",
                        idx
                    );
                }
            }
        }
        None => {
            serial_println!(
                "    [squashfs] SquashFS read-only filesystem initialized (no free slot)"
            );
        }
    }
}
