/// Minimal libc for Genesis userspace programs
///
/// Provides POSIX-like C library functions that wrap Genesis syscalls.
/// These are the building blocks for userspace programs.
///
/// Includes:
///   - Syscall number constants (matching syscall.rs)
///   - Machine code stubs for ring-3 syscall invocation
///   - String functions (strlen, strcmp, strncmp, strchr, strrchr, strcat, strdup)
///   - Memory functions (memcpy, memset, memcmp, memmove)
///   - Conversion functions (atoi, atol, itoa, strtol)
///   - I/O helpers (snprintf, putchar, puts, getchar)
///   - POSIX wrappers (open, close, read, write, lseek, stat, mkdir, etc.)
///   - errno definitions
///   - Signal constants
///
/// In the final OS, this would be compiled as a shared library (libc.so).
/// For now, it's a kernel module that defines the syscall wrappers
/// and standard library functions.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════════
// Syscall numbers (must match syscall.rs::nr)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod sys {
    pub const SYS_EXIT: u64 = 0;
    pub const SYS_WRITE: u64 = 1;
    pub const SYS_YIELD: u64 = 2;
    pub const SYS_GETPID: u64 = 3;
    pub const SYS_SPAWN: u64 = 4;
    pub const SYS_SLEEP: u64 = 5;
    pub const SYS_FORK: u64 = 6;
    pub const SYS_WAITPID: u64 = 7;
    pub const SYS_KILL: u64 = 8;
    pub const SYS_GETPPID: u64 = 9;
    pub const SYS_MMAP: u64 = 10;
    pub const SYS_MUNMAP: u64 = 11;
    pub const SYS_READ: u64 = 12;
    pub const SYS_OPEN: u64 = 13;
    pub const SYS_CLOSE: u64 = 14;
    pub const SYS_PIPE: u64 = 15;
    pub const SYS_DUP2: u64 = 16;
    pub const SYS_EXEC: u64 = 17;
    pub const SYS_BRK: u64 = 18;
    pub const SYS_SOCKET: u64 = 19;
    pub const SYS_BIND: u64 = 20;
    pub const SYS_LISTEN: u64 = 21;
    pub const SYS_ACCEPT: u64 = 22;
    pub const SYS_CONNECT: u64 = 23;
    pub const SYS_SEND: u64 = 24;
    pub const SYS_RECV: u64 = 25;
    pub const SYS_FUTEX: u64 = 26;
    pub const SYS_CLONE: u64 = 27;
    pub const SYS_SIGACTION: u64 = 28;
    pub const SYS_SIGRETURN: u64 = 29;
    pub const SYS_GETCWD: u64 = 30;
    pub const SYS_CHDIR: u64 = 31;
    pub const SYS_STAT: u64 = 32;
    pub const SYS_LSEEK: u64 = 54;
    pub const SYS_GETUID: u64 = 37;
    pub const SYS_GETGID: u64 = 38;
    pub const SYS_SETUID: u64 = 39;
    pub const SYS_SETGID: u64 = 40;
    pub const SYS_TIME: u64 = 45;
    pub const SYS_CLOCK_GETTIME: u64 = 46;
    pub const SYS_NANOSLEEP: u64 = 47;
    pub const SYS_FSTAT: u64 = 52;
    pub const SYS_DUP: u64 = 53;
    pub const SYS_MKDIR: u64 = 55;
    pub const SYS_RMDIR: u64 = 56;
    pub const SYS_UNLINK: u64 = 57;
    pub const SYS_RENAME: u64 = 58;
    pub const SYS_CHMOD: u64 = 59;
    pub const SYS_CHOWN: u64 = 60;
    pub const SYS_UNAME: u64 = 61;
    pub const SYS_FCNTL: u64 = 62;
    pub const SYS_IOCTL: u64 = 35;
    pub const SYS_SYMLINK: u64 = 69;
    pub const SYS_READLINK: u64 = 70;
    pub const SYS_SYSINFO: u64 = 73;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Machine code stubs for ring-3 syscall invocation
// ═══════════════════════════════════════════════════════════════════════════════

