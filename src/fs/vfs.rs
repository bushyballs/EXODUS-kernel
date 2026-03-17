/// VFS core: filesystem traits, inode, directory entry, mount table,
/// file locking (flock), directory entry cache (dcache), and inode
/// abstraction with timestamps.
///
/// Every filesystem implements the FileSystem trait.
/// Every file/directory is represented by an Inode.
/// Directory entries map names to inodes.
/// Mount table resolves paths through mount boundaries.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// File type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    CharDevice,
    BlockDevice,
    Pipe,
    Symlink,
    Socket,
}

/// File open flags
pub mod flags {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    pub const O_CREAT: u32 = 0x40;
    pub const O_TRUNC: u32 = 0x200;
    pub const O_APPEND: u32 = 0x400;
    pub const O_DIRECTORY: u32 = 0x10000;
    pub const O_NOFOLLOW: u32 = 0x20000;
    pub const O_CLOEXEC: u32 = 0x80000;
}

/// Inode -- the kernel's in-memory representation of a file or directory
pub struct Inode {
    /// Inode number (unique within filesystem)
    pub ino: u64,
    /// File type
    pub file_type: FileType,
    /// File size in bytes
    pub size: u64,
    /// Permission mode (Unix-style)
    pub mode: u32,
    /// Owner user ID
    pub uid: u32,
    /// Owner group ID
    pub gid: u32,
    /// Number of hard links
    pub nlink: u32,
    /// File operations for this inode type
    pub ops: Box<dyn FileOps>,
    /// Device ID (for block/char devices)
    pub rdev: u64,
    /// Block count (in 512-byte blocks)
    pub blocks: u64,
    /// Access time (Unix timestamp)
    pub atime: u64,
    /// Modification time (Unix timestamp)
    pub mtime: u64,
    /// Change time (Unix timestamp)
    pub ctime: u64,
    /// Creation time (Unix timestamp)
    pub crtime: u64,
}

impl core::fmt::Debug for Inode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Inode")
            .field("ino", &self.ino)
            .field("file_type", &self.file_type)
            .field("size", &self.size)
            .field("mode", &self.mode)
            .field("nlink", &self.nlink)
            .finish()
    }
}

impl Inode {
    /// Create a simple inode (backward-compatible helper)
    pub fn new(
        ino: u64,
        file_type: FileType,
        size: u64,
        mode: u32,
        uid: u32,
        gid: u32,
        nlink: u32,
        ops: Box<dyn FileOps>,
    ) -> Self {
        let now = crate::time::clock::unix_time();
        Inode {
            ino,
            file_type,
            size,
            mode,
            uid,
            gid,
            nlink,
            ops,
            rdev: 0,
            blocks: (size + 511) / 512,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
        }
    }
}

/// Directory entry -- maps a name to an inode
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub ino: u64,
    pub file_type: FileType,
}

/// File operations trait -- each filesystem implements this
///
/// This is the VFS abstraction layer. When a program calls read(),
/// the VFS dispatches to the correct filesystem's read implementation.
pub trait FileOps: Send + Sync + core::fmt::Debug {
    /// Read up to `buf.len()` bytes from offset into buf.
    /// Returns number of bytes read.
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Write `buf` at offset. Returns number of bytes written.
    fn write(&self, offset: u64, buf: &[u8]) -> Result<usize, FsError>;

    /// Get file size
    fn size(&self) -> u64;

    /// List directory entries (only valid for directories)
    fn readdir(&self) -> Result<Vec<DirEntry>, FsError> {
        Err(FsError::NotADirectory)
    }

    /// Look up a name in a directory, return the inode
    fn lookup(&self, _name: &str) -> Result<Inode, FsError> {
        Err(FsError::NotADirectory)
    }

    /// Create a file in this directory
    fn create(&self, _name: &str, _file_type: FileType) -> Result<Inode, FsError> {
        Err(FsError::NotSupported)
    }

    /// Remove a file from this directory
    fn unlink(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }
}

/// Filesystem errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    NotADirectory,
    IsADirectory,
    AlreadyExists,
    NotEmpty,
    NoSpace,
    IoError,
    NotSupported,
    InvalidArgument,
    TooManyOpenFiles,
    NotAFile,
    /// File is locked by another process
    WouldBlock,
    /// Deadlock detected in locking
    Deadlock,
    /// Cross-device link
    CrossDevice,
    /// Name too long
    NameTooLong,
}

/// Filesystem trait -- each mountable filesystem implements this
pub trait FileSystem: Send + Sync {
    /// Name of this filesystem type
    fn name(&self) -> &str;

    /// Get the root inode of this filesystem
    fn root(&self) -> Result<Inode, FsError>;

    /// Sync all pending writes to disk
    fn sync(&self) -> Result<(), FsError> {
        Ok(())
    }

    /// Get filesystem statistics
    fn statfs(&self) -> Result<FsStats, FsError> {
        Ok(FsStats::default())
    }
}

/// Filesystem statistics (like Linux statfs)
#[derive(Debug, Clone, Copy)]
pub struct FsStats {
    /// Total blocks
    pub blocks_total: u64,
    /// Free blocks
    pub blocks_free: u64,
    /// Available blocks (for non-root)
    pub blocks_available: u64,
    /// Total inodes
    pub inodes_total: u64,
    /// Free inodes
    pub inodes_free: u64,
    /// Block size in bytes
    pub block_size: u32,
    /// Maximum filename length
    pub max_name_len: u32,
}

impl Default for FsStats {
    fn default() -> Self {
        FsStats {
            blocks_total: 0,
            blocks_free: 0,
            blocks_available: 0,
            inodes_total: 0,
            inodes_free: 0,
            block_size: 4096,
            max_name_len: 255,
        }
    }
}

// ============================================================================
// Mount table -- track mounted filesystems at mount points
// ============================================================================

/// Mount point: associates a path with a filesystem
struct MountPoint {
    path: String,
    fs: Box<dyn FileSystem>,
    /// Mount flags (read-only, noexec, etc.)
    _flags: u32,
}

/// Mount flags
pub mod mount_flags {
    pub const MS_RDONLY: u32 = 1;
    pub const MS_NOEXEC: u32 = 8;
    pub const MS_NOSUID: u32 = 2;
    pub const MS_NODEV: u32 = 4;
}

/// Global mount table
static MOUNT_TABLE: Mutex<Vec<MountPoint>> = Mutex::new(Vec::new());

/// Initialize VFS
pub fn init() {
    // Mount table starts empty, filesystems register themselves
}

/// Mount a filesystem at a path
pub fn mount(path: &str, fs: Box<dyn FileSystem>) {
    mount_with_flags(path, fs, 0);
}

/// Mount a filesystem at a path with flags
pub fn mount_with_flags(path: &str, fs: Box<dyn FileSystem>, flags: u32) {
    let mut table = MOUNT_TABLE.lock();
    serial_println!("  VFS: mounted {} at {}", fs.name(), path);
    table.push(MountPoint {
        path: String::from(path),
        fs,
        _flags: flags,
    });
}

