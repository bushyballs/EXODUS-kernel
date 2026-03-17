use super::vfs::{DirEntry, FileOps, FileSystem, FileType, FsError, Inode};
/// Device filesystem (devfs) for Genesis
///
/// Provides virtual /dev files:
///   /dev/null    — reads return EOF; writes are discarded
///   /dev/zero    — reads return zero bytes; writes are discarded
///   /dev/full    — reads return zeros; writes return ENOSPC
///   /dev/random  — reads return cryptographic random bytes
///   /dev/urandom — alias for /dev/random (same quality on this kernel)
///   /dev/mem     — physical memory window (limited, zero-fills unmapped ranges)
///   /dev/console — reads from keyboard; writes to VGA
///   /dev/serial  — reads/writes COM1 serial port
///   /dev/tty     — controlling terminal (keyboard in, VGA out)
///   /dev/tty0    — alias for /dev/tty
///   /dev/stdin   — alias for /dev/tty (fd 0 equivalent)
///   /dev/stdout  — alias for /dev/console (fd 1 equivalent)
///   /dev/stderr  — alias for /dev/console (fd 2 equivalent)
///   /dev/sda     — first ATA disk (block device)
///
/// Major/minor numbers follow Linux convention (mem=1, tty=4, misc=10).
///
/// Inspired by: Linux devtmpfs, Plan 9 /dev, FreeBSD devfs.
/// All code is original.
use crate::kprint;
use crate::serial_print;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// Major / minor number constants (Linux-compatible)
// ============================================================================

/// Memory character devices (null, zero, full, random, urandom, mem, kmem)
pub const MAJOR_MEM: u32 = 1;
/// TTY / console devices
pub const MAJOR_TTY: u32 = 4;
/// Miscellaneous devices (loop, etc.)
pub const MAJOR_MISC: u32 = 10;

/// /dev/kmem  — kernel virtual memory (major 1, minor 2)
pub const MINOR_MEM_KMEM: u32 = 2;
/// /dev/null  — major 1, minor 3
pub const MINOR_MEM_NULL: u32 = 3;
/// /dev/zero  — major 1, minor 5
pub const MINOR_MEM_ZERO: u32 = 5;
/// /dev/full  — major 1, minor 7
pub const MINOR_MEM_FULL: u32 = 7;
/// /dev/random — major 1, minor 8
pub const MINOR_MEM_RANDOM: u32 = 8;
/// /dev/urandom — major 1, minor 9
pub const MINOR_MEM_URANDOM: u32 = 9;
/// /dev/mem   — major 1, minor 1
pub const MINOR_MEM_MEM: u32 = 1;

/// /dev/tty — major 4, minor 0
pub const MINOR_TTY_TTY: u32 = 0;
/// /dev/tty0 — major 4, minor 0  (virtual console 0)
pub const MINOR_TTY_TTY0: u32 = 0;

// ============================================================================
// Low-level device dispatch — major/minor → read/write
// Called from VFS or syscall layer when opening a device inode.
// ============================================================================

/// Read from a character device identified by (major, minor).
/// Returns number of bytes placed in `buf`, or a negative errno on error.
pub fn dev_read(major: u32, minor: u32, buf: &mut [u8]) -> isize {
    match (major, minor) {
        (MAJOR_MEM, MINOR_MEM_NULL) => 0, // EOF
        (MAJOR_MEM, MINOR_MEM_ZERO) => dev_zero_fill(buf) as isize,
        (MAJOR_MEM, MINOR_MEM_FULL) => dev_zero_fill(buf) as isize, // full reads zeros
        (MAJOR_MEM, MINOR_MEM_RANDOM) => dev_random_fill(buf) as isize,
        (MAJOR_MEM, MINOR_MEM_URANDOM) => dev_random_fill(buf) as isize,
        (MAJOR_MEM, MINOR_MEM_MEM) => dev_mem_read(0, buf) as isize,
        (MAJOR_MEM, MINOR_MEM_KMEM) => dev_mem_read(0, buf) as isize,
        _ => -19, // -ENODEV
    }
}

