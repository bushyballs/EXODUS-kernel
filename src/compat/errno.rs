/// POSIX errno definitions and error mapping
///
/// Part of the AIOS compatibility layer.
///
/// Provides the complete POSIX errno table and bidirectional conversion
/// between AIOS internal error codes and POSIX errno values. Userspace
/// programs see standard errno values; the kernel uses its own error
/// representation internally.
///
/// Design:
///   - Errno enum with repr(i32) for direct ABI compatibility.
///   - Conversion functions: AIOS error -> Errno, Errno -> &str.
///   - Per-process errno storage for thread-local-like access.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: POSIX errno.h, Linux include/uapi/asm-generic/errno.h.
/// All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Errno values
// ---------------------------------------------------------------------------

/// Standard POSIX error numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Errno {
    Success = 0,
    EPERM = 1,
    ENOENT = 2,
    ESRCH = 3,
    EINTR = 4,
    EIO = 5,
    ENXIO = 6,
    E2BIG = 7,
    ENOEXEC = 8,
    EBADF = 9,
    ECHILD = 10,
    EAGAIN = 11,
    ENOMEM = 12,
    EACCES = 13,
    EFAULT = 14,
    ENOTBLK = 15,
    EBUSY = 16,
    EEXIST = 17,
    EXDEV = 18,
    ENODEV = 19,
    ENOTDIR = 20,
    EISDIR = 21,
    EINVAL = 22,
    ENFILE = 23,
    EMFILE = 24,
    ENOTTY = 25,
    ETXTBSY = 26,
    EFBIG = 27,
    ENOSPC = 28,
    ESPIPE = 29,
    EROFS = 30,
    EMLINK = 31,
    EPIPE = 32,
    EDOM = 33,
    ERANGE = 34,
    EDEADLK = 35,
    ENAMETOOLONG = 36,
    ENOLCK = 37,
    ENOSYS = 38,
    ENOTEMPTY = 39,
    ELOOP = 40,
    ENOMSG = 42,
    EIDRM = 43,
    ENOSTR = 60,
    ENODATA = 61,
    ETIME = 62,
    ENOSR = 63,
    ENOLINK = 67,
    EPROTO = 71,
    EOVERFLOW = 75,
    EILSEQ = 84,
    ENOTSOCK = 88,
    EDESTADDRREQ = 89,
    EMSGSIZE = 90,
    EPROTOTYPE = 91,
    ENOPROTOOPT = 92,
    EPROTONOSUPPORT = 93,
    EOPNOTSUPP = 95,
    EAFNOSUPPORT = 97,
    EADDRINUSE = 98,
    EADDRNOTAVAIL = 99,
    ENETDOWN = 100,
    ENETUNREACH = 101,
    ECONNABORTED = 103,
    ECONNRESET = 104,
    ENOBUFS = 105,
    EISCONN = 106,
    ENOTCONN = 107,
    ETIMEDOUT = 110,
    ECONNREFUSED = 111,
    EHOSTUNREACH = 113,
    EALREADY = 114,
    EINPROGRESS = 115,
    ESTALE = 116,
}

// ---------------------------------------------------------------------------
// Conversion: i32 <-> Errno
// ---------------------------------------------------------------------------