/// Unmount a filesystem
pub fn umount(path: &str) -> Result<(), FsError> {
    let mut table = MOUNT_TABLE.lock();
    let idx = table
        .iter()
        .position(|m| m.path == path)
        .ok_or(FsError::NotFound)?;
    // Sync before unmount
    let _ = table[idx].fs.sync();
    table.remove(idx);
    serial_println!("  VFS: unmounted {}", path);
    Ok(())
}

/// Find the deepest matching mount point for a path.
/// Returns (mount index, remaining path after mount point).
pub fn resolve_mount(path: &str) -> Option<(usize, String)> {
    let table = MOUNT_TABLE.lock();
    let mut best_idx = None;
    let mut best_len = 0;

    for (i, mp) in table.iter().enumerate() {
        let mp_path = mp.path.as_str();
        if path == mp_path || path.starts_with(&alloc::format!("{}/", mp_path)) {
            if mp_path.len() > best_len {
                best_len = mp_path.len();
                best_idx = Some(i);
            }
        }
        // Also match root mount "/"
        if mp_path == "/" && best_idx.is_none() {
            best_idx = Some(i);
            best_len = 1;
        }
    }

    if let Some(idx) = best_idx {
        let remaining = if best_len >= path.len() {
            String::from("/")
        } else {
            let rest = &path[best_len..];
            if rest.starts_with('/') {
                String::from(rest)
            } else {
                let mut s = String::from("/");
                s.push_str(rest);
                s
            }
        };
        Some((idx, remaining))
    } else {
        None
    }
}

/// List all mount points
pub fn list_mounts() -> Vec<(String, String)> {
    let table = MOUNT_TABLE.lock();
    table
        .iter()
        .map(|mp| (mp.path.clone(), String::from(mp.fs.name())))
        .collect()
}

/// VFS open: resolve path through mount table to the right filesystem
pub fn vfs_lookup(path: &str) -> Result<Inode, FsError> {
    // Check virtual filesystems first
    if path.starts_with("/proc") {
        return vfs_lookup_proc(path);
    }
    if path.starts_with("/sys") {
        return vfs_lookup_sys(path);
    }

    // Try mount table resolution
    {
        let table = MOUNT_TABLE.lock();
        let mut best_idx = None;
        let mut best_len = 0usize;

        for (i, mp) in table.iter().enumerate() {
            let mp_path = mp.path.as_str();
            if path == mp_path || path.starts_with(&alloc::format!("{}/", mp_path)) {
                if mp_path.len() > best_len {
                    best_len = mp_path.len();
                    best_idx = Some(i);
                }
            }
        }

        if let Some(idx) = best_idx {
            let remaining = if best_len >= path.len() {
                String::new()
            } else {
                String::from(&path[best_len..])
            };

            let root = table[idx].fs.root()?;
            drop(table);

            if remaining.is_empty() || remaining == "/" {
                return Ok(root);
            }

            // Walk the path through the filesystem
            let components: Vec<&str> = remaining.split('/').filter(|s| !s.is_empty()).collect();
            let mut current_ops = root.ops;

            for (i, comp) in components.iter().enumerate() {
                let child = current_ops.lookup(comp)?;
                if i == components.len() - 1 {
                    return Ok(child);
                }
                current_ops = child.ops;
            }

            return Err(FsError::NotFound);
        }
    }

    // Fall back to memfs
    vfs_lookup_memfs(path)
}

/// Look up a path in the procfs virtual filesystem
fn vfs_lookup_proc(path: &str) -> Result<Inode, FsError> {
    // Check whether this path is a known procfs directory first.
    // procfs_is_path returns true for anything under /proc.
    // proc_list returns Some(...) for directory paths.
    if let Some(_entries) = proc_list(path) {
        // It's a directory
        return Ok(Inode::new(
            0,
            FileType::Directory,
            0,
            0o555,
            0,
            0,
            1,
            Box::new(StaticFileOps {
                data: alloc::vec![],
            }),
        ));
    }
    let content = super::procfs::read(path).ok_or(FsError::NotFound)?;
    let data = content.into_bytes();
    let size = data.len() as u64;
    Ok(Inode::new(
        0,
        FileType::Regular,
        size,
        0o444,
        0,
        0,
        1,
        Box::new(StaticFileOps { data }),
    ))
}

/// Look up a path in the memfs
fn vfs_lookup_memfs(path: &str) -> Result<Inode, FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    let data = node.data.clone();
    let file_type = node.file_type;
    let mode = node.mode;
    let size = data.len() as u64;
    Ok(Inode::new(
        0,
        file_type,
        size,
        mode,
        0,
        0,
        1,
        Box::new(StaticFileOps { data }),
    ))
}

/// Look up a path in the sysfs virtual filesystem
fn vfs_lookup_sys(path: &str) -> Result<Inode, FsError> {
    // Check if this path is a known sysfs directory.
    let mut out = [[0u8; 64]; 64];
    let count = super::sysfs::sysfs_readdir(path.as_bytes(), &mut out);
    if count > 0 {
        return Ok(Inode::new(
            0,
            FileType::Directory,
            0,
            0o555,
            0,
            0,
            1,
            Box::new(StaticFileOps {
                data: alloc::vec![],
            }),
        ));
    }
    // Try as a regular file.
    let mut buf = [0u8; 4096];
    let n = super::sysfs::sysfs_read(path.as_bytes(), &mut buf);
    if n >= 0 {
        let data = buf[..n as usize].to_vec();
        let size = data.len() as u64;
        return Ok(Inode::new(
            0,
            FileType::Regular,
            size,
            0o644,
            0,
            0,
            1,
            Box::new(StaticFileOps { data }),
        ));
    }
    Err(FsError::NotFound)
}

fn is_proc_path(path: &str) -> bool {
    path == "/proc" || path.starts_with("/proc/")
}

fn is_sys_path(path: &str) -> bool {
    path == "/sys" || path.starts_with("/sys/")
}

fn join_child_path(parent: &str, child: &str) -> String {
    if parent == "/" {
        alloc::format!("/{}", child)
    } else {
        alloc::format!("{}/{}", parent.trim_end_matches('/'), child)
    }
}

fn proc_list(path: &str) -> Option<Vec<String>> {
    let trimmed = path.trim_end_matches('/');
    if trimmed == "/proc" {
        return Some(super::procfs::list_dir("/proc"));
    }
    if trimmed == "/proc/self" {
        return Some(alloc::vec![
            String::from("status"),
            String::from("maps"),
            String::from("cmdline"),
        ]);
    }
    if trimmed == "/proc/net" {
        return Some(alloc::vec![
            String::from("dev"),
            String::from("route"),
            String::from("if_inet6"),
        ]);
    }
    if trimmed == "/proc/sys" {
        return Some(alloc::vec![String::from("kernel"), String::from("vm"),]);
    }
    if trimmed == "/proc/sys/kernel" {
        return Some(alloc::vec![
            String::from("hostname"),
            String::from("ostype"),
            String::from("osrelease"),
        ]);
    }
    if trimmed == "/proc/sys/vm" {
        return Some(alloc::vec![String::from("overcommit_memory"),]);
    }

    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() == 2 && parts[0] == "proc" && parts[1].parse::<u32>().is_ok() {
        return Some(alloc::vec![
            String::from("status"),
            String::from("maps"),
            String::from("cmdline"),
        ]);
    }
    None
}

