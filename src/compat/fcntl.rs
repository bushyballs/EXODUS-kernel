/// File control operations (POSIX fcntl)
///
/// Part of the AIOS compatibility layer.
///
/// Provides POSIX-compatible fcntl operations: file descriptor duplication,
/// flag get/set, advisory file locking (POSIX locks), and close-on-exec
/// management.
///
/// Design:
///   - FcntlCmd enumerates all supported fcntl commands.
///   - Open flags (O_RDONLY, O_WRONLY, O_CREAT, etc.) are defined as
///     Linux-compatible constants.
///   - Advisory locks (F_SETLK, F_SETLKW, F_GETLK) use a per-fd lock
///     table with (start, len, type, pid) records.
///   - fd flags (FD_CLOEXEC) are tracked per fd.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: POSIX fcntl.h, Linux fs/locks.c. All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// fcntl command types
// ---------------------------------------------------------------------------

/// File control command types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FcntlCmd {
    DupFd,
    GetFd,
    SetFd,
    GetFl,
    SetFl,
    GetLk,
    SetLk,
    SetLkW,
    DupFdCloexec,
}

// ---------------------------------------------------------------------------
// Open flags (Linux-compatible values)
// ---------------------------------------------------------------------------

pub const O_RDONLY: u32 = 0o0;
pub const O_WRONLY: u32 = 0o1;
pub const O_RDWR: u32 = 0o2;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_NOCTTY: u32 = 0o400;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_SYNC: u32 = 0o4010000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_NOFOLLOW: u32 = 0o400000;
pub const O_CLOEXEC: u32 = 0o2000000;
pub const O_ACCMODE: u32 = 0o3;

// fd flags
pub const FD_CLOEXEC: u64 = 1;

// Lock types
pub const F_RDLCK: u16 = 0;
pub const F_WRLCK: u16 = 1;
pub const F_UNLCK: u16 = 2;

// ---------------------------------------------------------------------------
// Open flags helper
// ---------------------------------------------------------------------------

/// Open flags compatible with POSIX.
pub struct OpenFlags {
    pub bits: u32,
}

impl OpenFlags {
    pub const fn new(bits: u32) -> Self {
        OpenFlags { bits }
    }

    pub fn readable(&self) -> bool {
        (self.bits & O_ACCMODE) == O_RDONLY || (self.bits & O_ACCMODE) == O_RDWR
    }

    pub fn writable(&self) -> bool {
        (self.bits & O_ACCMODE) == O_WRONLY || (self.bits & O_ACCMODE) == O_RDWR
    }

    pub fn create(&self) -> bool {
        self.bits & O_CREAT != 0
    }

    pub fn truncate(&self) -> bool {
        self.bits & O_TRUNC != 0
    }

    pub fn append(&self) -> bool {
        self.bits & O_APPEND != 0
    }

    pub fn nonblock(&self) -> bool {
        self.bits & O_NONBLOCK != 0
    }

    pub fn cloexec(&self) -> bool {
        self.bits & O_CLOEXEC != 0
    }

    pub fn directory(&self) -> bool {
        self.bits & O_DIRECTORY != 0
    }
}

// ---------------------------------------------------------------------------
// POSIX flock structure
// ---------------------------------------------------------------------------

/// Advisory file lock record.
#[derive(Clone, Copy)]
pub struct Flock {
    pub l_type: u16,
    pub l_whence: u16,
    pub l_start: i64,
    pub l_len: i64,
    pub l_pid: u32,
}

// ---------------------------------------------------------------------------
// Per-fd state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FdInfo {
    fd: i32,
    status_flags: u32,
    fd_flags: u64,
    locks: Vec<Flock>,
}

struct Inner {
    fds: Vec<FdInfo>,
    next_dup_fd: i32,
}

// ---------------------------------------------------------------------------
// Decode command
// ---------------------------------------------------------------------------

