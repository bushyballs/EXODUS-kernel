/// ioctl command dispatch -- major device routing
///
/// Part of the AIOS compatibility layer.
///
/// Provides a central ioctl dispatcher that routes ioctl commands to the
/// appropriate device-class handler based on the ioctl number encoding.
/// Linux encodes device type, direction, and size in the ioctl number;
/// this module decodes those fields and dispatches accordingly.
///
/// Design:
///   - ioctl numbers are decoded: type (8 bits), nr (8 bits), size (14 bits),
///     direction (2 bits).
///   - Handlers are registered per device type (major number category).
///   - A fallback handler returns ENOTTY for unrecognized commands.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux ioctl (include/uapi/asm-generic/ioctl.h). All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// ioctl number encoding/decoding (Linux convention)
// ---------------------------------------------------------------------------

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_DIRBITS: u32 = 2;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

pub const IOC_NONE: u32 = 0;
pub const IOC_WRITE: u32 = 1;
pub const IOC_READ: u32 = 2;

/// Decode the direction field from an ioctl number.
pub fn ioc_dir(cmd: u32) -> u32 {
    (cmd >> IOC_DIRSHIFT) & ((1 << IOC_DIRBITS) - 1)
}

/// Decode the type (magic) field from an ioctl number.
pub fn ioc_type(cmd: u32) -> u32 {
    (cmd >> IOC_TYPESHIFT) & ((1 << IOC_TYPEBITS) - 1)
}

/// Decode the command number from an ioctl number.
pub fn ioc_nr(cmd: u32) -> u32 {
    (cmd >> IOC_NRSHIFT) & ((1 << IOC_NRBITS) - 1)
}

/// Decode the argument size from an ioctl number.
pub fn ioc_size(cmd: u32) -> u32 {
    (cmd >> IOC_SIZESHIFT) & ((1 << IOC_SIZEBITS) - 1)
}

/// Encode an ioctl number.
pub fn ioc(dir: u32, ty: u32, nr: u32, size: u32) -> u32 {
    (dir << IOC_DIRSHIFT) | (ty << IOC_TYPESHIFT) | (nr << IOC_NRSHIFT) | (size << IOC_SIZESHIFT)
}

/// Convenience: _IO (no data transfer)
pub fn io(ty: u32, nr: u32) -> u32 {
    ioc(IOC_NONE, ty, nr, 0)
}

/// Convenience: _IOR (read from device)
pub fn ior(ty: u32, nr: u32, size: u32) -> u32 {
    ioc(IOC_READ, ty, nr, size)
}

/// Convenience: _IOW (write to device)
pub fn iow(ty: u32, nr: u32, size: u32) -> u32 {
    ioc(IOC_WRITE, ty, nr, size)
}

/// Convenience: _IOWR (read/write)
pub fn iowr(ty: u32, nr: u32, size: u32) -> u32 {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size)
}

// ---------------------------------------------------------------------------
// Well-known device types
// ---------------------------------------------------------------------------

pub const IOCTL_TYPE_TTY: u32 = b'T' as u32;
pub const IOCTL_TYPE_BLOCK: u32 = 0x12;
pub const IOCTL_TYPE_FS: u32 = b'f' as u32;
pub const IOCTL_TYPE_NET: u32 = 0x89;
pub const IOCTL_TYPE_INPUT: u32 = b'E' as u32;
pub const IOCTL_TYPE_SND: u32 = b'U' as u32;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Handler function: (fd, cmd, arg) -> result
type IoctlHandler = fn(usize, u32, usize) -> isize;

/// A registered handler for a device type.
struct HandlerEntry {
    device_type: u32,
    handler: IoctlHandler,
    name: &'static str,
}

/// Inner dispatcher state.
struct Inner {
    handlers: Vec<HandlerEntry>,
    dispatch_count: u64,
    fallback_count: u64,
}

// ---------------------------------------------------------------------------
// Default handlers
// ---------------------------------------------------------------------------

fn tty_ioctl_handler(fd: usize, cmd: u32, arg: usize) -> isize {
    let _ = (fd, cmd, arg);
    // Placeholder: real implementation would handle TIOCGWINSZ, TCGETS, etc.
    -25 // ENOTTY
}

fn block_ioctl_handler(fd: usize, cmd: u32, arg: usize) -> isize {
    let _ = (fd, cmd, arg);
    -25
}

fn fs_ioctl_handler(fd: usize, cmd: u32, arg: usize) -> isize {
    let _ = (fd, cmd, arg);
    -25
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            handlers: Vec::new(),
            dispatch_count: 0,
            fallback_count: 0,
        }
    }

    fn register(&mut self, device_type: u32, handler: IoctlHandler, name: &'static str) {
        // Replace if already registered
        for entry in self.handlers.iter_mut() {
            if entry.device_type == device_type {
                entry.handler = handler;
                entry.name = name;
                return;
            }
        }
        self.handlers.push(HandlerEntry {
            device_type,
            handler,
            name,
        });
    }

    fn dispatch(&mut self, fd: usize, cmd: u32, arg: usize) -> isize {
        self.dispatch_count = self.dispatch_count.saturating_add(1);
        let device_type = ioc_type(cmd);

        for entry in self.handlers.iter() {
            if entry.device_type == device_type {
                return (entry.handler)(fd, cmd, arg);
            }
        }

        // No handler found
        self.fallback_count = self.fallback_count.saturating_add(1);
        -25 // ENOTTY
    }

    fn populate_defaults(&mut self) {
        self.register(IOCTL_TYPE_TTY, tty_ioctl_handler, "tty");
        self.register(IOCTL_TYPE_BLOCK, block_ioctl_handler, "block");
        self.register(IOCTL_TYPE_FS, fs_ioctl_handler, "fs");
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static IOCTL_DISPATCH: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Dispatch an ioctl command to the appropriate handler.
pub fn dispatch(fd: usize, cmd: u32, arg: usize) -> isize {
    let mut guard = IOCTL_DISPATCH.lock();
    match guard.as_mut() {
        Some(inner) => inner.dispatch(fd, cmd, arg),
        None => -25,
    }
}

/// Register a handler for a device type.
pub fn register(device_type: u32, handler: IoctlHandler, name: &'static str) {
    let mut guard = IOCTL_DISPATCH.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register(device_type, handler, name);
    }
}

/// Return (dispatch_count, fallback_count).
pub fn stats() -> (u64, u64) {
    let guard = IOCTL_DISPATCH.lock();
    guard.as_ref().map_or((0, 0), |inner| (inner.dispatch_count, inner.fallback_count))
}

/// Initialize the ioctl dispatch subsystem.
pub fn init() {
    let mut guard = IOCTL_DISPATCH.lock();
    let mut inner = Inner::new();
    inner.populate_defaults();
    let count = inner.handlers.len();
    *guard = Some(inner);
    serial_println!("    ioctl: {} device type handlers registered", count);
}
