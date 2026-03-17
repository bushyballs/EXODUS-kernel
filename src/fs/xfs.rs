use crate::serial_println;
/// XFS filesystem stub for Genesis
///
/// Minimal XFS superblock parsing and directory traversal stub.
/// No heap: all state lives in fixed-size static arrays.
///
/// Simulates a minimal XFS filesystem at init with:
///   - root inode 128 (XFS root is always inode 128), mode=dir
///   - /etc, /bin, /var dirs
///   - /etc/hostname file
///
/// Inspired by: Linux XFS (fs/xfs). All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const XFS_SB_MAGIC: u32 = 0x58465342; // "XFSB"
pub const XFS_BSIZE: usize = 4096;
pub const XFS_MAX_MOUNTS: usize = 4;
pub const XFS_MAX_INODES: usize = 64;
pub const XFS_MAX_DIRENTS: usize = 128;
pub const XFS_MAX_NAME: usize = 128;

// File mode constants
pub const S_IFDIR: u16 = 0o040_000;
pub const S_IFREG: u16 = 0o100_000;

// Dirent file_type values
pub const DT_FILE: u8 = 1;
pub const DT_DIR: u8 = 2;
pub const DT_SYMLINK: u8 = 3;

// XFS root inode is always 128
pub const XFS_ROOT_INO: u64 = 128;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Simplified XFS on-disk superblock fields (in-memory representation).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct XfsSuperblock {
    pub sb_magicnum: u32,   // XFS_SB_MAGIC
    pub sb_blocksize: u32,  // bytes per block
    pub sb_dblocks: u64,    // number of data blocks
    pub sb_agcount: u32,    // number of allocation groups
    pub sb_sectsize: u16,   // sector size in bytes
    pub sb_inodesize: u16,  // inode size in bytes (256)
    pub sb_inopblock: u16,  // inodes per block
    pub sb_fname: [u8; 12], // filesystem name
    pub sb_icount: u64,     // total inode count
    pub sb_ifree: u64,      // free inode count
    pub sb_fdblocks: u64,   // free data blocks
}