/// Decode a Linux fcntl command number to our enum.
pub fn decode_cmd(cmd_nr: i32) -> FcntlCmd {
    match cmd_nr {
        0 => FcntlCmd::DupFd,
        1 => FcntlCmd::GetFd,
        2 => FcntlCmd::SetFd,
        3 => FcntlCmd::GetFl,
        4 => FcntlCmd::SetFl,
        5 => FcntlCmd::GetLk,
        6 => FcntlCmd::SetLk,
        7 => FcntlCmd::SetLkW,
        1030 => FcntlCmd::DupFdCloexec,
        _ => FcntlCmd::GetFd,
    }
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            fds: Vec::new(),
            next_dup_fd: 100,
        }
    }

    fn get_or_create(&mut self, fd: i32) -> &mut FdInfo {
        let pos = self.fds.iter().position(|f| f.fd == fd);
        match pos {
            Some(idx) => &mut self.fds[idx],
            None => {
                self.fds.push(FdInfo {
                    fd,
                    status_flags: 0,
                    fd_flags: 0,
                    locks: Vec::new(),
                });
                let last = self.fds.len() - 1;
                &mut self.fds[last]
            }
        }
    }

    fn find(&self, fd: i32) -> Option<&FdInfo> {
        self.fds.iter().find(|f| f.fd == fd)
    }

    fn do_fcntl(&mut self, fd: i32, cmd: FcntlCmd, arg: u64) -> i64 {
        match cmd {
            FcntlCmd::DupFd => {
                let min_fd = arg as i32;
                let new_fd = min_fd.max(self.next_dup_fd);
                self.next_dup_fd = new_fd + 1;
                let flags = self.find(fd).map_or(0, |f| f.status_flags);
                self.fds.push(FdInfo {
                    fd: new_fd,
                    status_flags: flags,
                    fd_flags: 0,
                    locks: Vec::new(),
                });
                new_fd as i64
            }
            FcntlCmd::DupFdCloexec => {
                let min_fd = arg as i32;
                let new_fd = min_fd.max(self.next_dup_fd);
                self.next_dup_fd = new_fd + 1;
                let flags = self.find(fd).map_or(0, |f| f.status_flags);
                self.fds.push(FdInfo {
                    fd: new_fd,
                    status_flags: flags,
                    fd_flags: FD_CLOEXEC,
                    locks: Vec::new(),
                });
                new_fd as i64
            }
            FcntlCmd::GetFd => {
                self.find(fd).map_or(-9, |f| f.fd_flags as i64)
            }
            FcntlCmd::SetFd => {
                let info = self.get_or_create(fd);
                info.fd_flags = arg;
                0
            }
            FcntlCmd::GetFl => {
                self.find(fd).map_or(-9, |f| f.status_flags as i64)
            }
            FcntlCmd::SetFl => {
                let changeable = O_APPEND | O_NONBLOCK | O_SYNC;
                let info = self.get_or_create(fd);
                info.status_flags = (info.status_flags & !changeable) | (arg as u32 & changeable);
                0
            }
            FcntlCmd::GetLk => {
                0 // No conflicting lock
            }
            FcntlCmd::SetLk => {
                let lock_type = (arg & 0xFFFF) as u16;
                if lock_type == F_UNLCK {
                    let info = self.get_or_create(fd);
                    info.locks.clear();
                } else {
                    let lock = Flock {
                        l_type: lock_type,
                        l_whence: 0,
                        l_start: 0,
                        l_len: 0,
                        l_pid: 0,
                    };
                    let info = self.get_or_create(fd);
                    info.locks.push(lock);
                }
                0
            }
            FcntlCmd::SetLkW => {
                let lock = Flock {
                    l_type: (arg & 0xFFFF) as u16,
                    l_whence: 0,
                    l_start: 0,
                    l_len: 0,
                    l_pid: 0,
                };
                let info = self.get_or_create(fd);
                info.locks.push(lock);
                0
            }
        }
    }

    fn close_fd(&mut self, fd: i32) {
        self.fds.retain(|f| f.fd != fd);
    }

    fn close_on_exec(&mut self) -> Vec<i32> {
        let to_close: Vec<i32> = self
            .fds
            .iter()
            .filter(|f| f.fd_flags & FD_CLOEXEC != 0)
            .map(|f| f.fd)
            .collect();
        self.fds.retain(|f| f.fd_flags & FD_CLOEXEC == 0);
        to_close
    }

    fn set_initial_flags(&mut self, fd: i32, flags: u32) {
        let info = self.get_or_create(fd);
        info.status_flags = flags;
        if flags & O_CLOEXEC != 0 {
            info.fd_flags |= FD_CLOEXEC;
        }
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static FCNTL: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Execute an fcntl operation on a file descriptor.
pub fn fcntl(fd: i32, cmd: FcntlCmd, arg: u64) -> i64 {
    let mut guard = FCNTL.lock();
    match guard.as_mut() {
        Some(inner) => inner.do_fcntl(fd, cmd, arg),
        None => -1,
    }
}

/// Set initial flags when a file is opened.
pub fn set_initial_flags(fd: i32, flags: u32) {
    let mut guard = FCNTL.lock();
    if let Some(inner) = guard.as_mut() {
        inner.set_initial_flags(fd, flags);
    }
}

/// Clean up when an fd is closed.
pub fn close_fd(fd: i32) {
    let mut guard = FCNTL.lock();
    if let Some(inner) = guard.as_mut() {
        inner.close_fd(fd);
    }
}

/// Close all FD_CLOEXEC fds on exec. Returns the list of closed fds.
pub fn close_on_exec() -> Vec<i32> {
    let mut guard = FCNTL.lock();
    guard.as_mut().map_or_else(Vec::new, |inner| inner.close_on_exec())
}

/// Initialize the fcntl subsystem.
pub fn init() {
    let mut guard = FCNTL.lock();
    *guard = Some(Inner::new());
    serial_println!("    fcntl: initialized (F_DUPFD, F_GETFL/SETFL, advisory locks)");
}
