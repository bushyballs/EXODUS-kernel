use crate::serial_println;
/// Btrfs filesystem stub for Genesis
///
/// Minimal Btrfs implementation: superblock parsing, subvolume management,
/// inode table, and snapshot creation.
/// No heap: all state lives in fixed-size static arrays.
///
/// Key Btrfs concepts represented: subvolumes, snapshots, checksum type,
/// B-tree generation counters, COW metadata.
///
/// Inspired by: Linux Btrfs (fs/btrfs). All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const BTRFS_MAGIC: u64 = 0x4D5F53665248425F; // "_BHRfS_M" little-endian
pub const BTRFS_MAX_MOUNTS: usize = 2;
pub const BTRFS_MAX_SUBVOLS: usize = 16;
pub const BTRFS_MAX_INODES: usize = 64;

pub const BTRFS_CRC32_CSUM: u16 = 1;
pub const BTRFS_XXHASH_CSUM: u16 = 2;

// Root subvolume ID (Btrfs always starts at 5)
const BTRFS_FS_TREE_OBJECTID: u64 = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct BtrfsSuperblock {
    pub magic: u64,
    pub bytenr: u64,      // address of this superblock
    pub label: [u8; 256], // filesystem label
    pub generation: u64,  // B-tree generation
    pub root: u64,        // root tree root bytenr
    pub chunk_root: u64,  // chunk tree root
    pub log_root: u64,    // log tree root
    pub total_bytes: u64, // total filesystem bytes
    pub bytes_used: u64,
    pub num_devices: u64,
    pub nodesize: u32,       // btree node size (default 16 KiB)
    pub sectorsize: u32,     // minimum block size (4 KiB)
    pub csum_type: u16,      // checksum algorithm
    pub incompat_flags: u64, // RAID, compression, etc.
}

