/// NFS v3 client — stub, no-heap implementation
///
/// NFS (Network File System) uses RPC over UDP/TCP (RFC 1813 for v3).
/// This is a stub that exposes the full NFS client API surface without
/// performing actual network I/O.  All operations complete successfully
/// with synthesised handles and zero-filled data buffers.
///
/// Rules enforced:
///   - No Vec, Box, String, alloc::*
///   - No f32 / f64 casts
///   - No unwrap() / expect() / panic!()
///   - Saturating counters, wrapping sequence numbers
///   - All statics hold Copy types with const fn empty()
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_NFS_MOUNTS: usize = 4;
pub const MAX_NFS_FILES: usize = 32;
pub const NFS_FHSIZE: usize = 32;
pub const NFS_MAXPATH: usize = 256;
pub const NFS_PORT: u16 = 2049;

// NFS status codes
pub const NFS_OK: i32 = 0;
pub const NFSERR_PERM: i32 = 1;
pub const NFSERR_NOENT: i32 = 2;
pub const NFSERR_IO: i32 = 5;

// NFS file types
pub const NFREG: u8 = 1;
pub const NFDIR: u8 = 2;
pub const NFLNK: u8 = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An NFS file handle (opaque 32-byte blob assigned by the server).
#[derive(Clone, Copy)]
pub struct NfsFh {
    pub data: [u8; NFS_FHSIZE],
}

impl NfsFh {
    pub const fn zero() -> Self {
        NfsFh {
            data: [0u8; NFS_FHSIZE],
        }
    }
}

/// File attributes returned by NFS GETATTR / LOOKUP.
#[derive(Clone, Copy)]
pub struct NfsAttr {
    /// File type: NFREG, NFDIR, NFLNK, …
    pub ftype: u8,
    /// UNIX permission bits (e.g. 0o755)
    pub mode: u32,
    /// Hard-link count
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    /// File size in bytes
    pub size: u64,
    /// Last-access time (seconds since epoch)
    pub atime: u64,
    /// Last-modification time
    pub mtime: u64,
    /// Last-status-change time
    pub ctime: u64,
}

impl NfsAttr {
    pub const fn empty() -> Self {
        NfsAttr {
            ftype: NFREG,
            mode: 0o644,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
        }
    }
}

/// An active NFS mount point.
#[derive(Clone, Copy)]
pub struct NfsMount {
    pub id: u32,
    pub server_ip: [u8; 4],
    /// Export path as raw bytes, zero-padded to NFS_MAXPATH.
    pub export_path: [u8; NFS_MAXPATH],
    pub path_len: u8,
    pub root_fh: NfsFh,
    /// Local mount point, zero-padded to 64 bytes.
    pub mount_point: [u8; 64],
    pub mp_len: u8,
    pub connected: bool,
    pub active: bool,
}

impl NfsMount {
    pub const fn empty() -> Self {
        NfsMount {
            id: 0,
            server_ip: [0u8; 4],
            export_path: [0u8; NFS_MAXPATH],
            path_len: 0,
            root_fh: NfsFh::zero(),
            mount_point: [0u8; 64],
            mp_len: 0,
            connected: false,
            active: false,
        }
    }
}

/// An open NFS file descriptor.
#[derive(Clone, Copy)]
pub struct NfsOpenFile {
    pub mount_id: u32,
    pub fh: NfsFh,
    pub attr: NfsAttr,
    /// Current byte offset into the file.
    pub file_offset: u64,
    /// Open flags (e.g. O_RDONLY, O_RDWR)
    pub flags: u32,
    pub active: bool,
}

