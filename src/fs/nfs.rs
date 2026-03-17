use crate::serial_println;
use crate::sync::Mutex;
/// Network File System (NFS) client -- RPC transport, mount protocol, file ops
///
/// Part of the AIOS filesystem layer.
///
/// Provides an NFS v3-style client that communicates with remote servers
/// using a simplified RPC mechanism. Supports mount, lookup, read, write,
/// getattr, readdir, and unmount operations.
///
/// Design:
///   - NfsClient holds connection state (server address, mount handle).
///   - RPC messages are serialized into byte buffers (XDR-like encoding).
///   - File handles are opaque byte arrays returned by the server.
///   - A global client table maps mount IDs to NfsClient instances.
///   - Since we cannot do real network I/O in a stub, operations simulate
///     the protocol flow and return placeholder results.
///
/// Inspired by: Linux NFS client (fs/nfs). All code is original.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const NFS_PROGRAM: u32 = 100003;
const NFS_VERSION: u32 = 3;
const MOUNT_PROGRAM: u32 = 100005;
const NFS_FHSIZE: usize = 64;

// ---------------------------------------------------------------------------
// RPC layer
// ---------------------------------------------------------------------------

/// RPC message types.
#[derive(Clone, Copy, PartialEq)]
enum RpcMsgType {
    Call,
    Reply,
}

/// Simplified RPC header.
#[derive(Clone)]
struct RpcHeader {
    xid: u32,
    msg_type: RpcMsgType,
    program: u32,
    version: u32,
    procedure: u32,
}

/// RPC call result status.
#[derive(Clone, Copy, PartialEq)]
pub enum NfsStatus {
    Ok,
    ErrPerm,
    ErrNoEnt,
    ErrIo,
    ErrAcces,
    ErrExist,
    ErrNotDir,
    ErrIsDir,
    ErrFbig,
    ErrNoSpc,
    ErrStale,
}

impl NfsStatus {
    fn from_u32(v: u32) -> Self {
        match v {
            0 => NfsStatus::Ok,
            1 => NfsStatus::ErrPerm,
            2 => NfsStatus::ErrNoEnt,
            5 => NfsStatus::ErrIo,
            13 => NfsStatus::ErrAcces,
            17 => NfsStatus::ErrExist,
            20 => NfsStatus::ErrNotDir,
            21 => NfsStatus::ErrIsDir,
            27 => NfsStatus::ErrFbig,
            28 => NfsStatus::ErrNoSpc,
            70 => NfsStatus::ErrStale,
            _ => NfsStatus::ErrIo,
        }
    }

    fn to_u32(&self) -> u32 {
        match self {
            NfsStatus::Ok => 0,
            NfsStatus::ErrPerm => 1,
            NfsStatus::ErrNoEnt => 2,
            NfsStatus::ErrIo => 5,
            NfsStatus::ErrAcces => 13,
            NfsStatus::ErrExist => 17,
            NfsStatus::ErrNotDir => 20,
            NfsStatus::ErrIsDir => 21,
            NfsStatus::ErrFbig => 27,
            NfsStatus::ErrNoSpc => 28,
            NfsStatus::ErrStale => 70,
        }
    }
}

// ---------------------------------------------------------------------------
// File handle and attributes
// ---------------------------------------------------------------------------

/// NFS file handle (opaque server-assigned identifier).
#[derive(Clone)]
pub struct NfsFileHandle {
    pub data: Vec<u8>,
}

/// NFS file attributes (fattr3).
#[derive(Clone)]
pub struct NfsAttr {
    pub file_type: u32, // 1=REG, 2=DIR, 5=LNK
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub used: u64,
    pub fileid: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}

/// Directory entry from readdir.
#[derive(Clone)]
pub struct NfsDirEntry {
    pub fileid: u64,
    pub name: String,
}

// ---------------------------------------------------------------------------
// NFS client
// ---------------------------------------------------------------------------

/// State of a single NFS mount.
struct NfsMount {
    id: usize,
    server: String,
    export: String,
    root_fh: NfsFileHandle,
    next_xid: u32,
    /// Simulated directory tree for stub operation
    entries: Vec<(String, NfsAttr, Vec<u8>)>, // (path, attrs, data)
}