/// Read a file through the VFS (handles /proc, /sys, then memfs fallback).
pub fn fs_read(path: &str) -> Result<Vec<u8>, FsError> {
    if is_proc_path(path) {
        return proc_read(path);
    }
    if is_sys_path(path) {
        return super::sysfs::read(path)
            .map(|s| s.into_bytes())
            .ok_or(FsError::NotFound);
    }
    memfs_read(path)
}

/// Read from a file at an offset through the VFS.
pub fn fs_read_at(path: &str, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
    if is_proc_path(path) || is_sys_path(path) {
        let data = fs_read(path)?;
        if offset >= data.len() {
            return Ok(0);
        }
        let n = (data.len() - offset).min(buf.len());
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        return Ok(n);
    }
    memfs_read_at(path, offset, buf)
}

/// Write a file through the VFS.
pub fn fs_write(path: &str, data: &[u8]) -> Result<(), FsError> {
    if is_proc_path(path) {
        // Route to procfs for writable /proc/sys paths
        let result = super::procfs::procfs_write(path.as_bytes(), data);
        return match result {
            n if n >= 0 => Ok(()),
            -2 => Err(FsError::NotFound),
            _ => Err(FsError::PermissionDenied),
        };
    }
    if is_sys_path(path) {
        // Route to sysfs_write for writable /sys paths.
        let result = super::sysfs::sysfs_write(path.as_bytes(), data);
        return match result {
            n if n >= 0 => Ok(()),
            -2 => Err(FsError::NotFound),
            -22 => Err(FsError::InvalidArgument),
            _ => Err(FsError::PermissionDenied),
        };
    }
    memfs_write(path, data)
}

/// Write a file at an offset through the VFS.
pub fn fs_write_at(path: &str, offset: usize, data: &[u8]) -> Result<usize, FsError> {
    if is_proc_path(path) {
        // Route to procfs for writable /proc/sys paths; offset is ignored
        // (procfs writes are always full-value replacements)
        let _ = offset;
        let result = super::procfs::procfs_write(path.as_bytes(), data);
        return match result {
            n if n >= 0 => Ok(n as usize),
            -2 => Err(FsError::NotFound),
            _ => Err(FsError::PermissionDenied),
        };
    }
    if is_sys_path(path) {
        // Route to sysfs_write; offset is ignored (attr writes replace the value)
        let _ = offset;
        let result = super::sysfs::sysfs_write(path.as_bytes(), data);
        return match result {
            n if n >= 0 => Ok(n as usize),
            -2 => Err(FsError::NotFound),
            -22 => Err(FsError::InvalidArgument),
            _ => Err(FsError::PermissionDenied),
        };
    }
    memfs_write_at(path, offset, data)
}

/// Stat a path through the VFS.
pub fn fs_stat(path: &str) -> Result<(FileType, u64), FsError> {
    if path == "/proc" {
        return Ok((FileType::Directory, 0));
    }
    if path == "/sys" {
        return Ok((FileType::Directory, 0));
    }

    if is_proc_path(path) {
        if let Some(entries) = proc_list(path) {
            if !entries.is_empty() {
                return Ok((FileType::Directory, 0));
            }
        }
        let data = fs_read(path)?;
        return Ok((FileType::Regular, data.len() as u64));
    }

    if is_sys_path(path) {
        let entries = super::sysfs::list_dir(path);
        if !entries.is_empty() {
            return Ok((FileType::Directory, 0));
        }
        let data = fs_read(path)?;
        return Ok((FileType::Regular, data.len() as u64));
    }

    memfs_stat(path)
}

/// List directory entries through the VFS.
pub fn fs_ls(path: &str) -> Result<Vec<(String, FileType, u64)>, FsError> {
    if is_proc_path(path) {
        let entries = proc_list(path).ok_or(FsError::NotFound)?;
        let mut out = Vec::new();
        for name in entries {
            let full = join_child_path(path, &name);
            let (kind, size) = fs_stat(&full).unwrap_or((FileType::Regular, 0));
            out.push((name, kind, size));
        }
        return Ok(out);
    }

    if is_sys_path(path) {
        let entries = super::sysfs::list_dir(path);
        if entries.is_empty() {
            return Err(FsError::NotFound);
        }
        let mut out = Vec::new();
        for name in entries {
            let full = join_child_path(path, &name);
            let (kind, size) = fs_stat(&full).unwrap_or((FileType::Regular, 0));
            out.push((name, kind, size));
        }
        return Ok(out);
    }

    memfs_ls(path)
}

/// Create a directory through the VFS.
pub fn fs_mkdir(path: &str) -> Result<(), FsError> {
    if is_proc_path(path) || is_sys_path(path) {
        return Err(FsError::PermissionDenied);
    }
    memfs_mkdir(path)
}

/// Remove a file or directory through the VFS.
pub fn fs_rm(path: &str) -> Result<(), FsError> {
    if is_proc_path(path) || is_sys_path(path) {
        return Err(FsError::PermissionDenied);
    }
    memfs_rm(path)
}

/// Change mode bits through the VFS.
pub fn fs_chmod(path: &str, mode: u32) -> Result<(), FsError> {
    if is_proc_path(path) || is_sys_path(path) {
        return Err(FsError::PermissionDenied);
    }
    memfs_chmod(path, mode)
}

/// Create a symlink through the VFS.
pub fn fs_symlink(link_path: &str, target: &str) -> Result<(), FsError> {
    if is_proc_path(link_path) || is_sys_path(link_path) {
        return Err(FsError::PermissionDenied);
    }
    memfs_symlink(link_path, target)
}

/// Read a symlink through the VFS.
pub fn fs_readlink(path: &str) -> Result<String, FsError> {
    if is_proc_path(path) || is_sys_path(path) {
        return Err(FsError::NotSupported);
    }
    memfs_readlink(path)
}

/// Simple FileOps backed by static data (for procfs, sysfs lookups)
#[derive(Debug)]
pub struct StaticFileOps {
    pub data: Vec<u8>,
}

impl FileOps for StaticFileOps {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        if offset >= self.data.len() as u64 {
            return Ok(0);
        }
        let start = offset as usize;
        let n = core::cmp::min(buf.len(), self.data.len() - start);
        buf[..n].copy_from_slice(&self.data[start..start + n]);
        Ok(n)
    }
    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::NotSupported)
    }
    fn size(&self) -> u64 {
        self.data.len() as u64
    }
}