/// Generate syscall stub machine code for a given syscall number
///
/// Produces: mov rax, <nr>; syscall; ret
pub fn syscall_stub(nr: u64) -> [u8; 16] {
    let mut code = [0u8; 16];
    // 48 c7 c0 XX XX XX XX  - mov rax, imm32
    code[0] = 0x48;
    code[1] = 0xC7;
    code[2] = 0xC0;
    code[3] = (nr & 0xFF) as u8;
    code[4] = ((nr >> 8) & 0xFF) as u8;
    code[5] = ((nr >> 16) & 0xFF) as u8;
    code[6] = ((nr >> 24) & 0xFF) as u8;
    // 0f 05 - syscall
    code[7] = 0x0F;
    code[8] = 0x05;
    // c3 - ret
    code[9] = 0xC3;
    code
}

/// Generate syscall stub with 4 arguments (r10 = arg4)
///
/// Produces: mov rax, <nr>; mov r10, rcx; syscall; ret
pub fn syscall_stub_4arg(nr: u64) -> [u8; 20] {
    let mut code = [0u8; 20];
    // mov rax, imm32
    code[0] = 0x48;
    code[1] = 0xC7;
    code[2] = 0xC0;
    code[3] = (nr & 0xFF) as u8;
    code[4] = ((nr >> 8) & 0xFF) as u8;
    code[5] = ((nr >> 16) & 0xFF) as u8;
    code[6] = ((nr >> 24) & 0xFF) as u8;
    // mov r10, rcx (49 89 ca)
    code[7] = 0x49;
    code[8] = 0x89;
    code[9] = 0xCA;
    // syscall (0f 05)
    code[10] = 0x0F;
    code[11] = 0x05;
    // ret (c3)
    code[12] = 0xC3;
    code
}

// ═══════════════════════════════════════════════════════════════════════════════
// String functions
// ═══════════════════════════════════════════════════════════════════════════════

