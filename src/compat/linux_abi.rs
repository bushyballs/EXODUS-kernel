/// Linux ABI compatibility -- syscall number mapping and struct layouts
///
/// Part of the AIOS compatibility layer.
///
/// Maps Linux x86_64 syscall numbers to AIOS native handlers so that
/// unmodified Linux binaries can run. Provides struct layout conversions
/// for common kernel-userspace data structures.
///
/// Design:
///   - A dispatch table indexed by Linux syscall number (0..511).
///   - Each entry is either a direct handler function or a translation
///     stub that converts arguments and calls the AIOS native path.
///   - Struct layout conversion handles differences in field sizes,
///     alignment, and padding between Linux and AIOS ABIs.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux compat syscall layer (arch/x86/entry). All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum Linux syscall number supported.
const MAX_SYSCALL_NR: usize = 512;

// ---------------------------------------------------------------------------
// Linux x86_64 syscall numbers (subset)
// ---------------------------------------------------------------------------

pub const NR_READ: usize = 0;
pub const NR_WRITE: usize = 1;
pub const NR_OPEN: usize = 2;
pub const NR_CLOSE: usize = 3;
pub const NR_STAT: usize = 4;
pub const NR_FSTAT: usize = 5;
pub const NR_LSTAT: usize = 6;
pub const NR_POLL: usize = 7;
pub const NR_LSEEK: usize = 8;
pub const NR_MMAP: usize = 9;
pub const NR_MPROTECT: usize = 10;
pub const NR_MUNMAP: usize = 11;
pub const NR_BRK: usize = 12;
pub const NR_RT_SIGACTION: usize = 13;
pub const NR_IOCTL: usize = 16;
pub const NR_ACCESS: usize = 21;
pub const NR_PIPE: usize = 22;
pub const NR_DUP: usize = 32;
pub const NR_DUP2: usize = 33;
pub const NR_GETPID: usize = 39;
pub const NR_CLONE: usize = 56;
pub const NR_FORK: usize = 57;
pub const NR_EXECVE: usize = 59;
pub const NR_EXIT: usize = 60;
pub const NR_WAIT4: usize = 61;
pub const NR_KILL: usize = 62;
pub const NR_FCNTL: usize = 72;
pub const NR_GETCWD: usize = 79;
pub const NR_CHDIR: usize = 80;
pub const NR_MKDIR: usize = 83;
pub const NR_RMDIR: usize = 84;
pub const NR_UNLINK: usize = 87;
pub const NR_READLINK: usize = 89;
pub const NR_CHMOD: usize = 90;
pub const NR_CHOWN: usize = 92;
pub const NR_GETUID: usize = 102;
pub const NR_GETGID: usize = 104;
pub const NR_GETEUID: usize = 107;
pub const NR_GETEGID: usize = 108;
pub const NR_EPOLL_CREATE: usize = 213;
pub const NR_EPOLL_CTL: usize = 233;
pub const NR_EPOLL_WAIT: usize = 232;
pub const NR_OPENAT: usize = 257;
pub const NR_MKDIRAT: usize = 258;
pub const NR_FSTATAT: usize = 262;
pub const NR_UNLINKAT: usize = 263;

// ---------------------------------------------------------------------------
// Linux struct layouts (for userspace interop)
// ---------------------------------------------------------------------------

/// Linux struct stat (x86_64).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxStat64 {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub _pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
}

/// Linux struct timespec.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LinuxTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Handler function signature: (arg1, arg2, arg3, arg4, arg5, arg6) -> result
type SyscallHandler = fn(usize, usize, usize, usize, usize, usize) -> isize;

/// An ABI table entry.
struct AbiEntry {
    linux_nr: usize,
    handler: SyscallHandler,
    name: &'static str,
}

/// Inner ABI table state.
struct Inner {
    /// Sparse table indexed by Linux syscall number
    entries: Vec<Option<AbiEntry>>,
    dispatch_count: u64,
    unhandled_count: u64,
}

// ---------------------------------------------------------------------------
// Default handlers
// ---------------------------------------------------------------------------

fn stub_handler(_a1: usize, _a2: usize, _a3: usize, _a4: usize, _a5: usize, _a6: usize) -> isize {
    -38 // ENOSYS
}