/// Write to a character device identified by (major, minor).
/// Returns number of bytes consumed, or a negative errno on error.
pub fn dev_write(major: u32, minor: u32, data: &[u8]) -> isize {
    match (major, minor) {
        (MAJOR_MEM, MINOR_MEM_NULL) => data.len() as isize, // discard
        (MAJOR_MEM, MINOR_MEM_ZERO) => data.len() as isize, // discard
        (MAJOR_MEM, MINOR_MEM_FULL) => -28,                 // -ENOSPC
        (MAJOR_MEM, MINOR_MEM_RANDOM) => data.len() as isize, // add entropy (accepted)
        (MAJOR_MEM, MINOR_MEM_URANDOM) => data.len() as isize,
        _ => -19, // -ENODEV
    }
}

// ============================================================================
// Internal helpers — no alloc, no float, saturating arithmetic only
// ============================================================================

/// Fill `buf` with zero bytes. Returns number of bytes written.
#[inline]
fn dev_zero_fill(buf: &mut [u8]) -> usize {
    for b in buf.iter_mut() {
        *b = 0;
    }
    buf.len()
}

/// Fill `buf` with cryptographic random bytes. Returns number of bytes written.
#[inline]
fn dev_random_fill(buf: &mut [u8]) -> usize {
    crate::crypto::random::fill_bytes(buf);
    buf.len()
}

/// Read physical memory at `phys_addr` into `buf`.
/// Unmapped / out-of-range bytes are zeroed for safety.
/// No float arithmetic; saturating address arithmetic throughout.
fn dev_mem_read(phys_addr: u64, buf: &mut [u8]) -> usize {
    // Physical memory is identity-mapped at PHYS_OFFSET in this kernel.
    // We only allow reads below 4 GiB to stay within the low-memory window.
    const PHYS_OFFSET: u64 = 0;
    const MAX_PHYS: u64 = 0x1_0000_0000; // 4 GiB cap for safety

    let mut written = 0usize;
    for (i, slot) in buf.iter_mut().enumerate() {
        let addr = phys_addr.saturating_add(i as u64);
        if addr < MAX_PHYS {
            let virt = PHYS_OFFSET.saturating_add(addr) as *const u8;
            // SAFETY: address is within identity-mapped low memory.
            *slot = unsafe { core::ptr::read_volatile(virt) };
        } else {
            *slot = 0; // beyond window — return zero
        }
        written = written.saturating_add(1);
    }
    written
}

// ============================================================================
// devfs filesystem implementation
// ============================================================================

/// devfs filesystem object (mounted at /dev)
pub struct DevFs;

impl FileSystem for DevFs {
    fn name(&self) -> &str {
        "devfs"
    }

    fn root(&self) -> Result<Inode, FsError> {
        Ok(Inode {
            ino: 1,
            file_type: FileType::Directory,
            size: 0,
            mode: 0o755,
            uid: 0,
            gid: 0,
            nlink: 2,
            ops: Box::new(DevFsRoot),
            rdev: make_rdev(0, 0),
            blocks: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            crtime: 0,
        })
    }
}

/// Encode a (major, minor) pair into a single u64 rdev value.
#[inline]
fn make_rdev(major: u32, minor: u32) -> u64 {
    ((major as u64) << 32) | (minor as u64)
}

// ============================================================================
// Root directory of /dev
// ============================================================================

#[derive(Debug)]
struct DevFsRoot;

impl FileOps for DevFsRoot {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn size(&self) -> u64 {
        0
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, FsError> {
        Ok(alloc::vec![
            // Memory-class character devices
            DirEntry {
                name: String::from("null"),
                ino: 2,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("zero"),
                ino: 3,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("full"),
                ino: 4,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("random"),
                ino: 5,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("urandom"),
                ino: 6,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("mem"),
                ino: 7,
                file_type: FileType::CharDevice
            },
            // Terminal devices
            DirEntry {
                name: String::from("console"),
                ino: 8,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("serial"),
                ino: 9,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("tty"),
                ino: 10,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("tty0"),
                ino: 11,
                file_type: FileType::CharDevice
            },
            // Standard I/O aliases (route to tty / console)
            DirEntry {
                name: String::from("stdin"),
                ino: 12,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("stdout"),
                ino: 13,
                file_type: FileType::CharDevice
            },
            DirEntry {
                name: String::from("stderr"),
                ino: 14,
                file_type: FileType::CharDevice
            },
            // Block devices
            DirEntry {
                name: String::from("sda"),
                ino: 20,
                file_type: FileType::BlockDevice
            },
        ])
    }

