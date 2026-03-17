/// POSIX API compatibility shims
///
/// Part of the AIOS compatibility layer.
///
/// Maps POSIX-standard API calls to native AIOS syscalls. Userspace programs
/// compiled against a POSIX libc can use these shims transparently.
///
/// Design:
///   - A mapping table pairs POSIX function names to native AIOS syscall numbers.
///   - Each mapping can have argument translation rules (reorder, resize, etc.).
///   - The table is populated at init() and consulted during syscall dispatch.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: FreeBSD linux_syscall.c, WSL syscall translation. All code is original.

use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Argument translation mode for a POSIX -> AIOS mapping.
#[derive(Clone, Copy, PartialEq)]
pub enum ArgMode {
    /// Arguments pass through unchanged.
    Direct,
    /// Arguments are reordered (e.g. swap arg1 and arg2).
    Reorder,
    /// A fixed value is prepended as the first argument.
    PrependConst(usize),
}

/// A single POSIX-to-AIOS mapping entry.
#[derive(Clone)]
struct PosixMapping {
    /// POSIX function name (e.g. "open", "read", "write")
    name: String,
    /// Native AIOS syscall number
    syscall_nr: usize,
    /// Argument translation mode
    arg_mode: ArgMode,
    /// Minimum number of arguments required
    min_args: u8,
}

/// Inner state for the POSIX shim layer.
struct Inner {
    mappings: Vec<PosixMapping>,
    call_count: u64,
    miss_count: u64,
}

// ---------------------------------------------------------------------------
// Standard POSIX syscall numbers (AIOS-native)
// ---------------------------------------------------------------------------