// ============================================================================
// File locking (flock semantics)
// ============================================================================

/// Lock type for flock operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockType {
    /// Shared lock (multiple readers)
    Shared,
    /// Exclusive lock (single writer)
    Exclusive,
    /// Unlock
    Unlock,
}

/// A file lock entry
struct FileLock {
    /// Inode number of the locked file
    ino: u64,
    /// PID of the lock holder
    pid: u32,
    /// Lock type
    lock_type: LockType,
}

/// Global file lock table
static FILE_LOCKS: Mutex<Vec<FileLock>> = Mutex::new(Vec::new());

/// Acquire a file lock (flock semantics)
///
/// - Shared locks: multiple processes can hold shared locks simultaneously
/// - Exclusive locks: only one process can hold an exclusive lock
/// - A process can upgrade from shared to exclusive if it's the only shared holder
pub fn flock(ino: u64, pid: u32, lock_type: LockType, blocking: bool) -> Result<(), FsError> {
    match lock_type {
        LockType::Unlock => {
            let mut locks = FILE_LOCKS.lock();
            locks.retain(|l| !(l.ino == ino && l.pid == pid));
            Ok(())
        }
        LockType::Shared => {
            let mut locks = FILE_LOCKS.lock();
            // Check for exclusive locks by other processes
            for lock in locks.iter() {
                if lock.ino == ino && lock.lock_type == LockType::Exclusive && lock.pid != pid {
                    if blocking {
                        // In a real kernel we'd sleep; for now return WouldBlock
                        return Err(FsError::WouldBlock);
                    }
                    return Err(FsError::WouldBlock);
                }
            }
            // Remove any existing lock by this pid on this inode
            locks.retain(|l| !(l.ino == ino && l.pid == pid));
            locks.push(FileLock {
                ino,
                pid,
                lock_type: LockType::Shared,
            });
            Ok(())
        }
        LockType::Exclusive => {
            let mut locks = FILE_LOCKS.lock();
            // Check for any locks by other processes
            for lock in locks.iter() {
                if lock.ino == ino && lock.pid != pid {
                    if blocking {
                        return Err(FsError::WouldBlock);
                    }
                    return Err(FsError::WouldBlock);
                }
            }
            // Remove any existing lock by this pid on this inode
            locks.retain(|l| !(l.ino == ino && l.pid == pid));
            locks.push(FileLock {
                ino,
                pid,
                lock_type: LockType::Exclusive,
            });
            Ok(())
        }
    }
}

/// Release all locks held by a process (called on process exit)
pub fn flock_release_all(pid: u32) {
    let mut locks = FILE_LOCKS.lock();
    locks.retain(|l| l.pid != pid);
}

/// Check if a file is locked and by whom
pub fn flock_query(ino: u64) -> Vec<(u32, LockType)> {
    let locks = FILE_LOCKS.lock();
    locks
        .iter()
        .filter(|l| l.ino == ino)
        .map(|l| (l.pid, l.lock_type))
        .collect()
}

// ============================================================================
// Directory entry cache (dcache) -- accelerates path lookups
// ============================================================================

/// Maximum dcache entries
const DCACHE_MAX: usize = 512;

/// A cached directory entry
struct DcacheEntry {
    /// Full path to the parent directory
    parent_path: String,
    /// Name of this entry in the parent
    name: String,
    /// Inode number
    ino: u64,
    /// File type
    file_type: FileType,
    /// Access counter for LRU eviction
    access_count: u32,
    /// Validity flag (set to false when invalidated)
    valid: bool,
}

/// Global dcache
static DCACHE: Mutex<Vec<DcacheEntry>> = Mutex::new(Vec::new());

/// Look up a path component in the dcache
pub fn dcache_lookup(parent_path: &str, name: &str) -> Option<(u64, FileType)> {
    let mut cache = DCACHE.lock();
    for entry in cache.iter_mut() {
        if entry.valid && entry.parent_path == parent_path && entry.name == name {
            entry.access_count = entry.access_count.saturating_add(1);
            return Some((entry.ino, entry.file_type));
        }
    }
    None
}

/// Insert an entry into the dcache
pub fn dcache_insert(parent_path: &str, name: &str, ino: u64, file_type: FileType) {
    let mut cache = DCACHE.lock();

    // Check if already present
    for entry in cache.iter_mut() {
        if entry.parent_path == parent_path && entry.name == name {
            entry.ino = ino;
            entry.file_type = file_type;
            entry.valid = true;
            entry.access_count = entry.access_count.saturating_add(1);
            return;
        }
    }

    // Evict LRU if full
    if cache.len() >= DCACHE_MAX {
        let mut min_idx = 0;
        let mut min_count = u32::MAX;
        for (i, entry) in cache.iter().enumerate() {
            if entry.access_count < min_count {
                min_count = entry.access_count;
                min_idx = i;
            }
        }
        cache.remove(min_idx);
    }

    cache.push(DcacheEntry {
        parent_path: String::from(parent_path),
        name: String::from(name),
        ino,
        file_type,
        access_count: 1,
        valid: true,
    });
}

/// Invalidate dcache entries for a path (e.g., after unlink/rename)
pub fn dcache_invalidate(parent_path: &str, name: &str) {
    let mut cache = DCACHE.lock();
    for entry in cache.iter_mut() {
        if entry.parent_path == parent_path && entry.name == name {
            entry.valid = false;
        }
    }
}

/// Invalidate all dcache entries under a path (e.g., after umount)
pub fn dcache_invalidate_subtree(path: &str) {
    let mut cache = DCACHE.lock();
    let prefix = alloc::format!("{}/", path);
    for entry in cache.iter_mut() {
        if entry.parent_path == path || entry.parent_path.starts_with(&prefix) {
            entry.valid = false;
        }
    }
}

/// Flush the entire dcache
pub fn dcache_flush() {
    let mut cache = DCACHE.lock();
    cache.clear();
}

/// Get dcache statistics
pub fn dcache_stats() -> (usize, usize) {
    let cache = DCACHE.lock();
    let total = cache.len();
    let valid = cache.iter().filter(|e| e.valid).count();
    (total, valid)
}

// ============================================================================
// Inode number allocator
// ============================================================================

/// Global inode counter for dynamically-generated inodes
static NEXT_INO: Mutex<u64> = Mutex::new(1000);

/// Allocate a new unique inode number
pub fn alloc_ino() -> u64 {
    let mut counter = NEXT_INO.lock();
    let ino = *counter;
    *counter = counter.saturating_add(1);
    ino
}

// ============================================================================
// In-memory filesystem (memfs) -- RAM-backed VFS tree
// ============================================================================

/// In-memory filesystem node
#[derive(Debug, Clone)]
pub struct MemNode {
    pub name: String,
    pub file_type: FileType,
    pub data: Vec<u8>,
    pub children: Vec<MemNode>,
    pub mode: u32,
    pub ino: u64,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
}