impl XfsSuperblock {
    const fn empty() -> Self {
        XfsSuperblock {
            sb_magicnum: 0,
            sb_blocksize: 0,
            sb_dblocks: 0,
            sb_agcount: 0,
            sb_sectsize: 0,
            sb_inodesize: 0,
            sb_inopblock: 0,
            sb_fname: [0u8; 12],
            sb_icount: 0,
            sb_ifree: 0,
            sb_fdblocks: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct XfsInode {
    pub ino: u64,
    pub mode: u16, // S_IFDIR=0o040000, S_IFREG=0o100000
    pub nlink: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub active: bool,
}

impl XfsInode {
    pub const fn empty() -> Self {
        XfsInode {
            ino: 0,
            mode: 0,
            nlink: 0,
            uid: 0,
            gid: 0,
            size: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct XfsDirent {
    pub ino: u64,
    pub name: [u8; XFS_MAX_NAME],
    pub name_len: u8,
    pub file_type: u8, // DT_FILE=1, DT_DIR=2, DT_SYMLINK=3
    pub active: bool,
}

impl XfsDirent {
    pub const fn empty() -> Self {
        XfsDirent {
            ino: 0,
            name: [0u8; XFS_MAX_NAME],
            name_len: 0,
            file_type: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct XfsMount {
    pub sb: XfsSuperblock,
    pub inodes: [XfsInode; XFS_MAX_INODES],
    pub dirents: [XfsDirent; XFS_MAX_DIRENTS],
    pub ninode: usize,
    pub ndirent: usize,
    pub active: bool,
}

impl XfsMount {
    pub const fn empty() -> Self {
        XfsMount {
            sb: XfsSuperblock::empty(),
            inodes: [XfsInode::empty(); XFS_MAX_INODES],
            dirents: [XfsDirent::empty(); XFS_MAX_DIRENTS],
            ninode: 0,
            ndirent: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state (no heap — fixed-size static arrays)
// ---------------------------------------------------------------------------

static XFS_MOUNTS: Mutex<[XfsMount; XFS_MAX_MOUNTS]> =
    Mutex::new([XfsMount::empty(); XFS_MAX_MOUNTS]);

// ---------------------------------------------------------------------------
// Helper: copy bytes into fixed array, return length
// ---------------------------------------------------------------------------

#[inline]
fn copy_name<const N: usize>(dst: &mut [u8; N], src: &[u8]) -> usize {
    let n = src.len().min(N);
    dst[..n].copy_from_slice(&src[..n]);
    n
}

// ---------------------------------------------------------------------------
// Internal helpers for building the simulated tree
// ---------------------------------------------------------------------------

fn mount_add_inode(m: &mut XfsMount, ino: u64, mode: u16, size: u64, nlink: u16) -> bool {
    if m.ninode >= XFS_MAX_INODES {
        return false;
    }
    m.inodes[m.ninode] = XfsInode {
        ino,
        mode,
        nlink,
        uid: 0,
        gid: 0,
        size,
        atime: 0,
        mtime: 0,
        ctime: 0,
        active: true,
    };
    m.ninode = m.ninode.saturating_add(1);
    true
}

fn mount_add_dirent(m: &mut XfsMount, ino: u64, name: &[u8], file_type: u8) -> bool {
    if m.ndirent >= XFS_MAX_DIRENTS {
        return false;
    }
    let mut d = XfsDirent::empty();
    d.ino = ino;
    let nlen = copy_name(&mut d.name, name);
    d.name_len = nlen.min(255) as u8;
    d.file_type = file_type;
    d.active = true;
    m.dirents[m.ndirent] = d;
    m.ndirent = m.ndirent.saturating_add(1);
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount a simulated XFS filesystem.  Populates a valid-looking superblock and
/// a minimal directory tree.  Returns the mount index on success.
pub fn xfs_mount(_dev_path: &[u8]) -> Option<u32> {
    let mut mounts = XFS_MOUNTS.lock();
    let slot = mounts.iter().position(|m| !m.active)?;

    let m = &mut mounts[slot];
    // Zero in-place to avoid a large stack frame in debug builds.
    unsafe {
        core::ptr::write_bytes(m as *mut XfsMount, 0, 1);
    }

    // Fill in a simulated superblock
    m.sb.sb_magicnum = XFS_SB_MAGIC;
    m.sb.sb_blocksize = XFS_BSIZE as u32;
    m.sb.sb_dblocks = 262144; // 1 GiB / 4 KiB
    m.sb.sb_agcount = 4;
    m.sb.sb_sectsize = 512;
    m.sb.sb_inodesize = 256;
    m.sb.sb_inopblock = (XFS_BSIZE as u16) / 256;
    m.sb.sb_icount = 128;
    m.sb.sb_ifree = 112;
    m.sb.sb_fdblocks = 261000;
    let label = b"genesis-xfs\0";
    m.sb.sb_fname[..12].copy_from_slice(label);

    // Build the minimal directory tree
    // root dir: inode 128
    mount_add_inode(m, 128, S_IFDIR, 0, 4);
    mount_add_dirent(m, 128, b"/", DT_DIR);

    // /etc: inode 129
    mount_add_inode(m, 129, S_IFDIR, 0, 2);
    mount_add_dirent(m, 129, b"etc", DT_DIR);

    // /bin: inode 130
    mount_add_inode(m, 130, S_IFDIR, 0, 2);
    mount_add_dirent(m, 130, b"bin", DT_DIR);

    // /var: inode 131
    mount_add_inode(m, 131, S_IFDIR, 0, 2);
    mount_add_dirent(m, 131, b"var", DT_DIR);

    // /etc/hostname: inode 132
    mount_add_inode(m, 132, S_IFREG, 12, 1);
    mount_add_dirent(m, 132, b"hostname", DT_FILE);

    m.active = true;
    Some(slot as u32)
}

/// Unmount an XFS filesystem by mount index.
pub fn xfs_unmount(mount_idx: u32) -> bool {
    let idx = mount_idx as usize;
    let mut mounts = XFS_MOUNTS.lock();
    if idx >= XFS_MAX_MOUNTS {
        return false;
    }
    if !mounts[idx].active {
        return false;
    }
    mounts[idx] = XfsMount::empty();
    true
}

/// Look up a path component in the dirent table for the given mount.
/// Returns the inode number on success.
///
/// Simple lookup: tries to match the last path component against dirent names.
pub fn xfs_lookup(mount_idx: u32, path: &[u8]) -> Option<u64> {
    let idx = mount_idx as usize;
    let mounts = XFS_MOUNTS.lock();
    if idx >= XFS_MAX_MOUNTS || !mounts[idx].active {
        return None;
    }
    let m = &mounts[idx];

    // Extract last component of path
    let component: &[u8] = {
        let slash = path.iter().rposition(|&b| b == b'/');
        match slash {
            Some(pos) => &path[pos.saturating_add(1)..],
            None => path,
        }
    };

    if component.is_empty() || component == b"/" {
        // Root lookup
        return Some(XFS_ROOT_INO);
    }

    for i in 0..m.ndirent {
        let d = &m.dirents[i];
        if !d.active {
            continue;
        }
        let dname = &d.name[..d.name_len as usize];
        if dname == component {
            return Some(d.ino);
        }
    }
    None
}

/// Read directory entries for a given directory inode.
/// Fills `out[..n]` where n = min(matching dirents, max).
pub fn xfs_readdir(mount_idx: u32, dir_ino: u64, out: &mut [XfsDirent], max: usize) -> usize {
    let idx = mount_idx as usize;
    let mounts = XFS_MOUNTS.lock();
    if idx >= XFS_MAX_MOUNTS || !mounts[idx].active {
        return 0;
    }
    let m = &mounts[idx];

    // Verify the inode is a directory
    let is_dir = m.inodes[..m.ninode]
        .iter()
        .any(|ino| ino.active && ino.ino == dir_ino && (ino.mode & 0o170_000 == S_IFDIR as u16));
    if !is_dir {
        return 0;
    }

    // For the stub: return all dirents associated with the mount that are
    // children of dir_ino.  In a real XFS driver this would be a B+ tree walk.
    // Here, "child" means any dirent whose parent we infer from the tree layout.
    let limit = max.min(out.len());
    let mut count = 0usize;

    for i in 0..m.ndirent {
        if count >= limit {
            break;
        }
        let d = &m.dirents[i];
        if !d.active {
            continue;
        }
        // Root dir (128) contains etc/bin/var; others contain their own files.
        // We approximate by including all dirents when dir_ino == root,
        // or only the matching inode when querying a specific dir.
        if dir_ino == XFS_ROOT_INO {
            // Top-level: etc, bin, var
            if d.ino == 129 || d.ino == 130 || d.ino == 131 {
                out[count] = *d;
                count = count.saturating_add(1);
            }
        } else if d.ino != dir_ino {
            // For any other directory, look for inodes whose "parent" is dir_ino.
            // Simplified: /etc/hostname is a child of /etc (ino 129).
            if dir_ino == 129 && d.ino == 132 {
                out[count] = *d;
                count = count.saturating_add(1);
            }
        }
    }
    count
}

/// Get inode attributes for the given inode number.
pub fn xfs_getattr(mount_idx: u32, ino: u64) -> Option<XfsInode> {
    let idx = mount_idx as usize;
    let mounts = XFS_MOUNTS.lock();
    if idx >= XFS_MAX_MOUNTS || !mounts[idx].active {
        return None;
    }
    let m = &mounts[idx];
    for i in 0..m.ninode {
        if m.inodes[i].active && m.inodes[i].ino == ino {
            return Some(m.inodes[i]);
        }
    }
    None
}

/// Read file data.  Stub: always returns 0 bytes (no backing store).
pub fn xfs_read(_mount_idx: u32, _ino: u64, _buf: &mut [u8], _offset: u64) -> usize {
    0
}

/// Initialize the XFS filesystem driver.
pub fn init() {
    serial_println!("    [xfs] XFS filesystem driver initialized");
}