    fn lookup(&self, name: &str) -> Result<Inode, FsError> {
        match name {
            // --- /dev/null ---
            "null" => Ok(Inode {
                ino: 2,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevNull),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_NULL),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/zero ---
            "zero" => Ok(Inode {
                ino: 3,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevZero),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_ZERO),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/full ---
            "full" => Ok(Inode {
                ino: 4,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevFull),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_FULL),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/random ---
            "random" => Ok(Inode {
                ino: 5,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevRandom),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_RANDOM),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/urandom ---
            "urandom" => Ok(Inode {
                ino: 6,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevUrandom),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_URANDOM),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/mem ---
            "mem" => Ok(Inode {
                ino: 7,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o640,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevMem),
                rdev: make_rdev(MAJOR_MEM, MINOR_MEM_MEM),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/console ---
            "console" => Ok(Inode {
                ino: 8,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o620,
                uid: 0,
                gid: 5,
                nlink: 1,
                ops: Box::new(DevConsole),
                rdev: make_rdev(MAJOR_TTY, 1),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/serial ---
            "serial" => Ok(Inode {
                ino: 9,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o620,
                uid: 0,
                gid: 5,
                nlink: 1,
                ops: Box::new(DevSerial),
                rdev: make_rdev(4, 64), // ttyS0 = major 4, minor 64
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/tty and /dev/tty0 ---
            "tty" => Ok(Inode {
                ino: 10,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 5,
                nlink: 1,
                ops: Box::new(DevTty),
                rdev: make_rdev(MAJOR_TTY, MINOR_TTY_TTY),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            "tty0" => Ok(Inode {
                ino: 11,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o620,
                uid: 0,
                gid: 5,
                nlink: 1,
                ops: Box::new(DevTty),
                rdev: make_rdev(MAJOR_TTY, MINOR_TTY_TTY0),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/stdin — reads from active TTY keyboard ---
            "stdin" => Ok(Inode {
                ino: 12,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevStdin),
                rdev: make_rdev(MAJOR_TTY, MINOR_TTY_TTY),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/stdout — writes to active TTY/VGA ---
            "stdout" => Ok(Inode {
                ino: 13,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevStdout),
                rdev: make_rdev(MAJOR_TTY, MINOR_TTY_TTY),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/stderr — writes to active TTY/VGA (same as stdout) ---
            "stderr" => Ok(Inode {
                ino: 14,
                file_type: FileType::CharDevice,
                size: 0,
                mode: 0o666,
                uid: 0,
                gid: 0,
                nlink: 1,
                ops: Box::new(DevStderr),
                rdev: make_rdev(MAJOR_TTY, MINOR_TTY_TTY),
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            // --- /dev/sda — first ATA disk (block device) ---
            "sda" => Ok(Inode {
                ino: 20,
                file_type: FileType::BlockDevice,
                size: 0,
                mode: 0o660,
                uid: 0,
                gid: 6,
                nlink: 1,
                ops: Box::new(DevSda),
                rdev: make_rdev(8, 0), // Linux: major 8 = sd*
                blocks: 0,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
            }),
            _ => Err(FsError::NotFound),
        }
    }
}

// ============================================================================
// Device implementations
// ============================================================================

// --- /dev/null ---------------------------------------------------------------

/// /dev/null — reads return EOF immediately; writes are silently discarded.
#[derive(Debug)]
struct DevNull;

impl FileOps for DevNull {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(0) // EOF
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        Ok(buf.len()) // discard
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/zero ---------------------------------------------------------------

/// /dev/zero — reads fill the buffer with zero bytes; writes are discarded.
#[derive(Debug)]
struct DevZero;

impl FileOps for DevZero {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(dev_zero_fill(buf))
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/full ---------------------------------------------------------------

/// /dev/full — reads fill the buffer with zeros (like /dev/zero),
/// but writes always fail with ENOSPC (-28).
#[derive(Debug)]
struct DevFull;

impl FileOps for DevFull {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(dev_zero_fill(buf))
    }
    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::NoSpace) // ENOSPC
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/random -------------------------------------------------------------

/// /dev/random — cryptographically secure random bytes.
/// On this kernel /dev/random and /dev/urandom use the same CSPRNG
/// (ChaCha20 seeded from RDRAND/RDSEED + TSC jitter).
/// Writes add entropy to the pool (accepted and discarded here since
/// the CSPRNG rekeys automatically).
#[derive(Debug)]
struct DevRandom;

impl FileOps for DevRandom {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(dev_random_fill(buf))
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        // Treat incoming data as entropy input — accepted silently.
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/urandom ------------------------------------------------------------

/// /dev/urandom — same CSPRNG as /dev/random; never blocks.
/// Distinct inode so that stat(2) returns the correct minor number (9).
#[derive(Debug)]
struct DevUrandom;

impl FileOps for DevUrandom {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(dev_random_fill(buf))
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/mem ----------------------------------------------------------------

/// /dev/mem — physical memory character device.
///
/// Reads return raw bytes from the identity-mapped physical address space.
/// The `offset` parameter is the physical address.
/// Addresses beyond the 4 GiB safety cap are zeroed.
/// Writes are rejected (read-only) to prevent accidental corruption.
#[derive(Debug)]
struct DevMem;

impl FileOps for DevMem {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(dev_mem_read(offset, buf))
    }
    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied) // /dev/mem is read-only in this kernel
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/console ------------------------------------------------------------

/// /dev/console — reads drain the keyboard ring buffer; writes go to the VGA.
#[derive(Debug)]
struct DevConsole;

impl FileOps for DevConsole {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut count = 0usize;
        while count < buf.len() {
            if let Some(event) = crate::drivers::keyboard::pop_key() {
                if event.pressed && event.character != '\0' {
                    buf[count] = event.character as u8;
                    count = count.saturating_add(1);
                    if event.character == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        Ok(count)
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        for &byte in buf {
            if byte == b'\n' || (byte >= 0x20 && byte <= 0x7e) {
                kprint!("{}", byte as char);
            }
        }
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/serial -------------------------------------------------------------

/// /dev/serial — reads/writes COM1 (0x3F8) serial port.
#[derive(Debug)]
struct DevSerial;

impl FileOps for DevSerial {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        const COM1_BASE: u16 = 0x3F8;
        const COM1_LSR: u16 = COM1_BASE + 5;
        const LSR_DATA_READY: u8 = 0x01;

        let mut count = 0usize;
        while count < buf.len() {
            let lsr = crate::io::inb(COM1_LSR);
            if lsr & LSR_DATA_READY == 0 {
                break;
            }
            let byte = crate::io::inb(COM1_BASE);
            buf[count] = byte;
            count = count.saturating_add(1);
            if byte == b'\n' {
                break;
            }
        }
        Ok(count)
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        for &byte in buf {
            serial_print!("{}", byte as char);
        }
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/tty ----------------------------------------------------------------

/// /dev/tty — controlling terminal of the current process.
/// Reads drain the keyboard ring buffer; writes go to the VGA framebuffer.
#[derive(Debug)]
struct DevTty;

impl FileOps for DevTty {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let mut count = 0usize;
        while count < buf.len() {
            if let Some(event) = crate::drivers::keyboard::pop_key() {
                if event.pressed && event.character != '\0' {
                    buf[count] = event.character as u8;
                    count = count.saturating_add(1);
                    if event.character == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        Ok(count)
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        for &byte in buf {
            if byte == b'\n' || (byte >= 0x20 && byte <= 0x7e) {
                kprint!("{}", byte as char);
            }
        }
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/stdin --------------------------------------------------------------

/// /dev/stdin — standard input, backed by the active TTY keyboard.
/// Reads pull from the keyboard ring buffer; writes are rejected.
#[derive(Debug)]
struct DevStdin;

impl FileOps for DevStdin {
    fn read(&self, _offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        // Read from keyboard ring buffer (same source as /dev/tty).
        let mut count = 0usize;
        while count < buf.len() {
            if let Some(event) = crate::drivers::keyboard::pop_key() {
                if event.pressed && event.character != '\0' {
                    buf[count] = event.character as u8;
                    count = count.saturating_add(1);
                    if event.character == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
        Ok(count)
    }
    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied) // stdin is read-only
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/stdout -------------------------------------------------------------

/// /dev/stdout — standard output, backed by the VGA framebuffer / console.
/// Reads are rejected; writes display bytes on the active console.
#[derive(Debug)]
struct DevStdout;

impl FileOps for DevStdout {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied) // stdout is write-only
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        for &byte in buf {
            if byte == b'\n' || (byte >= 0x20 && byte <= 0x7e) {
                kprint!("{}", byte as char);
            }
        }
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/stderr -------------------------------------------------------------

/// /dev/stderr — standard error, backed by the same console as /dev/stdout.
/// Reads are rejected; writes display bytes on the active console.
#[derive(Debug)]
struct DevStderr;

impl FileOps for DevStderr {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::PermissionDenied) // stderr is write-only
    }
    fn write(&self, _offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        // Identical to stdout — both go to the kernel console.
        for &byte in buf {
            if byte == b'\n' || (byte >= 0x20 && byte <= 0x7e) {
                kprint!("{}", byte as char);
            }
        }
        Ok(buf.len())
    }
    fn size(&self) -> u64 {
        0
    }
}

// --- /dev/sda ----------------------------------------------------------------

/// /dev/sda — first ATA disk (block device).
/// Offset is a byte offset into the disk; reads/writes are sector-aligned
/// via a read-modify-write cycle for partial sectors.
#[derive(Debug)]
struct DevSda;

impl FileOps for DevSda {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let sector = offset / 512;
        let sector_offset = (offset % 512) as usize;
        let mut sector_buf = [0u8; 512];
        crate::drivers::ata::read_sectors(0, sector, 1, &mut sector_buf)
            .map_err(|_| FsError::IoError)?;
        // Copy only what fits, starting at the intra-sector offset.
        let available = 512usize.saturating_sub(sector_offset);
        let copy_len = buf.len().min(available);
        buf[..copy_len].copy_from_slice(&sector_buf[sector_offset..sector_offset + copy_len]);
        Ok(copy_len)
    }
    fn write(&self, offset: u64, buf: &[u8]) -> Result<usize, FsError> {
        let sector = offset / 512;
        let sector_offset = (offset % 512) as usize;
        let mut sector_buf = [0u8; 512];
        // Read-modify-write for partial-sector writes.
        if sector_offset != 0 || buf.len() < 512 {
            let _ = crate::drivers::ata::read_sectors(0, sector, 1, &mut sector_buf);
        }
        let available = 512usize.saturating_sub(sector_offset);
        let copy_len = buf.len().min(available);
        sector_buf[sector_offset..sector_offset + copy_len].copy_from_slice(&buf[..copy_len]);
        crate::drivers::ata::write_sectors(0, sector, 1, &sector_buf)
            .map_err(|_| FsError::IoError)?;
        Ok(copy_len)
    }
    fn size(&self) -> u64 {
        let drives = crate::drivers::ata::drives();
        if let Some(d) = drives.first() {
            d.sectors * 512
        } else {
            0
        }
    }
}

// ============================================================================
// NativeHoagsFs — wraps a mounted HoagsFileSystem on ATA
// ============================================================================

/// HoagsFS native implementation — constructs an ArcHoagsFs from the first ATA drive.
pub struct NativeHoagsFs;

impl FileSystem for NativeHoagsFs {
    fn name(&self) -> &str {
        "hoagsfs"
    }

    fn root(&self) -> Result<Inode, FsError> {
        use super::hoagsfs::{ArcHoagsFs, HoagsFileSystem};
        use crate::fs::AtaBlockDevice;

        if crate::drivers::ata::drive_count() == 0 {
            return Err(FsError::IoError);
        }
        let device = alloc::boxed::Box::new(AtaBlockDevice::new(0));
        let fs = HoagsFileSystem::mount(device)?;
        let arc_fs = ArcHoagsFs::new(fs);
        arc_fs.root()
    }
}

// ============================================================================
// Init
// ============================================================================

/// Mount devfs at /dev and log the registered device nodes.
pub fn init() {
    super::vfs::mount("/dev", Box::new(DevFs));
    crate::serial_println!("  [devfs] /dev mounted: null zero full random urandom mem console serial tty stdin stdout stderr sda");
}