impl MemNode {
    pub fn new_dir(name: &str) -> Self {
        let now = crate::time::clock::unix_time();
        MemNode {
            name: String::from(name),
            file_type: FileType::Directory,
            data: Vec::new(),
            children: Vec::new(),
            mode: 0o755,
            ino: alloc_ino(),
            uid: 0,
            gid: 0,
            nlink: 2,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }

    pub fn new_file(name: &str, data: &[u8]) -> Self {
        let now = crate::time::clock::unix_time();
        MemNode {
            name: String::from(name),
            file_type: FileType::Regular,
            data: Vec::from(data),
            children: Vec::new(),
            mode: 0o644,
            ino: alloc_ino(),
            uid: 0,
            gid: 0,
            nlink: 1,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }

    pub fn new_symlink(name: &str, target: &str) -> Self {
        let now = crate::time::clock::unix_time();
        MemNode {
            name: String::from(name),
            file_type: FileType::Symlink,
            data: Vec::from(target.as_bytes()),
            children: Vec::new(),
            mode: 0o777,
            ino: alloc_ino(),
            uid: 0,
            gid: 0,
            nlink: 1,
            atime: now,
            mtime: now,
            ctime: now,
        }
    }
}

/// Global in-memory filesystem root
static MEMFS_ROOT: Mutex<Option<MemNode>> = Mutex::new(None);

/// Initialize the in-memory filesystem with a basic directory tree
pub fn init_memfs() {
    let mut root = MemNode::new_dir("/");

    // Create standard directories
    let mut bin = MemNode::new_dir("bin");
    bin.children.push(MemNode::new_file(
        "hello",
        b"#!/bin/hoags-shell\necho Hello, Genesis!\n",
    ));
    root.children.push(bin);

    root.children.push(MemNode::new_dir("etc"));
    root.children.push(MemNode::new_dir("home"));
    root.children.push(MemNode::new_dir("tmp"));
    root.children.push(MemNode::new_dir("var"));
    root.children.push(MemNode::new_dir("dev"));
    root.children.push(MemNode::new_dir("proc"));
    root.children.push(MemNode::new_dir("sys"));
    root.children.push(MemNode::new_dir("mnt"));
    root.children.push(MemNode::new_dir("run"));

    // /etc/hostname
    if let Some(etc) = root.children.iter_mut().find(|c| c.name == "etc") {
        etc.children.push(MemNode::new_file("hostname", b"genesis"));
        etc.children.push(MemNode::new_file(
            "motd",
            b"Welcome to Hoags OS Genesis v1.0.0\nAll systems operational.\n",
        ));
        etc.children.push(MemNode::new_file(
            "passwd",
            b"root:x:0:0:root:/root:/bin/hoags-shell\n",
        ));
        etc.children.push(MemNode::new_file("fstab", b"# <device> <mount> <type> <options> <dump> <pass>\ndevfs /dev devfs rw 0 0\nproc /proc proc rw 0 0\nsys /sys sysfs rw 0 0\ntmpfs /tmp tmpfs rw 0 0\ntmpfs /run tmpfs rw 0 0\n"));
    }

    *MEMFS_ROOT.lock() = Some(root);
}

/// Split a path into components, handling leading/trailing slashes
fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Navigate to a node by path (read-only)
fn find_node<'a>(root: &'a MemNode, path: &str) -> Option<&'a MemNode> {
    let parts = split_path(path);
    let mut current = root;
    for part in &parts {
        match current.children.iter().find(|c| c.name == *part) {
            Some(child) => current = child,
            None => return None,
        }
    }
    Some(current)
}

/// Navigate to a node by path (mutable)
fn find_node_mut<'a>(root: &'a mut MemNode, path: &str) -> Option<&'a mut MemNode> {
    let parts = split_path(path);
    let mut current = root;
    for part in parts {
        match current.children.iter_mut().find(|c| c.name == part) {
            Some(child) => current = child,
            None => return None,
        }
    }
    Some(current)
}

/// List directory contents at a path
pub fn memfs_ls(path: &str) -> Result<Vec<(String, FileType, u64)>, FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    if node.file_type != FileType::Directory {
        return Err(FsError::NotADirectory);
    }
    Ok(node
        .children
        .iter()
        .map(|c| (c.name.clone(), c.file_type, c.data.len() as u64))
        .collect())
}

/// Read a file's contents
pub fn memfs_read(path: &str) -> Result<Vec<u8>, FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    if node.file_type == FileType::Directory {
        return Err(FsError::IsADirectory);
    }
    Ok(node.data.clone())
}

/// Read bytes from a file at a specific offset.
pub fn memfs_read_at(path: &str, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
    if buf.is_empty() {
        return Ok(0);
    }

    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    if node.file_type == FileType::Directory {
        return Err(FsError::IsADirectory);
    }

    if offset >= node.data.len() {
        return Ok(0);
    }

    let available = node.data.len() - offset;
    let n = core::cmp::min(buf.len(), available);
    buf[..n].copy_from_slice(&node.data[offset..offset + n]);
    Ok(n)
}

/// Create a directory
pub fn memfs_mkdir(path: &str) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;

    let parts = split_path(path);
    if parts.is_empty() {
        return Err(FsError::InvalidArgument);
    }
    let (parent_parts, new_name) = parts.split_at(parts.len() - 1);
    let parent_path: String = if parent_parts.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        p.push_str(&parent_parts.join("/"));
        p
    };

    let parent = find_node_mut(root, &parent_path).ok_or(FsError::NotFound)?;
    if parent.children.iter().any(|c| c.name == new_name[0]) {
        return Err(FsError::AlreadyExists);
    }
    parent.children.push(MemNode::new_dir(new_name[0]));
    Ok(())
}

/// Create or update a file
pub fn memfs_write(path: &str, data: &[u8]) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;

    // Try to find existing file
    if let Some(node) = find_node_mut(root, path) {
        if node.file_type == FileType::Directory {
            return Err(FsError::IsADirectory);
        }
        node.data = Vec::from(data);
        node.mtime = crate::time::clock::unix_time();
        return Ok(());
    }

    // Create new file in parent
    let parts = split_path(path);
    if parts.is_empty() {
        return Err(FsError::InvalidArgument);
    }
    let (parent_parts, new_name) = parts.split_at(parts.len() - 1);
    let parent_path: String = if parent_parts.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        p.push_str(&parent_parts.join("/"));
        p
    };

    let parent = find_node_mut(root, &parent_path).ok_or(FsError::NotFound)?;
    parent.children.push(MemNode::new_file(new_name[0], data));
    Ok(())
}