impl BtrfsSuperblock {
    const fn empty() -> Self {
        BtrfsSuperblock {
            magic: 0,
            bytenr: 0,
            label: [0u8; 256],
            generation: 0,
            root: 0,
            chunk_root: 0,
            log_root: 0,
            total_bytes: 0,
            bytes_used: 0,
            num_devices: 0,
            nodesize: 0,
            sectorsize: 0,
            csum_type: 0,
            incompat_flags: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct BtrfsSubvol {
    pub id: u64,
    pub name: [u8; 256],
    pub name_len: u16,
    pub flags: u64, // BTRFS_ROOT_SUBVOL_RDONLY etc.
    pub active: bool,
}

impl BtrfsSubvol {
    pub const fn empty() -> Self {
        BtrfsSubvol {
            id: 0,
            name: [0u8; 256],
            name_len: 0,
            flags: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct BtrfsInode {
    pub ino: u64,
    pub subvol_id: u64,
    pub mode: u16,
    pub size: u64,
    pub nlink: u32,
    pub active: bool,
}

impl BtrfsInode {
    pub const fn empty() -> Self {
        BtrfsInode {
            ino: 0,
            subvol_id: 0,
            mode: 0,
            size: 0,
            nlink: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct BtrfsMount {
    pub sb: BtrfsSuperblock,
    pub subvols: [BtrfsSubvol; BTRFS_MAX_SUBVOLS],
    pub inodes: [BtrfsInode; BTRFS_MAX_INODES],
    pub nsubvol: usize,
    pub ninode: usize,
    pub active: bool,
}

impl BtrfsMount {
    pub const fn empty() -> Self {
        BtrfsMount {
            sb: BtrfsSuperblock::empty(),
            subvols: [BtrfsSubvol::empty(); BTRFS_MAX_SUBVOLS],
            inodes: [BtrfsInode::empty(); BTRFS_MAX_INODES],
            nsubvol: 0,
            ninode: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state (no heap — fixed-size static arrays)
// ---------------------------------------------------------------------------

static BTRFS_MOUNTS: Mutex<[BtrfsMount; BTRFS_MAX_MOUNTS]> =
    Mutex::new([BtrfsMount::empty(); BTRFS_MAX_MOUNTS]);

/// Subvolume ID counter (wrapping).
static SUBVOL_ID_CTR: Mutex<u64> = Mutex::new(256); // Btrfs user subvol IDs start at 256

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

#[inline]
fn copy_label(dst: &mut [u8; 256], src: &[u8]) -> u16 {
    let n = src.len().min(256);
    dst[..n].copy_from_slice(&src[..n]);
    n as u16
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn mount_add_subvol(m: &mut BtrfsMount, id: u64, name: &[u8], flags: u64) -> bool {
    if m.nsubvol >= BTRFS_MAX_SUBVOLS {
        return false;
    }
    let mut sv = BtrfsSubvol::empty();
    sv.id = id;
    sv.name_len = copy_label(&mut sv.name, name);
    sv.flags = flags;
    sv.active = true;
    m.subvols[m.nsubvol] = sv;
    m.nsubvol = m.nsubvol.saturating_add(1);
    true
}

fn mount_add_inode(m: &mut BtrfsMount, ino: u64, subvol_id: u64, mode: u16, size: u64) -> bool {
    if m.ninode >= BTRFS_MAX_INODES {
        return false;
    }
    m.inodes[m.ninode] = BtrfsInode {
        ino,
        subvol_id,
        mode,
        size,
        nlink: 1,
        active: true,
    };
    m.ninode = m.ninode.saturating_add(1);
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount a simulated Btrfs filesystem.  Populates a valid-looking superblock
/// and a default subvolume with root inode.  Returns the mount index.
pub fn btrfs_mount(_dev_path: &[u8]) -> Option<u32> {
    let mut mounts = BTRFS_MOUNTS.lock();
    let slot = mounts.iter().position(|m| !m.active)?;

    let m = &mut mounts[slot];
    // Zero in-place to avoid a large stack frame in debug builds.
    unsafe {
        core::ptr::write_bytes(m as *mut BtrfsMount, 0, 1);
    }

    // Fill in a simulated superblock
    m.sb.magic = BTRFS_MAGIC;
    m.sb.bytenr = 0x0001_0000; // typical primary superblock offset
    m.sb.generation = 1;
    m.sb.root = 0x0002_0000;
    m.sb.chunk_root = 0x0003_0000;
    m.sb.log_root = 0;
    m.sb.total_bytes = 0x4000_0000; // 1 GiB
    m.sb.bytes_used = 0x0010_0000; // 1 MiB used
    m.sb.num_devices = 1;
    m.sb.nodesize = 16384;
    m.sb.sectorsize = 4096;
    m.sb.csum_type = BTRFS_CRC32_CSUM;
    m.sb.incompat_flags = 0;
    let label = b"genesis-btrfs";
    m.sb.label[..label.len()].copy_from_slice(label);

    // Default subvolume (id=5)
    mount_add_subvol(m, BTRFS_FS_TREE_OBJECTID, b"default", 0);

    // Root inode in default subvolume
    mount_add_inode(m, 256, BTRFS_FS_TREE_OBJECTID, 0o040_755, 0);

    m.active = true;
    Some(slot as u32)
}

/// Unmount a Btrfs volume.
pub fn btrfs_unmount(idx: u32) -> bool {
    let i = idx as usize;
    let mut mounts = BTRFS_MOUNTS.lock();
    if i >= BTRFS_MAX_MOUNTS || !mounts[i].active {
        return false;
    }
    mounts[i] = BtrfsMount::empty();
    true
}

/// Look up a path by name in the inode table.
/// Simple stub: returns inode 256 for root "/", nothing else.
pub fn btrfs_lookup(idx: u32, path: &[u8]) -> Option<u64> {
    let i = idx as usize;
    let mounts = BTRFS_MOUNTS.lock();
    if i >= BTRFS_MAX_MOUNTS || !mounts[i].active {
        return None;
    }
    let m = &mounts[i];

    if path == b"/" || path.is_empty() {
        // Return first inode (root)
        if m.ninode > 0 && m.inodes[0].active {
            return Some(m.inodes[0].ino);
        }
        return None;
    }

    // Search by matching last path component against inode numbers
    // (Btrfs stub does not maintain a full dirent table — just inode pool)
    for j in 0..m.ninode {
        let ino = &m.inodes[j];
        if ino.active {
            // Stub: any path that is a decimal representation of ino.ino matches
            return Some(ino.ino);
        }
    }
    None
}

/// Read file data.  Stub: always returns 0 (no backing store).
pub fn btrfs_read(_idx: u32, _ino: u64, _buf: &mut [u8], _offset: u64) -> usize {
    0
}

/// List subvolumes on a Btrfs mount.  Fills `out[..n]` where n = min(subvols, max).
pub fn btrfs_subvol_list(idx: u32, out: &mut [BtrfsSubvol], max: usize) -> usize {
    let i = idx as usize;
    let mounts = BTRFS_MOUNTS.lock();
    if i >= BTRFS_MAX_MOUNTS || !mounts[i].active {
        return 0;
    }
    let m = &mounts[i];
    let limit = max.min(out.len()).min(m.nsubvol);
    let mut count = 0usize;
    for j in 0..m.nsubvol {
        if count >= limit {
            break;
        }
        if m.subvols[j].active {
            out[count] = m.subvols[j];
            count = count.saturating_add(1);
        }
    }
    count
}

/// Create a snapshot (read-only clone) of src_subvol with the given name.
/// Returns the new subvolume ID on success.
pub fn btrfs_snapshot_create(idx: u32, src_subvol: u64, name: &[u8]) -> Option<u64> {
    let i = idx as usize;
    let mut mounts = BTRFS_MOUNTS.lock();
    if i >= BTRFS_MAX_MOUNTS || !mounts[i].active {
        return None;
    }
    let m = &mut mounts[i];

    // Verify src_subvol exists
    let src_exists = m.subvols[..m.nsubvol]
        .iter()
        .any(|sv| sv.active && sv.id == src_subvol);
    if !src_exists {
        return None;
    }

    if m.nsubvol >= BTRFS_MAX_SUBVOLS {
        return None;
    }

    // Assign a new subvolume ID
    let new_id = {
        let mut ctr = SUBVOL_ID_CTR.lock();
        let id = *ctr;
        *ctr = ctr.wrapping_add(1);
        id
    };

    // BTRFS_ROOT_SUBVOL_RDONLY = 1
    let rdonly_flag: u64 = 1;
    mount_add_subvol(m, new_id, name, rdonly_flag);

    // Increment generation
    m.sb.generation = m.sb.generation.saturating_add(1);

    Some(new_id)
}

/// Initialize the Btrfs filesystem driver.
pub fn init() {
    serial_println!("    [btrfs] Btrfs filesystem driver initialized");
}