impl Errno {
    /// Convert from a raw i32 value (absolute value is used).
    pub fn from_i32(val: i32) -> Self {
        let abs = if val < 0 { -val } else { val };
        match abs {
            0 => Errno::Success,
            1 => Errno::EPERM,
            2 => Errno::ENOENT,
            3 => Errno::ESRCH,
            4 => Errno::EINTR,
            5 => Errno::EIO,
            6 => Errno::ENXIO,
            7 => Errno::E2BIG,
            8 => Errno::ENOEXEC,
            9 => Errno::EBADF,
            10 => Errno::ECHILD,
            11 => Errno::EAGAIN,
            12 => Errno::ENOMEM,
            13 => Errno::EACCES,
            14 => Errno::EFAULT,
            15 => Errno::ENOTBLK,
            16 => Errno::EBUSY,
            17 => Errno::EEXIST,
            18 => Errno::EXDEV,
            19 => Errno::ENODEV,
            20 => Errno::ENOTDIR,
            21 => Errno::EISDIR,
            22 => Errno::EINVAL,
            23 => Errno::ENFILE,
            24 => Errno::EMFILE,
            25 => Errno::ENOTTY,
            26 => Errno::ETXTBSY,
            27 => Errno::EFBIG,
            28 => Errno::ENOSPC,
            29 => Errno::ESPIPE,
            30 => Errno::EROFS,
            31 => Errno::EMLINK,
            32 => Errno::EPIPE,
            33 => Errno::EDOM,
            34 => Errno::ERANGE,
            35 => Errno::EDEADLK,
            36 => Errno::ENAMETOOLONG,
            37 => Errno::ENOLCK,
            38 => Errno::ENOSYS,
            39 => Errno::ENOTEMPTY,
            40 => Errno::ELOOP,
            61 => Errno::ENODATA,
            75 => Errno::EOVERFLOW,
            84 => Errno::EILSEQ,
            88 => Errno::ENOTSOCK,
            95 => Errno::EOPNOTSUPP,
            98 => Errno::EADDRINUSE,
            104 => Errno::ECONNRESET,
            110 => Errno::ETIMEDOUT,
            111 => Errno::ECONNREFUSED,
            116 => Errno::ESTALE,
            _ => Errno::EINVAL,
        }
    }

    /// Get the integer value.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Get as negative integer (kernel return convention: -errno).
    pub fn as_neg(self) -> i32 {
        -(self as i32)
    }

    /// Human-readable error string.
    pub fn strerror(self) -> &'static str {
        match self {
            Errno::Success => "Success",
            Errno::EPERM => "Operation not permitted",
            Errno::ENOENT => "No such file or directory",
            Errno::ESRCH => "No such process",
            Errno::EINTR => "Interrupted system call",
            Errno::EIO => "Input/output error",
            Errno::ENXIO => "No such device or address",
            Errno::E2BIG => "Argument list too long",
            Errno::ENOEXEC => "Exec format error",
            Errno::EBADF => "Bad file descriptor",
            Errno::ECHILD => "No child processes",
            Errno::EAGAIN => "Resource temporarily unavailable",
            Errno::ENOMEM => "Cannot allocate memory",
            Errno::EACCES => "Permission denied",
            Errno::EFAULT => "Bad address",
            Errno::ENOTBLK => "Block device required",
            Errno::EBUSY => "Device or resource busy",
            Errno::EEXIST => "File exists",
            Errno::EXDEV => "Invalid cross-device link",
            Errno::ENODEV => "No such device",
            Errno::ENOTDIR => "Not a directory",
            Errno::EISDIR => "Is a directory",
            Errno::EINVAL => "Invalid argument",
            Errno::ENFILE => "Too many open files in system",
            Errno::EMFILE => "Too many open files",
            Errno::ENOTTY => "Inappropriate ioctl for device",
            Errno::ETXTBSY => "Text file busy",
            Errno::EFBIG => "File too large",
            Errno::ENOSPC => "No space left on device",
            Errno::ESPIPE => "Illegal seek",
            Errno::EROFS => "Read-only file system",
            Errno::EMLINK => "Too many links",
            Errno::EPIPE => "Broken pipe",
            Errno::EDOM => "Numerical argument out of domain",
            Errno::ERANGE => "Numerical result out of range",
            Errno::EDEADLK => "Resource deadlock avoided",
            Errno::ENAMETOOLONG => "File name too long",
            Errno::ENOLCK => "No locks available",
            Errno::ENOSYS => "Function not implemented",
            Errno::ENOTEMPTY => "Directory not empty",
            Errno::ELOOP => "Too many levels of symbolic links",
            Errno::ENODATA => "No data available",
            Errno::EOVERFLOW => "Value too large for defined data type",
            Errno::EILSEQ => "Invalid or incomplete multibyte or wide character",
            Errno::ENOTSOCK => "Socket operation on non-socket",
            Errno::EOPNOTSUPP => "Operation not supported",
            Errno::EADDRINUSE => "Address already in use",
            Errno::ECONNRESET => "Connection reset by peer",
            Errno::ETIMEDOUT => "Connection timed out",
            Errno::ECONNREFUSED => "Connection refused",
            _ => "Unknown error",
        }
    }
}