/// Write bytes to a file at a specific offset (grows the file as needed).
pub fn memfs_write_at(path: &str, offset: usize, data: &[u8]) -> Result<usize, FsError> {
    if data.is_empty() {
        return Ok(0);
    }

    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;
    let node = find_node_mut(root, path).ok_or(FsError::NotFound)?;
    if node.file_type == FileType::Directory {
        return Err(FsError::IsADirectory);
    }

    let end = offset.saturating_add(data.len());
    if node.data.len() < end {
        node.data.resize(end, 0);
    }
    node.data[offset..end].copy_from_slice(data);
    node.mtime = crate::time::clock::unix_time();
    Ok(data.len())
}

/// Remove a file or empty directory
pub fn memfs_rm(path: &str) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;

    let parts = split_path(path);
    if parts.is_empty() {
        return Err(FsError::InvalidArgument);
    }
    let (parent_parts, target_name) = parts.split_at(parts.len() - 1);
    let parent_path: String = if parent_parts.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        p.push_str(&parent_parts.join("/"));
        p
    };

    let parent = find_node_mut(root, &parent_path).ok_or(FsError::NotFound)?;
    let idx = parent
        .children
        .iter()
        .position(|c| c.name == target_name[0])
        .ok_or(FsError::NotFound)?;

    if parent.children[idx].file_type == FileType::Directory
        && !parent.children[idx].children.is_empty()
    {
        return Err(FsError::NotEmpty);
    }

    // Invalidate dcache
    dcache_invalidate(&parent_path, target_name[0]);

    parent.children.remove(idx);
    Ok(())
}

/// Stat a file (returns file_type, size, ino, mode, uid, gid, nlink, atime, mtime, ctime)
pub fn memfs_stat(path: &str) -> Result<(FileType, u64), FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    Ok((node.file_type, node.data.len() as u64))
}

/// Extended stat returning full inode metadata
pub fn memfs_stat_full(path: &str) -> Result<MemNode, FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    Ok(node.clone())
}

// ============================================================================
// Symlinks
// ============================================================================

/// Create a symbolic link
pub fn memfs_symlink(path: &str, target: &str) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;

    let parts = split_path(path);
    if parts.is_empty() {
        return Err(FsError::InvalidArgument);
    }
    let (parent_parts, new_name) = parts.split_at(parts.len() - 1);
    let parent_path: String = if parent_parts.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        p.push_str(&parent_parts.join("/"));
        p
    };

    let parent = find_node_mut(root, &parent_path).ok_or(FsError::NotFound)?;
    if parent.children.iter().any(|c| c.name == new_name[0]) {
        return Err(FsError::AlreadyExists);
    }
    parent
        .children
        .push(MemNode::new_symlink(new_name[0], target));
    Ok(())
}

/// Read a symlink target
pub fn memfs_readlink(path: &str) -> Result<String, FsError> {
    let guard = MEMFS_ROOT.lock();
    let root = guard.as_ref().ok_or(FsError::NotSupported)?;
    let node = find_node(root, path).ok_or(FsError::NotFound)?;
    if node.file_type != FileType::Symlink {
        return Err(FsError::InvalidArgument);
    }
    Ok(String::from_utf8_lossy(&node.data).into_owned())
}

/// Resolve a path following symlinks (up to 8 levels deep)
pub fn memfs_resolve(path: &str) -> Result<String, FsError> {
    let mut resolved = String::from(path);
    for _ in 0..8 {
        let guard = MEMFS_ROOT.lock();
        let root = guard.as_ref().ok_or(FsError::NotSupported)?;
        if let Some(node) = find_node(root, &resolved) {
            if node.file_type == FileType::Symlink {
                let target = String::from_utf8_lossy(&node.data).into_owned();
                drop(guard);
                if target.starts_with('/') {
                    resolved = target;
                } else {
                    let parts = split_path(&resolved);
                    if parts.len() > 1 {
                        let parent: String = parts[..parts.len() - 1].join("/");
                        resolved = alloc::format!("/{}/{}", parent, target);
                    } else {
                        resolved = alloc::format!("/{}", target);
                    }
                }
                continue;
            }
        }
        break;
    }
    Ok(resolved)
}

// ============================================================================
// Hard links
// ============================================================================

/// Create a hard link: `link_path` becomes another name for the inode at
/// `target_path`.  Both names share the same data; `nlink` is incremented.
///
/// Cross-directory hard links within the memfs are supported because the
/// in-memory tree copies the data reference.  Symlink targets are NOT
/// followed (mirrors Linux behaviour for `link(2)`).
pub fn memfs_link(target_path: &str, link_path: &str) -> Result<(), FsError> {
    // Read the target's content and metadata first (immutable borrow).
    let (data, file_type, mode, uid, gid) = {
        let guard = MEMFS_ROOT.lock();
        let root = guard.as_ref().ok_or(FsError::NotSupported)?;
        let node = find_node(root, target_path).ok_or(FsError::NotFound)?;
        if node.file_type == FileType::Directory {
            // Hard links to directories are not allowed (EPERM).
            return Err(FsError::PermissionDenied);
        }
        if node.file_type == FileType::Symlink {
            return Err(FsError::NotSupported);
        }
        (
            node.data.clone(),
            node.file_type,
            node.mode,
            node.uid,
            node.gid,
        )
    };

    // Validate that link_path does not yet exist.
    {
        let guard = MEMFS_ROOT.lock();
        let root = guard.as_ref().ok_or(FsError::NotSupported)?;
        if find_node(root, link_path).is_some() {
            return Err(FsError::AlreadyExists);
        }
    }

    // Write the new link entry: a new node with the same data (copy).
    let parts = split_path(link_path);
    if parts.is_empty() {
        return Err(FsError::InvalidArgument);
    }
    let (parent_parts, new_name) = parts.split_at(parts.len() - 1);
    let parent_path: String = if parent_parts.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        p.push_str(&parent_parts.join("/"));
        p
    };

    let now = crate::time::clock::unix_time();
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;

    // Increment nlink on the original target.
    if let Some(target) = find_node_mut(root, target_path) {
        target.nlink = target.nlink.saturating_add(1);
        target.ctime = now;
    }

    // Add the new directory entry in the parent directory.
    let parent = find_node_mut(root, &parent_path).ok_or(FsError::NotFound)?;
    if parent.children.iter().any(|c| c.name == new_name[0]) {
        return Err(FsError::AlreadyExists);
    }
    let mut new_node = MemNode::new_file(new_name[0], &data);
    new_node.file_type = file_type;
    new_node.mode = mode;
    new_node.uid = uid;
    new_node.gid = gid;
    new_node.nlink = 2; // both the original and this link
    new_node.atime = now;
    new_node.mtime = now;
    new_node.ctime = now;
    parent.children.push(new_node);

    Ok(())
}

/// VFS hard-link wrapper (dispatches to memfs; can be extended to
/// mounted filesystem drivers in the future).
pub fn vfs_link(target_path: &str, link_path: &str) -> Result<(), FsError> {
    if is_proc_path(target_path)
        || is_sys_path(target_path)
        || is_proc_path(link_path)
        || is_sys_path(link_path)
    {
        return Err(FsError::PermissionDenied);
    }
    memfs_link(target_path, link_path)
}