impl NfsOpenFile {
    pub const fn empty() -> Self {
        NfsOpenFile {
            mount_id: 0,
            fh: NfsFh::zero(),
            attr: NfsAttr::empty(),
            file_offset: 0,
            flags: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

static NFS_MOUNTS: Mutex<[NfsMount; MAX_NFS_MOUNTS]> =
    Mutex::new([NfsMount::empty(); MAX_NFS_MOUNTS]);

static NFS_FILES: Mutex<[NfsOpenFile; MAX_NFS_FILES]> =
    Mutex::new([NfsOpenFile::empty(); MAX_NFS_FILES]);

/// Monotonically increasing mount ID counter.
static NEXT_MOUNT_ID: crate::sync::Mutex<u32> = crate::sync::Mutex::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Compute a simple 32-byte hash of up to 256 input bytes.
/// Spread across all 32 output slots using a FNV-1a-inspired mix.
/// No floats, no division by zero, fully safe.
fn hash_bytes_to_fh(inputs: &[&[u8]]) -> NfsFh {
    let mut fh = NfsFh::zero();
    let mut h: u32 = 0x811c_9dc5; // FNV offset basis
    let mut slot: usize = 0;

    for slice in inputs {
        let mut i = 0usize;
        while i < slice.len() {
            h ^= slice[i] as u32;
            h = h.wrapping_mul(0x0100_0193); // FNV prime
                                             // Write one byte into the handle, cycling through all 32 slots
            fh.data[slot % NFS_FHSIZE] = fh.data[slot % NFS_FHSIZE].wrapping_add((h & 0xFF) as u8);
            slot = slot.wrapping_add(1);
            i = i.saturating_add(1);
        }
    }

    // Second pass: spread remaining entropy across every slot
    let mut j = 0usize;
    while j < NFS_FHSIZE {
        h = h.wrapping_mul(0x0100_0193).wrapping_add(j as u32);
        fh.data[j] ^= (h >> 8) as u8;
        j = j.saturating_add(1);
    }

    fh
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount an NFS export.
///
/// This is a stub implementation: always succeeds and sets `connected = true`.
/// The root file handle is synthesised from a hash of the server IP and export
/// path, so repeated calls with the same arguments return the same handle.
///
/// Returns the mount ID on success, or `None` if the mount table is full.
pub fn nfs_mount(server_ip: [u8; 4], export_path: &[u8], mount_point: &[u8]) -> Option<u32> {
    let mut mounts = NFS_MOUNTS.lock();

    // Find a free slot
    let mut slot: Option<usize> = None;
    let mut i = 0usize;
    while i < MAX_NFS_MOUNTS {
        if !mounts[i].active {
            slot = Some(i);
            break;
        }
        i = i.saturating_add(1);
    }

    let idx = slot?;

    // Assign mount ID
    let mount_id = {
        let mut id_lock = NEXT_MOUNT_ID.lock();
        let id = *id_lock;
        *id_lock = id_lock.saturating_add(1);
        id
    };

    // Build root file handle: hash(server_ip || export_path)
    let root_fh = hash_bytes_to_fh(&[&server_ip, export_path]);

    // Populate the mount entry
    let m = &mut mounts[idx];
    m.id = mount_id;
    m.server_ip = server_ip;
    m.connected = true;
    m.active = true;
    m.root_fh = root_fh;

    // Copy export path (truncate to NFS_MAXPATH)
    let plen = export_path.len().min(NFS_MAXPATH);
    let mut k = 0usize;
    while k < plen {
        m.export_path[k] = export_path[k];
        k = k.saturating_add(1);
    }
    m.path_len = plen as u8;

    // Copy mount point (truncate to 64)
    let mplen = mount_point.len().min(64);
    let mut k = 0usize;
    while k < mplen {
        m.mount_point[k] = mount_point[k];
        k = k.saturating_add(1);
    }
    m.mp_len = mplen as u8;

    serial_println!(
        "[nfs_client] mount id={} server={}.{}.{}.{} ok (stub)",
        mount_id,
        server_ip[0],
        server_ip[1],
        server_ip[2],
        server_ip[3]
    );

    Some(mount_id)
}

/// Unmount a previously mounted NFS export.
///
/// Returns `true` on success, `false` if `mount_id` was not found.
pub fn nfs_umount(mount_id: u32) -> bool {
    let mut mounts = NFS_MOUNTS.lock();
    let mut i = 0usize;
    while i < MAX_NFS_MOUNTS {
        if mounts[i].active && mounts[i].id == mount_id {
            mounts[i] = NfsMount::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Look up a path on a mounted export.
///
/// Stub: synthesises a file handle from the path bytes and returns `NFS_OK`.
/// The attributes describe a regular file of size 0 with mode 0o644.
pub fn nfs_lookup(mount_id: u32, path: &[u8], fh_out: &mut NfsFh, attr_out: &mut NfsAttr) -> i32 {
    // Verify mount exists
    {
        let mounts = NFS_MOUNTS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_NFS_MOUNTS {
            if mounts[i].active && mounts[i].id == mount_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return NFSERR_NOENT;
        }
    }

    // Synthesise a file handle from the path
    let mount_id_bytes = mount_id.to_be_bytes();
    *fh_out = hash_bytes_to_fh(&[&mount_id_bytes, path]);
    *attr_out = NfsAttr::empty();

    NFS_OK
}

/// Open an NFS file by its file handle.
///
/// Returns the file descriptor index on success, or `None` if the file table
/// is full or the mount does not exist.
pub fn nfs_open(mount_id: u32, fh: &NfsFh, flags: u32) -> Option<u32> {
    // Verify mount exists
    {
        let mounts = NFS_MOUNTS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_NFS_MOUNTS {
            if mounts[i].active && mounts[i].id == mount_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return None;
        }
    }

    let mut files = NFS_FILES.lock();
    let mut slot: Option<usize> = None;
    let mut i = 0usize;
    while i < MAX_NFS_FILES {
        if !files[i].active {
            slot = Some(i);
            break;
        }
        i = i.saturating_add(1);
    }

    let idx = slot?;

    files[idx].mount_id = mount_id;
    files[idx].fh = *fh;
    files[idx].attr = NfsAttr::empty();
    files[idx].file_offset = 0;
    files[idx].flags = flags;
    files[idx].active = true;

    Some(idx as u32)
}

/// Close an open NFS file descriptor.
///
/// Returns `true` on success, `false` if `fd` was not valid.
pub fn nfs_close(fd: u32) -> bool {
    let idx = fd as usize;
    if idx >= MAX_NFS_FILES {
        return false;
    }
    let mut files = NFS_FILES.lock();
    if !files[idx].active {
        return false;
    }
    files[idx] = NfsOpenFile::empty();
    true
}

/// Read up to `len` bytes from an NFS file starting at `offset`.
///
/// Stub: fills `buf[..len]` with zeros and returns `len as isize`.
/// Returns -1 if `fd` is invalid, -5 (NFSERR_IO) if len > 4096.
pub fn nfs_read(fd: u32, offset: u64, buf: &mut [u8; 4096], len: usize) -> isize {
    let idx = fd as usize;
    if idx >= MAX_NFS_FILES {
        return -1;
    }
    {
        let files = NFS_FILES.lock();
        if !files[idx].active {
            return -1;
        }
    }
    if len > 4096 {
        return -(NFSERR_IO as isize);
    }

    // Zero-fill the requested region (stub)
    let fill = len.min(4096);
    let mut i = 0usize;
    while i < fill {
        buf[i] = 0;
        i = i.saturating_add(1);
    }

    // Update file offset (ignore `offset` parameter — stub is stateless)
    {
        let mut files = NFS_FILES.lock();
        if files[idx].active {
            files[idx].file_offset = offset.saturating_add(len as u64);
        }
    }

    fill as isize
}

/// Write `len` bytes to an NFS file at `offset`.
///
/// Stub: discards the data and returns `len as isize` to indicate success.
/// Also advances the file's internal offset.
pub fn nfs_write(fd: u32, offset: u64, _data: &[u8], len: usize) -> isize {
    let idx = fd as usize;
    if idx >= MAX_NFS_FILES {
        return -1;
    }
    let mut files = NFS_FILES.lock();
    if !files[idx].active {
        return -1;
    }
    // Advance offset
    files[idx].file_offset = offset.saturating_add(len as u64);
    len as isize
}

/// Retrieve the attributes of an open NFS file.
///
/// Returns `Some(NfsAttr)` on success, `None` if `fd` is invalid.
pub fn nfs_getattr(fd: u32) -> Option<NfsAttr> {
    let idx = fd as usize;
    if idx >= MAX_NFS_FILES {
        return None;
    }
    let files = NFS_FILES.lock();
    if !files[idx].active {
        return None;
    }
    Some(files[idx].attr)
}

/// Read directory entries from an NFS directory file handle.
///
/// Stub: returns 0 entries and `NFS_OK`.
pub fn nfs_readdir(
    mount_id: u32,
    _fh: &NfsFh,
    _out_names: &mut [[u8; 64]; 32],
    out_count: &mut u8,
) -> i32 {
    // Verify mount exists
    let mounts = NFS_MOUNTS.lock();
    let mut found = false;
    let mut i = 0usize;
    while i < MAX_NFS_MOUNTS {
        if mounts[i].active && mounts[i].id == mount_id {
            found = true;
            break;
        }
        i = i.saturating_add(1);
    }
    drop(mounts);

    if !found {
        return NFSERR_NOENT;
    }

    *out_count = 0;
    NFS_OK
}

/// Initialise the NFS client subsystem.
pub fn init() {
    serial_println!("[nfs_client] NFS v3 client initialized (stub)");
}