/// Convert an Errno back to its numeric value.
pub fn errno_to_i32(e: Errno) -> i32 {
    e as i32
}

// ---------------------------------------------------------------------------
// AIOS internal error -> Errno mapping
// ---------------------------------------------------------------------------

/// Convert an AIOS internal error code to a POSIX errno.
///
/// AIOS internal error codes:
///   0   = success
///   -1  = general failure (EIO)
///   -2  = not found (ENOENT)
///   -3  = permission denied (EACCES)
///   -4  = out of memory (ENOMEM)
///   -5  = invalid argument (EINVAL)
///   -6  = not implemented (ENOSYS)
///   -7  = already exists (EEXIST)
///   -8  = bad file descriptor (EBADF)
///   -9  = is a directory (EISDIR)
///   -10 = not a directory (ENOTDIR)
///   -11 = device busy (EBUSY)
///   -12 = interrupted (EINTR)
///   other = EIO
pub fn to_errno(aios_error: i32) -> Errno {
    match aios_error {
        0 => Errno::Success,
        -1 => Errno::EIO,
        -2 => Errno::ENOENT,
        -3 => Errno::EACCES,
        -4 => Errno::ENOMEM,
        -5 => Errno::EINVAL,
        -6 => Errno::ENOSYS,
        -7 => Errno::EEXIST,
        -8 => Errno::EBADF,
        -9 => Errno::EISDIR,
        -10 => Errno::ENOTDIR,
        -11 => Errno::EBUSY,
        -12 => Errno::EINTR,
        _ => Errno::EIO,
    }
}

/// Convert a raw i32 (possibly negative) to Errno via absolute value lookup.
pub fn from_raw(raw: i32) -> Errno {
    Errno::from_i32(raw)
}

// ---------------------------------------------------------------------------
// Per-process errno storage
// ---------------------------------------------------------------------------

struct ErrnoEntry {
    pid: u32,
    value: Errno,
}

struct Inner {
    entries: Vec<ErrnoEntry>,
}

impl Inner {
    fn new() -> Self {
        Inner {
            entries: Vec::new(),
        }
    }

    fn set(&mut self, pid: u32, err: Errno) {
        for e in self.entries.iter_mut() {
            if e.pid == pid {
                e.value = err;
                return;
            }
        }
        self.entries.push(ErrnoEntry { pid, value: err });
    }

    fn get(&self, pid: u32) -> Errno {
        self.entries
            .iter()
            .find(|e| e.pid == pid)
            .map_or(Errno::Success, |e| e.value)
    }

    fn clear(&mut self, pid: u32) {
        for e in self.entries.iter_mut() {
            if e.pid == pid {
                e.value = Errno::Success;
                return;
            }
        }
    }

    fn remove_process(&mut self, pid: u32) {
        self.entries.retain(|e| e.pid != pid);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static ERRNO_STORE: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set errno for a process.
pub fn set_errno(pid: u32, err: Errno) {
    let mut guard = ERRNO_STORE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.set(pid, err);
    }
}

/// Get errno for a process.
pub fn get_errno(pid: u32) -> Errno {
    let guard = ERRNO_STORE.lock();
    guard.as_ref().map_or(Errno::Success, |inner| inner.get(pid))
}

/// Clear errno for a process (set to Success).
pub fn clear_errno(pid: u32) {
    let mut guard = ERRNO_STORE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.clear(pid);
    }
}

/// Clean up errno storage when a process exits.
pub fn cleanup(pid: u32) {
    let mut guard = ERRNO_STORE.lock();
    if let Some(inner) = guard.as_mut() {
        inner.remove_process(pid);
    }
}

/// Initialize the errno subsystem.
pub fn init() {
    let mut guard = ERRNO_STORE.lock();
    *guard = Some(Inner::new());
    serial_println!("    errno: initialized (POSIX errno table, per-process storage)");
}