// ============================================================================
// Rename with dcache invalidation
// ============================================================================

/// Rename a file or directory, invalidating both dcache entries on success.
///
/// This wraps the existing data-copy rename with proper dcache invalidation
/// so that subsequent lookups on either old or new path see the correct state.
pub fn vfs_rename(old_path: &str, new_path: &str) -> Result<(), FsError> {
    if is_proc_path(old_path)
        || is_sys_path(old_path)
        || is_proc_path(new_path)
        || is_sys_path(new_path)
    {
        return Err(FsError::PermissionDenied);
    }

    // Read source data
    let data = memfs_read(old_path)?;

    // Detect cross-directory rename vs. same-directory rename.
    let old_parts = split_path(old_path);
    let new_parts = split_path(new_path);

    let old_parent: String = if old_parts.len() > 1 {
        let mut p = String::from("/");
        p.push_str(&old_parts[..old_parts.len() - 1].join("/"));
        p
    } else {
        String::from("/")
    };
    let new_parent: String = if new_parts.len() > 1 {
        let mut p = String::from("/");
        p.push_str(&new_parts[..new_parts.len() - 1].join("/"));
        p
    } else {
        String::from("/")
    };

    let old_name = old_parts.last().copied().unwrap_or("");
    let new_name = new_parts.last().copied().unwrap_or("");

    // Verify new parent exists (will catch cross-directory moves to missing dirs).
    {
        let guard = MEMFS_ROOT.lock();
        let root = guard.as_ref().ok_or(FsError::NotSupported)?;
        find_node(root, &new_parent).ok_or(FsError::NotFound)?;
    }

    // Write to new path, then remove old.
    memfs_write(new_path, &data)?;
    memfs_rm(old_path)?;

    // Invalidate dcache for both ends of the rename.
    dcache_invalidate(&old_parent, old_name);
    dcache_invalidate(&new_parent, new_name);

    Ok(())
}

/// Change file permissions
pub fn memfs_chmod(path: &str, mode: u32) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;
    let node = find_node_mut(root, path).ok_or(FsError::NotFound)?;
    node.mode = mode;
    node.ctime = crate::time::clock::unix_time();
    Ok(())
}

/// Change file ownership
pub fn memfs_chown(path: &str, uid: u32, gid: u32) -> Result<(), FsError> {
    let mut guard = MEMFS_ROOT.lock();
    let root = guard.as_mut().ok_or(FsError::NotSupported)?;
    let node = find_node_mut(root, path).ok_or(FsError::NotFound)?;
    node.uid = uid;
    node.gid = gid;
    node.ctime = crate::time::clock::unix_time();
    Ok(())
}

// ============================================================================
// /proc filesystem -- dynamic process info (legacy dispatch, still used)
// ============================================================================

/// Read /proc entries dynamically
pub fn proc_read(path: &str) -> Result<Vec<u8>, FsError> {
    let parts = split_path(path);

    match parts.as_slice() {
        ["proc"] | [] => {
            // List PIDs
            let table = crate::process::pcb::PROCESS_TABLE.lock();
            let mut out = String::new();
            for (i, slot) in table.iter().enumerate() {
                if slot.is_some() {
                    out.push_str(&alloc::format!("{}\n", i));
                }
            }
            Ok(out.into_bytes())
        }
        ["proc", "uptime"] => {
            let secs = crate::time::clock::uptime_secs();
            Ok(alloc::format!("{}.00\n", secs).into_bytes())
        }
        ["proc", "meminfo"] => {
            let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
            let total = crate::memory::frame_allocator::MAX_MEMORY;
            let free = fa.free_count() * crate::memory::frame_allocator::FRAME_SIZE;
            let used = fa.used_count() * crate::memory::frame_allocator::FRAME_SIZE;
            drop(fa);
            Ok(alloc::format!(
                "MemTotal: {} kB\nMemFree: {} kB\nMemUsed: {} kB\n",
                total / 1024,
                free / 1024,
                used / 1024
            )
            .into_bytes())
        }
        ["proc", "version"] => Ok(b"Genesis 1.0.0 (Hoags Kernel) x86_64\n".to_vec()),
        ["proc", "cpuinfo"] => {
            Ok(b"processor\t: 0\nvendor\t\t: Hoags Inc\nmodel name\t: Genesis CPU\n".to_vec())
        }
        ["proc", "mounts"] => {
            // Dynamic mounts from mount table
            let mounts = list_mounts();
            let mut out = String::new();
            for (path, fstype) in &mounts {
                out.push_str(&alloc::format!("{} {} {} rw 0 0\n", fstype, path, fstype));
            }
            if out.is_empty() {
                out.push_str("memfs / memfs rw 0 0\n");
            }
            Ok(out.into_bytes())
        }
        ["proc", "self", subpath] => {
            // /proc/self -> current process
            let pid = crate::process::getpid();
            let redir = alloc::format!("proc/{}/{}", pid, subpath);
            proc_read(&redir)
        }
        ["proc", pid_str, "status"] => {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if let Some(info) = super::procfs::pid_status(pid) {
                    return Ok(info.into_bytes());
                }
            }
            Err(FsError::NotFound)
        }
        ["proc", pid_str, "cmdline"] => {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if let Some(info) = super::procfs::pid_cmdline(pid) {
                    return Ok(info.into_bytes());
                }
            }
            Err(FsError::NotFound)
        }
        ["proc", pid_str, "maps"] => {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if let Some(info) = super::procfs::pid_maps(pid) {
                    return Ok(info.into_bytes());
                }
            }
            Err(FsError::NotFound)
        }
        // ── /proc/interrupts / /proc/ioports / /proc/iomem ──────────────────
        ["proc", "interrupts"] => Ok(super::procfs::interrupts().into_bytes()),
        ["proc", "ioports"] => Ok(super::procfs::read("/proc/ioports")
            .unwrap_or_default()
            .into_bytes()),
        ["proc", "iomem"] => Ok(super::procfs::read("/proc/iomem")
            .unwrap_or_default()
            .into_bytes()),
        // ── /proc/stat ───────────────────────────────────────────────────────
        ["proc", "stat"] => Ok(super::procfs::stat().into_bytes()),
        // ── /proc/loadavg ────────────────────────────────────────────────────
        ["proc", "loadavg"] => Ok(super::procfs::loadavg().into_bytes()),
        // ── /proc/<pid> (catch-all for numeric PIDs) ─────────────────────────
        ["proc", pid_str] => {
            if let Ok(pid) = pid_str.parse::<usize>() {
                let table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(Some(proc_entry)) = table.get(pid) {
                    let info = alloc::format!(
                        "Name: {}\nState: {:?}\nPid: {}\nPPid: {}\nPgid: {}\n",
                        proc_entry.name,
                        proc_entry.state,
                        proc_entry.pid,
                        proc_entry.parent_pid,
                        proc_entry.pgid
                    );
                    return Ok(info.into_bytes());
                }
            }
            Err(FsError::NotFound)
        }
        // ── /proc/net/* ──────────────────────────────────────────────────────
        ["proc", "net", "dev"] => Ok(super::procfs::read("/proc/net/dev")
            .unwrap_or_default()
            .into_bytes()),
        ["proc", "net", "route"] => Ok(super::procfs::read("/proc/net/route")
            .unwrap_or_default()
            .into_bytes()),
        ["proc", "net", "if_inet6"] => Ok(super::procfs::read("/proc/net/if_inet6")
            .unwrap_or_default()
            .into_bytes()),
        // ── /proc/sys/kernel/* ───────────────────────────────────────────────
        ["proc", "sys", "kernel", "hostname"] => {
            Ok(super::procfs::read("/proc/sys/kernel/hostname")
                .unwrap_or_default()
                .into_bytes())
        }
        ["proc", "sys", "kernel", "ostype"] => Ok(b"Linux\n".to_vec()),
        ["proc", "sys", "kernel", "osrelease"] => Ok(b"6.1.0-genesis\n".to_vec()),
        // ── /proc/sys/vm/* ───────────────────────────────────────────────────
        ["proc", "sys", "vm", "overcommit_memory"] => {
            Ok(super::procfs::read("/proc/sys/vm/overcommit_memory")
                .unwrap_or_default()
                .into_bytes())
        }
        // ── Fallback: delegate to procfs::read() for all remaining /proc paths
        _ => {
            if let Some(content) = super::procfs::read(path) {
                return Ok(content.into_bytes());
            }
            Err(FsError::NotFound)
        }
    }
}