impl NfsMount {
    fn new(id: usize, server: &str, export: &str) -> Self {
        // Create a root file handle (simulated)
        let mut root_data = Vec::new();
        root_data.extend_from_slice(server.as_bytes());
        root_data.extend_from_slice(export.as_bytes());
        root_data.resize(NFS_FHSIZE, 0);

        let mut mount = NfsMount {
            id,
            server: String::from(server),
            export: String::from(export),
            root_fh: NfsFileHandle { data: root_data },
            next_xid: 1,
            entries: Vec::new(),
        };

        // Populate root directory entry
        mount.entries.push((
            String::from("/"),
            NfsAttr {
                file_type: 2, // DIR
                mode: 0o755,
                nlink: 2,
                uid: 0,
                gid: 0,
                size: 0,
                used: 0,
                fileid: 1,
                atime: 0,
                mtime: 0,
                ctime: 0,
            },
            Vec::new(),
        ));
        mount
    }

    fn next_xid(&mut self) -> u32 {
        let xid = self.next_xid;
        self.next_xid = self.next_xid.saturating_add(1);
        xid
    }

    fn lookup(&self, path: &str) -> Result<(NfsFileHandle, NfsAttr), NfsStatus> {
        for (p, attr, _) in self.entries.iter() {
            if p == path {
                let mut fh_data = Vec::from(path.as_bytes());
                fh_data.resize(NFS_FHSIZE, 0);
                return Ok((NfsFileHandle { data: fh_data }, attr.clone()));
            }
        }
        Err(NfsStatus::ErrNoEnt)
    }

    fn getattr(&self, path: &str) -> Result<NfsAttr, NfsStatus> {
        for (p, attr, _) in self.entries.iter() {
            if p == path {
                return Ok(attr.clone());
            }
        }
        Err(NfsStatus::ErrNoEnt)
    }

    fn read(&self, path: &str, offset: u64, count: usize) -> Result<Vec<u8>, NfsStatus> {
        for (p, _, data) in self.entries.iter() {
            if p == path {
                let start = (offset as usize).min(data.len());
                let end = (start + count).min(data.len());
                return Ok(Vec::from(&data[start..end]));
            }
        }
        Err(NfsStatus::ErrNoEnt)
    }

    fn write(&mut self, path: &str, offset: u64, buf: &[u8]) -> Result<usize, NfsStatus> {
        for (p, attr, data) in self.entries.iter_mut() {
            if p == path {
                let start = offset as usize;
                let needed = start + buf.len();
                if data.len() < needed {
                    data.resize(needed, 0);
                }
                data[start..start + buf.len()].copy_from_slice(buf);
                attr.size = data.len() as u64;
                return Ok(buf.len());
            }
        }
        Err(NfsStatus::ErrNoEnt)
    }

    fn create(&mut self, path: &str, mode: u32) -> Result<NfsFileHandle, NfsStatus> {
        // Check for existing
        for (p, _, _) in self.entries.iter() {
            if p == path {
                return Err(NfsStatus::ErrExist);
            }
        }
        let attr = NfsAttr {
            file_type: 1, // REG
            mode,
            nlink: 1,
            uid: 0,
            gid: 0,
            size: 0,
            used: 0,
            fileid: self.entries.len() as u64 + 1,
            atime: 0,
            mtime: 0,
            ctime: 0,
        };
        self.entries.push((String::from(path), attr, Vec::new()));
        let mut fh = Vec::from(path.as_bytes());
        fh.resize(NFS_FHSIZE, 0);
        Ok(NfsFileHandle { data: fh })
    }