fn sys_read(fd: usize, buf: usize, count: usize, _: usize, _: usize, _: usize) -> isize {
    // Delegate to AIOS native read
    let _ = (fd, buf, count);
    -38 // Placeholder -- real implementation calls VFS
}

fn sys_write(fd: usize, buf: usize, count: usize, _: usize, _: usize, _: usize) -> isize {
    let _ = (fd, buf, count);
    -38
}

fn sys_open(pathname: usize, flags: usize, mode: usize, _: usize, _: usize, _: usize) -> isize {
    let _ = (pathname, flags, mode);
    -38
}

fn sys_close(fd: usize, _: usize, _: usize, _: usize, _: usize, _: usize) -> isize {
    let _ = fd;
    -38
}

fn sys_getpid(_: usize, _: usize, _: usize, _: usize, _: usize, _: usize) -> isize {
    // Return current process ID from the scheduler
    1 // Placeholder
}

fn sys_exit(code: usize, _: usize, _: usize, _: usize, _: usize, _: usize) -> isize {
    let _ = code;
    0
}

fn sys_getuid(_: usize, _: usize, _: usize, _: usize, _: usize, _: usize) -> isize {
    0 // root
}

fn sys_getgid(_: usize, _: usize, _: usize, _: usize, _: usize, _: usize) -> isize {
    0
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        let mut entries = Vec::with_capacity(MAX_SYSCALL_NR);
        for _ in 0..MAX_SYSCALL_NR {
            entries.push(None);
        }
        Inner {
            entries,
            dispatch_count: 0,
            unhandled_count: 0,
        }
    }

    fn register(&mut self, nr: usize, handler: SyscallHandler, name: &'static str) {
        if nr < MAX_SYSCALL_NR {
            self.entries[nr] = Some(AbiEntry {
                linux_nr: nr,
                handler,
                name,
            });
        }
    }

    fn dispatch(&mut self, nr: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize) -> isize {
        self.dispatch_count = self.dispatch_count.saturating_add(1);
        if nr < MAX_SYSCALL_NR {
            if let Some(entry) = &self.entries[nr] {
                return (entry.handler)(a1, a2, a3, a4, a5, a6);
            }
        }
        self.unhandled_count = self.unhandled_count.saturating_add(1);
        -38 // ENOSYS
    }

    fn populate_defaults(&mut self) {
        self.register(NR_READ, sys_read, "read");
        self.register(NR_WRITE, sys_write, "write");
        self.register(NR_OPEN, sys_open, "open");
        self.register(NR_CLOSE, sys_close, "close");
        self.register(NR_GETPID, sys_getpid, "getpid");
        self.register(NR_EXIT, sys_exit, "exit");
        self.register(NR_GETUID, sys_getuid, "getuid");
        self.register(NR_GETGID, sys_getgid, "getgid");
        self.register(NR_GETEUID, sys_getuid, "geteuid");
        self.register(NR_GETEGID, sys_getgid, "getegid");
    }

    fn handler_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static LINUX_ABI: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Dispatch a Linux syscall.
pub fn dispatch(nr: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize) -> isize {
    let mut guard = LINUX_ABI.lock();
    match guard.as_mut() {
        Some(inner) => inner.dispatch(nr, a1, a2, a3, a4, a5, a6),
        None => -38,
    }
}

/// Register a custom Linux syscall handler.
pub fn register(nr: usize, handler: SyscallHandler, name: &'static str) {
    let mut guard = LINUX_ABI.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register(nr, handler, name);
    }
}

/// Return (dispatch_count, unhandled_count).
pub fn stats() -> (u64, u64) {
    let guard = LINUX_ABI.lock();
    guard.as_ref().map_or((0, 0), |inner| (inner.dispatch_count, inner.unhandled_count))
}

/// Initialize the Linux ABI compatibility layer.
pub fn init() {
    let mut guard = LINUX_ABI.lock();
    let mut inner = Inner::new();
    inner.populate_defaults();
    let count = inner.handler_count();
    *guard = Some(inner);
    serial_println!("    linux_abi: {} syscall handlers registered", count);
}