// ============================================================================
// Disk buffer cache (block cache for filesystem I/O)
// ============================================================================

const CACHE_BLOCK_SIZE: usize = 512;
const CACHE_MAX_BLOCKS: usize = 256;

struct CacheBlock {
    sector: u64,
    data: [u8; CACHE_BLOCK_SIZE],
    dirty: bool,
    valid: bool,
    access_count: u32,
}

impl CacheBlock {
    const fn empty() -> Self {
        CacheBlock {
            sector: 0,
            data: [0; CACHE_BLOCK_SIZE],
            dirty: false,
            valid: false,
            access_count: 0,
        }
    }
}

static BLOCK_CACHE: Mutex<[CacheBlock; CACHE_MAX_BLOCKS]> =
    Mutex::new([const { CacheBlock::empty() }; CACHE_MAX_BLOCKS]);

/// Read a sector through the buffer cache
pub fn cached_read(sector: u64) -> Result<[u8; CACHE_BLOCK_SIZE], FsError> {
    let mut cache = BLOCK_CACHE.lock();

    // Check if already cached
    for block in cache.iter_mut() {
        if block.valid && block.sector == sector {
            block.access_count = block.access_count.saturating_add(1);
            return Ok(block.data);
        }
    }

    // Cache miss -- find a slot (LRU eviction: least access_count)
    let mut min_idx = 0;
    let mut min_count = u32::MAX;
    for (i, block) in cache.iter().enumerate() {
        if !block.valid {
            min_idx = i;
            break;
        }
        if block.access_count < min_count {
            min_count = block.access_count;
            min_idx = i;
        }
    }

    // Evict if dirty
    if cache[min_idx].dirty {
        let _ =
            crate::drivers::ata::write_sectors(0, cache[min_idx].sector, 1, &cache[min_idx].data);
        cache[min_idx].dirty = false;
    }

    // Read from disk (via ATA)
    let mut data = [0u8; CACHE_BLOCK_SIZE];
    let _drives = crate::drivers::ata::drives();
    let _ = crate::drivers::ata::read_sectors(0, sector, 1, &mut data);

    cache[min_idx].sector = sector;
    cache[min_idx].data = data;
    cache[min_idx].valid = true;
    cache[min_idx].dirty = false;
    cache[min_idx].access_count = 1;

    Ok(data)
}

/// Write a sector through the buffer cache
pub fn cached_write(sector: u64, data: &[u8; CACHE_BLOCK_SIZE]) -> Result<(), FsError> {
    let mut cache = BLOCK_CACHE.lock();

    // Update if cached
    for block in cache.iter_mut() {
        if block.valid && block.sector == sector {
            block.data = *data;
            block.dirty = true;
            block.access_count = block.access_count.saturating_add(1);
            return Ok(());
        }
    }

    // Not cached -- insert
    let mut min_idx = 0;
    let mut min_count = u32::MAX;
    for (i, block) in cache.iter().enumerate() {
        if !block.valid {
            min_idx = i;
            break;
        }
        if block.access_count < min_count {
            min_count = block.access_count;
            min_idx = i;
        }
    }

    cache[min_idx].sector = sector;
    cache[min_idx].data = *data;
    cache[min_idx].valid = true;
    cache[min_idx].dirty = true;
    cache[min_idx].access_count = 1;

    Ok(())
}

/// Flush all dirty blocks to disk
pub fn cache_sync() {
    let mut cache = BLOCK_CACHE.lock();
    for block in cache.iter_mut() {
        if block.valid && block.dirty {
            let _ = crate::drivers::ata::write_sectors(0, block.sector, 1, &block.data);
            block.dirty = false;
        }
    }
}

// ============================================================================
// inotify -- file change notifications
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InotifyEvent {
    Create,
    Delete,
    Modify,
    Open,
    Close,
    MovedFrom,
    MovedTo,
}

struct InotifyWatch {
    path: String,
    events: Vec<(InotifyEvent, String)>,
    _mask: u32,
}

#[allow(dead_code)]
const INOTIFY_CREATE: u32 = 0x100;
#[allow(dead_code)]
const INOTIFY_DELETE: u32 = 0x200;
#[allow(dead_code)]
const INOTIFY_MODIFY: u32 = 0x002;

static INOTIFY_WATCHES: Mutex<Vec<InotifyWatch>> = Mutex::new(Vec::new());

/// Add an inotify watch on a path
pub fn inotify_add_watch(path: &str, mask: u32) -> usize {
    let mut watches = INOTIFY_WATCHES.lock();
    let id = watches.len();
    watches.push(InotifyWatch {
        path: String::from(path),
        events: Vec::new(),
        _mask: mask,
    });
    id
}

/// Get pending events for a watch
pub fn inotify_read(watch_id: usize) -> Vec<(InotifyEvent, String)> {
    let mut watches = INOTIFY_WATCHES.lock();
    if let Some(watch) = watches.get_mut(watch_id) {
        let events = watch.events.clone();
        watch.events.clear();
        events
    } else {
        Vec::new()
    }
}

/// Notify inotify watches about a filesystem event
pub fn inotify_notify(path: &str, event: InotifyEvent, name: &str) {
    let mut watches = INOTIFY_WATCHES.lock();
    for watch in watches.iter_mut() {
        if path.starts_with(&watch.path) {
            watch.events.push((event, String::from(name)));
        }
    }
}