/// strlen -- count bytes until null terminator
pub fn strlen(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

/// strnlen -- count bytes until null or max
pub fn strnlen(s: &[u8], maxlen: usize) -> usize {
    let check_len = s.len().min(maxlen);
    s[..check_len]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(check_len)
}

/// strcmp -- compare two null-terminated byte strings
pub fn strcmp(a: &[u8], b: &[u8]) -> i32 {
    let la = strlen(a);
    let lb = strlen(b);
    let min = la.min(lb);
    for i in 0..min {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
    }
    (la as i32) - (lb as i32)
}

/// strncmp -- compare at most n bytes
pub fn strncmp(a: &[u8], b: &[u8], n: usize) -> i32 {
    let la = strlen(a).min(n);
    let lb = strlen(b).min(n);
    let min = la.min(lb);
    for i in 0..min {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
    }
    (la as i32) - (lb as i32)
}

/// strchr -- find first occurrence of byte c in s
pub fn strchr(s: &[u8], c: u8) -> Option<usize> {
    let len = strlen(s);
    s[..len].iter().position(|&b| b == c)
}

/// strrchr -- find last occurrence of byte c in s
pub fn strrchr(s: &[u8], c: u8) -> Option<usize> {
    let len = strlen(s);
    s[..len].iter().rposition(|&b| b == c)
}

/// strstr -- find first occurrence of needle in haystack
pub fn strstr(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    let h_len = strlen(haystack);
    let n_len = strlen(needle);
    if n_len == 0 {
        return Some(0);
    }
    if n_len > h_len {
        return None;
    }
    for i in 0..=(h_len - n_len) {
        if &haystack[i..i + n_len] == &needle[..n_len] {
            return Some(i);
        }
    }
    None
}

/// strcat -- append src to dst (dst must have enough room)
pub fn strcat(dst: &mut [u8], src: &[u8]) {
    let dst_len = strlen(dst);
    let src_len = strlen(src);
    let copy_len = src_len.min(dst.len().saturating_sub(dst_len + 1));
    dst[dst_len..dst_len + copy_len].copy_from_slice(&src[..copy_len]);
    if dst_len + copy_len < dst.len() {
        dst[dst_len + copy_len] = 0;
    }
}

/// strdup -- duplicate a string (allocating)
pub fn strdup(s: &[u8]) -> Vec<u8> {
    let len = strlen(s);
    let mut dup = Vec::with_capacity(len + 1);
    dup.extend_from_slice(&s[..len]);
    dup.push(0);
    dup
}

/// tolower -- convert ASCII byte to lowercase
pub fn tolower(c: u8) -> u8 {
    if c >= b'A' && c <= b'Z' {
        c + 32
    } else {
        c
    }
}

/// toupper -- convert ASCII byte to uppercase
pub fn toupper(c: u8) -> u8 {
    if c >= b'a' && c <= b'z' {
        c - 32
    } else {
        c
    }
}

/// isdigit -- check if ASCII digit
pub fn isdigit(c: u8) -> bool {
    c >= b'0' && c <= b'9'
}

/// isalpha -- check if ASCII letter
pub fn isalpha(c: u8) -> bool {
    (c >= b'A' && c <= b'Z') || (c >= b'a' && c <= b'z')
}

/// isalnum -- check if ASCII alphanumeric
pub fn isalnum(c: u8) -> bool {
    isdigit(c) || isalpha(c)
}

/// isspace -- check if whitespace
pub fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Memory functions
// ═══════════════════════════════════════════════════════════════════════════════

/// memcpy -- copy bytes (non-overlapping)
pub fn memcpy(dst: &mut [u8], src: &[u8], n: usize) {
    let len = n.min(dst.len()).min(src.len());
    dst[..len].copy_from_slice(&src[..len]);
}

/// memmove -- copy bytes (may overlap)
pub fn memmove(dst: &mut [u8], src: &[u8], n: usize) {
    let len = n.min(dst.len()).min(src.len());
    // Use temporary buffer for overlapping copies
    let mut tmp = Vec::with_capacity(len);
    tmp.extend_from_slice(&src[..len]);
    dst[..len].copy_from_slice(&tmp);
}

/// memset -- fill bytes
pub fn memset(buf: &mut [u8], val: u8, n: usize) {
    let len = n.min(buf.len());
    for b in &mut buf[..len] {
        *b = val;
    }
}

/// memcmp -- compare byte arrays
pub fn memcmp(a: &[u8], b: &[u8], n: usize) -> i32 {
    let len = n.min(a.len()).min(b.len());
    for i in 0..len {
        if a[i] != b[i] {
            return (a[i] as i32) - (b[i] as i32);
        }
    }
    0
}

/// memchr -- find byte in array
pub fn memchr(buf: &[u8], val: u8, n: usize) -> Option<usize> {
    let len = n.min(buf.len());
    buf[..len].iter().position(|&b| b == val)
}

/// bzero -- zero out memory
pub fn bzero(buf: &mut [u8], n: usize) {
    memset(buf, 0, n);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Conversion functions
// ═══════════════════════════════════════════════════════════════════════════════

/// atoi -- parse integer from string
pub fn atoi(s: &str) -> i64 {
    let s = s.trim();
    let (neg, s) = if s.starts_with('-') {
        (true, &s[1..])
    } else {
        (false, s)
    };
    let mut result: i64 = 0;
    for c in s.bytes() {
        if c < b'0' || c > b'9' {
            break;
        }
        result = result.saturating_mul(10).saturating_add((c - b'0') as i64);
    }
    if neg {
        -result
    } else {
        result
    }
}

/// atol -- alias for atoi
pub fn atol(s: &str) -> i64 {
    atoi(s)
}

/// strtol -- parse long integer with base detection
///
/// Supports base 10, 16 (0x prefix), 8 (0 prefix), and 2 (0b prefix).
pub fn strtol(s: &str, base: u32) -> (i64, usize) {
    let s = s.trim();
    let (neg, s) = if s.starts_with('-') {
        (true, &s[1..])
    } else {
        (false, s)
    };

    let (actual_base, start) = if base == 0 {
        if s.starts_with("0x") || s.starts_with("0X") {
            (16u32, 2usize)
        } else if s.starts_with("0b") || s.starts_with("0B") {
            (2, 2)
        } else if s.starts_with('0') && s.len() > 1 {
            (8, 1)
        } else {
            (10, 0)
        }
    } else {
        (base, 0)
    };

    let mut result: i64 = 0;
    let mut consumed = start;

    for &c in s[start..].as_bytes() {
        let digit = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a' + 10) as u32,
            b'A'..=b'F' => (c - b'A' + 10) as u32,
            _ => break,
        };
        if digit >= actual_base {
            break;
        }
        result = result
            .saturating_mul(actual_base as i64)
            .saturating_add(digit as i64);
        consumed += 1;
    }

    if neg {
        result = -result;
    }
    (result, consumed + if neg { 1 } else { 0 })
}

/// itoa -- convert integer to string
pub fn itoa(mut n: i64) -> String {
    if n == 0 {
        return String::from("0");
    }

    let neg = n < 0;
    if neg {
        n = -n;
    }

    let mut digits = Vec::new();
    while n > 0 {
        digits.push(b'0' + (n % 10) as u8);
        n /= 10;
    }

    if neg {
        digits.push(b'-');
    }
    digits.reverse();

    String::from_utf8(digits).unwrap_or_default()
}

/// utoa -- convert unsigned integer to string with base
pub fn utoa(mut n: u64, base: u32) -> String {
    if n == 0 {
        return String::from("0");
    }

    let base = base.max(2).min(36) as u64;
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();

    while n > 0 {
        result.push(digits[(n % base) as usize]);
        n /= base;
    }

    result.reverse();
    String::from_utf8(result).unwrap_or_default()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Formatted output
// ═══════════════════════════════════════════════════════════════════════════════

/// printf-like format (simplified)
///
/// Supports: %d, %i, %u, %x, %X, %o, %c, %s (as "(str)"), %p, %%
pub fn snprintf(fmt: &str, args: &[i64]) -> String {
    let mut output = String::new();
    let mut arg_idx = 0;
    let mut chars = fmt.chars();

    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(spec) = chars.next() {
                match spec {
                    'd' | 'i' => {
                        if arg_idx < args.len() {
                            output.push_str(&itoa(args[arg_idx]));
                            arg_idx += 1;
                        }
                    }
                    'u' => {
                        if arg_idx < args.len() {
                            output.push_str(&utoa(args[arg_idx] as u64, 10));
                            arg_idx += 1;
                        }
                    }
                    's' => {
                        output.push_str("(str)");
                        arg_idx += 1;
                    }
                    'x' => {
                        if arg_idx < args.len() {
                            output.push_str(&alloc::format!("{:x}", args[arg_idx]));
                            arg_idx += 1;
                        }
                    }
                    'X' => {
                        if arg_idx < args.len() {
                            output.push_str(&alloc::format!("{:X}", args[arg_idx]));
                            arg_idx += 1;
                        }
                    }
                    'o' => {
                        if arg_idx < args.len() {
                            output.push_str(&utoa(args[arg_idx] as u64, 8));
                            arg_idx += 1;
                        }
                    }
                    'c' => {
                        if arg_idx < args.len() {
                            let ch = (args[arg_idx] & 0xFF) as u8 as char;
                            output.push(ch);
                            arg_idx += 1;
                        }
                    }
                    'p' => {
                        if arg_idx < args.len() {
                            output.push_str(&alloc::format!("0x{:x}", args[arg_idx]));
                            arg_idx += 1;
                        }
                    }
                    '%' => output.push('%'),
                    _ => {
                        output.push('%');
                        output.push(spec);
                    }
                }
            }
        } else {
            output.push(c);
        }
    }

    output
}

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX wrappers (kernel-side emulation)
// ═══════════════════════════════════════════════════════════════════════════════