const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_CLOSE: usize = 3;
const SYS_STAT: usize = 4;
const SYS_FSTAT: usize = 5;
const SYS_LSTAT: usize = 6;
const SYS_LSEEK: usize = 8;
const SYS_MMAP: usize = 9;
const SYS_MPROTECT: usize = 10;
const SYS_MUNMAP: usize = 11;
const SYS_BRK: usize = 12;
const SYS_IOCTL: usize = 16;
const SYS_ACCESS: usize = 21;
const SYS_PIPE: usize = 22;
const SYS_DUP: usize = 32;
const SYS_DUP2: usize = 33;
const SYS_GETPID: usize = 39;
const SYS_FORK: usize = 57;
const SYS_EXECVE: usize = 59;
const SYS_EXIT: usize = 60;
const SYS_WAIT4: usize = 61;
const SYS_KILL: usize = 62;
const SYS_GETCWD: usize = 79;
const SYS_CHDIR: usize = 80;
const SYS_MKDIR: usize = 83;
const SYS_RMDIR: usize = 84;
const SYS_UNLINK: usize = 87;
const SYS_RENAME: usize = 82;
const SYS_LINK: usize = 86;
const SYS_SYMLINK: usize = 88;
const SYS_READLINK: usize = 89;
const SYS_CHMOD: usize = 90;
const SYS_CHOWN: usize = 92;
const SYS_GETUID: usize = 102;
const SYS_GETGID: usize = 104;
const SYS_GETEUID: usize = 107;
const SYS_GETEGID: usize = 108;

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            mappings: Vec::new(),
            call_count: 0,
            miss_count: 0,
        }
    }

    fn register(&mut self, name: &str, syscall_nr: usize, arg_mode: ArgMode, min_args: u8) {
        self.mappings.push(PosixMapping {
            name: String::from(name),
            syscall_nr,
            arg_mode,
            min_args,
        });
    }

    fn translate(&mut self, name: &str) -> Option<(usize, ArgMode)> {
        self.call_count = self.call_count.saturating_add(1);
        for m in self.mappings.iter() {
            if m.name == name {
                return Some((m.syscall_nr, m.arg_mode));
            }
        }
        self.miss_count = self.miss_count.saturating_add(1);
        None
    }

    fn lookup_by_nr(&self, nr: usize) -> Option<&PosixMapping> {
        self.mappings.iter().find(|m| m.syscall_nr == nr)
    }

    fn populate_defaults(&mut self) {
        self.register("read", SYS_READ, ArgMode::Direct, 3);
        self.register("write", SYS_WRITE, ArgMode::Direct, 3);
        self.register("open", SYS_OPEN, ArgMode::Direct, 2);
        self.register("close", SYS_CLOSE, ArgMode::Direct, 1);
        self.register("stat", SYS_STAT, ArgMode::Direct, 2);
        self.register("fstat", SYS_FSTAT, ArgMode::Direct, 2);
        self.register("lstat", SYS_LSTAT, ArgMode::Direct, 2);
        self.register("lseek", SYS_LSEEK, ArgMode::Direct, 3);
        self.register("mmap", SYS_MMAP, ArgMode::Direct, 6);
        self.register("mprotect", SYS_MPROTECT, ArgMode::Direct, 3);
        self.register("munmap", SYS_MUNMAP, ArgMode::Direct, 2);
        self.register("brk", SYS_BRK, ArgMode::Direct, 1);
        self.register("ioctl", SYS_IOCTL, ArgMode::Direct, 3);
        self.register("access", SYS_ACCESS, ArgMode::Direct, 2);
        self.register("pipe", SYS_PIPE, ArgMode::Direct, 1);
        self.register("dup", SYS_DUP, ArgMode::Direct, 1);
        self.register("dup2", SYS_DUP2, ArgMode::Direct, 2);
        self.register("getpid", SYS_GETPID, ArgMode::Direct, 0);
        self.register("fork", SYS_FORK, ArgMode::Direct, 0);
        self.register("execve", SYS_EXECVE, ArgMode::Direct, 3);
        self.register("exit", SYS_EXIT, ArgMode::Direct, 1);
        self.register("wait4", SYS_WAIT4, ArgMode::Direct, 4);
        self.register("kill", SYS_KILL, ArgMode::Direct, 2);
        self.register("getcwd", SYS_GETCWD, ArgMode::Direct, 2);
        self.register("chdir", SYS_CHDIR, ArgMode::Direct, 1);
        self.register("mkdir", SYS_MKDIR, ArgMode::Direct, 2);
        self.register("rmdir", SYS_RMDIR, ArgMode::Direct, 1);
        self.register("unlink", SYS_UNLINK, ArgMode::Direct, 1);
        self.register("rename", SYS_RENAME, ArgMode::Direct, 2);
        self.register("link", SYS_LINK, ArgMode::Direct, 2);
        self.register("symlink", SYS_SYMLINK, ArgMode::Direct, 2);
        self.register("readlink", SYS_READLINK, ArgMode::Direct, 3);
        self.register("chmod", SYS_CHMOD, ArgMode::Direct, 2);
        self.register("chown", SYS_CHOWN, ArgMode::Direct, 3);
        self.register("getuid", SYS_GETUID, ArgMode::Direct, 0);
        self.register("getgid", SYS_GETGID, ArgMode::Direct, 0);
        self.register("geteuid", SYS_GETEUID, ArgMode::Direct, 0);
        self.register("getegid", SYS_GETEGID, ArgMode::Direct, 0);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static POSIX_SHIM: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Translate a POSIX call name to a native AIOS syscall number.
pub fn translate(name: &str) -> Option<usize> {
    let mut guard = POSIX_SHIM.lock();
    guard.as_mut().and_then(|inner| inner.translate(name).map(|(nr, _)| nr))
}

/// Translate with full info (syscall number + argument mode).
pub fn translate_full(name: &str) -> Option<(usize, ArgMode)> {
    let mut guard = POSIX_SHIM.lock();
    guard.as_mut().and_then(|inner| inner.translate(name))
}

/// Register a custom POSIX -> AIOS mapping.
pub fn register(name: &str, syscall_nr: usize, arg_mode: ArgMode, min_args: u8) {
    let mut guard = POSIX_SHIM.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register(name, syscall_nr, arg_mode, min_args);
    }
}

/// Return (call_count, miss_count) for diagnostics.
pub fn stats() -> (u64, u64) {
    let guard = POSIX_SHIM.lock();
    guard.as_ref().map_or((0, 0), |inner| (inner.call_count, inner.miss_count))
}

/// Return the total number of registered mappings.
pub fn mapping_count() -> usize {
    let guard = POSIX_SHIM.lock();
    guard.as_ref().map_or(0, |inner| inner.mappings.len())
}

/// Initialize the POSIX compatibility shim.
pub fn init() {
    let mut guard = POSIX_SHIM.lock();
    let mut inner = Inner::new();
    inner.populate_defaults();
    let count = inner.mappings.len();
    *guard = Some(inner);
    serial_println!("    posix: {} POSIX API mappings registered", count);
}