    fn readdir(&self, dir_path: &str) -> Result<Vec<NfsDirEntry>, NfsStatus> {
        let prefix = if dir_path == "/" {
            String::from("/")
        } else {
            let mut p = String::from(dir_path);
            p.push('/');
            p
        };

        let mut entries = Vec::new();
        for (p, attr, _) in self.entries.iter() {
            if p == dir_path {
                continue; // skip the directory itself
            }
            if p.starts_with(prefix.as_str()) {
                let remainder = &p[prefix.len()..];
                if !remainder.contains('/') {
                    entries.push(NfsDirEntry {
                        fileid: attr.fileid,
                        name: String::from(remainder),
                    });
                }
            }
        }
        Ok(entries)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct NfsTable {
    mounts: Vec<Option<NfsMount>>,
    next_id: usize,
}

impl NfsTable {
    fn new() -> Self {
        NfsTable {
            mounts: Vec::new(),
            next_id: 0,
        }
    }
}

static NFS_TABLE: Mutex<Option<NfsTable>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mount an NFS export, returning a mount ID.
pub fn mount(server: &str, export: &str) -> Result<usize, NfsStatus> {
    let mut guard = NFS_TABLE.lock();
    let table = guard.as_mut().ok_or(NfsStatus::ErrIo)?;
    let id = table.next_id;
    table.next_id = table.next_id.saturating_add(1);
    let mount = NfsMount::new(id, server, export);
    if id < table.mounts.len() {
        table.mounts[id] = Some(mount);
    } else {
        table.mounts.push(Some(mount));
    }
    Ok(id)
}

/// Unmount an NFS export.
pub fn unmount(mount_id: usize) {
    let mut guard = NFS_TABLE.lock();
    if let Some(table) = guard.as_mut() {
        if mount_id < table.mounts.len() {
            table.mounts[mount_id] = None;
        }
    }
}

/// Look up a file by path on an NFS mount.
pub fn lookup(mount_id: usize, path: &str) -> Result<(NfsFileHandle, NfsAttr), NfsStatus> {
    let guard = NFS_TABLE.lock();
    let table = guard.as_ref().ok_or(NfsStatus::ErrIo)?;
    let m = table
        .mounts
        .get(mount_id)
        .and_then(|s| s.as_ref())
        .ok_or(NfsStatus::ErrStale)?;
    m.lookup(path)
}

/// Read data from an NFS file.
pub fn read(mount_id: usize, path: &str, offset: u64, count: usize) -> Result<Vec<u8>, NfsStatus> {
    let guard = NFS_TABLE.lock();
    let table = guard.as_ref().ok_or(NfsStatus::ErrIo)?;
    let m = table
        .mounts
        .get(mount_id)
        .and_then(|s| s.as_ref())
        .ok_or(NfsStatus::ErrStale)?;
    m.read(path, offset, count)
}

/// Write data to an NFS file.
pub fn write(mount_id: usize, path: &str, offset: u64, data: &[u8]) -> Result<usize, NfsStatus> {
    let mut guard = NFS_TABLE.lock();
    let table = guard.as_mut().ok_or(NfsStatus::ErrIo)?;
    let m = table
        .mounts
        .get_mut(mount_id)
        .and_then(|s| s.as_mut())
        .ok_or(NfsStatus::ErrStale)?;
    m.write(path, offset, data)
}

/// Create a file on an NFS mount.
pub fn create(mount_id: usize, path: &str, mode: u32) -> Result<NfsFileHandle, NfsStatus> {
    let mut guard = NFS_TABLE.lock();
    let table = guard.as_mut().ok_or(NfsStatus::ErrIo)?;
    let m = table
        .mounts
        .get_mut(mount_id)
        .and_then(|s| s.as_mut())
        .ok_or(NfsStatus::ErrStale)?;
    m.create(path, mode)
}

/// Read directory entries.
pub fn readdir(mount_id: usize, path: &str) -> Result<Vec<NfsDirEntry>, NfsStatus> {
    let guard = NFS_TABLE.lock();
    let table = guard.as_ref().ok_or(NfsStatus::ErrIo)?;
    let m = table
        .mounts
        .get(mount_id)
        .and_then(|s| s.as_ref())
        .ok_or(NfsStatus::ErrStale)?;
    m.readdir(path)
}

/// Initialize the NFS client subsystem.
pub fn init() {
    let mut guard = NFS_TABLE.lock();
    *guard = Some(NfsTable::new());
    serial_println!("    nfs: initialized (NFS v3 client, RPC transport)");
}