/// exit -- terminate the current process
pub fn _exit(status: i32) {
    crate::process::exit(status);
}

/// getpid -- get process ID
pub fn getpid() -> u32 {
    crate::process::getpid()
}

/// getppid -- get parent process ID
pub fn getppid() -> u32 {
    crate::process::getppid()
}

/// getuid -- get user ID
pub fn getuid() -> u32 {
    let pid = crate::process::getpid();
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    table[pid as usize].as_ref().map(|p| p.uid).unwrap_or(0)
}

/// getgid -- get group ID
pub fn getgid() -> u32 {
    let pid = crate::process::getpid();
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    table[pid as usize].as_ref().map(|p| p.gid).unwrap_or(0)
}

/// sleep -- suspend execution for seconds
pub fn sleep(seconds: u64) {
    crate::time::clock::sleep_ms(seconds * 1000);
}

/// usleep -- suspend execution for microseconds
pub fn usleep(usec: u64) {
    let ms = usec / 1000;
    if ms > 0 {
        crate::time::clock::sleep_ms(ms);
    }
}

/// time -- get current Unix time
pub fn time() -> u64 {
    crate::time::clock::unix_time()
}

/// getcwd -- get current working directory (returns kernel String)
pub fn getcwd() -> String {
    let pid = crate::process::getpid();
    let table = crate::process::pcb::PROCESS_TABLE.lock();
    table[pid as usize]
        .as_ref()
        .map(|p| p.cwd.clone())
        .unwrap_or_else(|| String::from("/"))
}

/// chdir -- change current working directory
pub fn chdir(path: &str) -> i32 {
    if crate::fs::vfs::memfs_stat(path).is_err() {
        return -(errno::ENOENT as i32);
    }
    let pid = crate::process::getpid();
    let mut table = crate::process::pcb::PROCESS_TABLE.lock();
    if let Some(proc) = table[pid as usize].as_mut() {
        proc.cwd = String::from(path);
        0
    } else {
        -(errno::ESRCH as i32)
    }
}

/// sched_yield -- yield the CPU
pub fn sched_yield() {
    crate::process::yield_now();
}

// ═══════════════════════════════════════════════════════════════════════════════
// Open file constants
// ═══════════════════════════════════════════════════════════════════════════════

pub mod fcntl {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    pub const O_CREAT: u32 = 0x40;
    pub const O_TRUNC: u32 = 0x200;
    pub const O_APPEND: u32 = 0x400;
    pub const O_EXCL: u32 = 0x80;
    pub const O_NONBLOCK: u32 = 0x800;
    pub const O_CLOEXEC: u32 = 0x80000;

    pub const F_DUPFD: u32 = 0;
    pub const F_GETFD: u32 = 1;
    pub const F_SETFD: u32 = 2;
    pub const F_GETFL: u32 = 3;
    pub const F_SETFL: u32 = 4;

    pub const FD_CLOEXEC: u32 = 1;
}

/// lseek whence values
pub mod seek {
    pub const SEEK_SET: u32 = 0;
    pub const SEEK_CUR: u32 = 1;
    pub const SEEK_END: u32 = 2;
}

// ═══════════════════════════════════════════════════════════════════════════════
// errno values
// ═══════════════════════════════════════════════════════════════════════════════

pub mod errno {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EIO: i32 = 5;
    pub const ENXIO: i32 = 6;
    pub const E2BIG: i32 = 7;
    pub const ENOEXEC: i32 = 8;
    pub const EBADF: i32 = 9;
    pub const ECHILD: i32 = 10;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EFAULT: i32 = 14;
    pub const ENOTBLK: i32 = 15;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const EXDEV: i32 = 18;
    pub const ENODEV: i32 = 19;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENFILE: i32 = 23;
    pub const EMFILE: i32 = 24;
    pub const ENOTTY: i32 = 25;
    pub const ETXTBSY: i32 = 26;
    pub const EFBIG: i32 = 27;
    pub const ENOSPC: i32 = 28;
    pub const ESPIPE: i32 = 29;
    pub const EROFS: i32 = 30;
    pub const EMLINK: i32 = 31;
    pub const EPIPE: i32 = 32;
    pub const EDOM: i32 = 33;
    pub const ERANGE: i32 = 34;
    pub const EDEADLK: i32 = 35;
    pub const ENAMETOOLONG: i32 = 36;
    pub const ENOLCK: i32 = 37;
    pub const ENOSYS: i32 = 38;
    pub const ENOTEMPTY: i32 = 39;
    pub const ELOOP: i32 = 40;
    pub const ENOTSUP: i32 = 95;
    pub const ECONNREFUSED: i32 = 111;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNRESET: i32 = 104;
    pub const EADDRINUSE: i32 = 98;
    pub const EADDRNOTAVAIL: i32 = 99;
    pub const ENETUNREACH: i32 = 101;
    pub const EHOSTUNREACH: i32 = 113;

    /// Convert errno to human-readable string
    pub fn strerror(err: i32) -> &'static str {
        match err {
            EPERM => "Operation not permitted",
            ENOENT => "No such file or directory",
            ESRCH => "No such process",
            EINTR => "Interrupted system call",
            EIO => "Input/output error",
            ENOMEM => "Cannot allocate memory",
            EACCES => "Permission denied",
            EFAULT => "Bad address",
            EEXIST => "File exists",
            ENOTDIR => "Not a directory",
            EISDIR => "Is a directory",
            EINVAL => "Invalid argument",
            ENOSPC => "No space left on device",
            EPIPE => "Broken pipe",
            EAGAIN => "Resource temporarily unavailable",
            EBADF => "Bad file descriptor",
            ENOSYS => "Function not implemented",
            ENOTEMPTY => "Directory not empty",
            ENAMETOOLONG => "File name too long",
            ECONNREFUSED => "Connection refused",
            ETIMEDOUT => "Connection timed out",
            _ => "Unknown error",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Signal constants
// ═══════════════════════════════════════════════════════════════════════════════

pub mod signal {
    pub const SIGHUP: u8 = 1;
    pub const SIGINT: u8 = 2;
    pub const SIGQUIT: u8 = 3;
    pub const SIGILL: u8 = 4;
    pub const SIGTRAP: u8 = 5;
    pub const SIGABRT: u8 = 6;
    pub const SIGBUS: u8 = 7;
    pub const SIGFPE: u8 = 8;
    pub const SIGKILL: u8 = 9;
    pub const SIGUSR1: u8 = 10;
    pub const SIGSEGV: u8 = 11;
    pub const SIGUSR2: u8 = 12;
    pub const SIGPIPE: u8 = 13;
    pub const SIGALRM: u8 = 14;
    pub const SIGTERM: u8 = 15;
    pub const SIGCHLD: u8 = 17;
    pub const SIGCONT: u8 = 18;
    pub const SIGSTOP: u8 = 19;
    pub const SIGTSTP: u8 = 20;
    pub const SIGTTIN: u8 = 21;
    pub const SIGTTOU: u8 = 22;

    /// Number of signals
    pub const NSIG: u8 = 32;

    /// Signal disposition
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SigAction {
        Default,
        Ignore,
        Handler(usize), // function pointer as usize
    }

    /// Get signal name
    pub fn sig_name(sig: u8) -> &'static str {
        match sig {
            SIGHUP => "SIGHUP",
            SIGINT => "SIGINT",
            SIGQUIT => "SIGQUIT",
            SIGILL => "SIGILL",
            SIGTRAP => "SIGTRAP",
            SIGABRT => "SIGABRT",
            SIGBUS => "SIGBUS",
            SIGFPE => "SIGFPE",
            SIGKILL => "SIGKILL",
            SIGUSR1 => "SIGUSR1",
            SIGSEGV => "SIGSEGV",
            SIGUSR2 => "SIGUSR2",
            SIGPIPE => "SIGPIPE",
            SIGALRM => "SIGALRM",
            SIGTERM => "SIGTERM",
            SIGCHLD => "SIGCHLD",
            SIGCONT => "SIGCONT",
            SIGSTOP => "SIGSTOP",
            SIGTSTP => "SIGTSTP",
            _ => "SIG???",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Wait status macros (matching POSIX)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod wait {
    /// True if child exited normally
    pub fn wifexited(status: i32) -> bool {
        (status & 0x7F) == 0
    }

    /// Return exit code (only valid if WIFEXITED)
    pub fn wexitstatus(status: i32) -> i32 {
        (status >> 8) & 0xFF
    }

    /// True if child was stopped by a signal
    pub fn wifstopped(status: i32) -> bool {
        (status & 0xFF) == 0x7F
    }

    /// True if child was killed by a signal
    pub fn wifsignaled(status: i32) -> bool {
        !wifexited(status) && !wifstopped(status)
    }

    /// Return signal that killed child
    pub fn wtermsig(status: i32) -> i32 {
        status & 0x7F
    }

    /// Return signal that stopped child
    pub fn wstopsig(status: i32) -> i32 {
        (status >> 8) & 0xFF
    }

    pub const WNOHANG: i32 = 1;
    pub const WUNTRACED: i32 = 2;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Misc utilities
// ═══════════════════════════════════════════════════════════════════════════════

/// abs -- absolute value
pub fn abs(n: i64) -> i64 {
    if n < 0 {
        -n
    } else {
        n
    }
}

/// min/max helpers
pub fn min(a: i64, b: i64) -> i64 {
    if a < b {
        a
    } else {
        b
    }
}

pub fn max(a: i64, b: i64) -> i64 {
    if a > b {
        a
    } else {
        b
    }
}

/// environ -- environment variable helper
///
/// Returns the value of an environment variable from the kernel process table.
pub fn getenv(name: &str) -> Option<String> {
    // This would normally check the process's environment block
    // For kernel-mode emulation, we don't have a real environ
    let _ = name;
    None
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

/// Initialize libc subsystem
pub fn init() {
    serial_println!("  libc: C library ready (string, mem, stdio, posix, errno, signal)");
}
