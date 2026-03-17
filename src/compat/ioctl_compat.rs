/// ioctl number translation layer -- Linux to AIOS mapping
///
/// Part of the AIOS compatibility layer.
///
/// Translates Linux ioctl numbers to AIOS-native device operations.
/// Some Linux ioctl numbers use non-standard encoding or have different
/// semantics; this module handles the mapping per device class.
///
/// Design:
///   - A translation table maps (device_class, linux_ioctl_nr) pairs to
///     AIOS-native ioctl numbers and optional argument fixup callbacks.
///   - Device classes: terminal, block, filesystem, network, input, sound.
///   - For simple ioctls the translation is a direct number remapping.
///   - For complex ioctls the argument structure may need to be converted.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: Linux compat_ioctl (fs/compat_ioctl.c). All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Device class identifiers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum DeviceClass {
    Terminal,
    Block,
    FileSystem,
    Network,
    Input,
    Sound,
    Generic,
}

// ---------------------------------------------------------------------------
// Well-known Linux ioctl numbers for terminals
// ---------------------------------------------------------------------------

pub const TCGETS: u64 = 0x5401;
pub const TCSETS: u64 = 0x5402;
pub const TCSETSW: u64 = 0x5403;
pub const TCSETSF: u64 = 0x5404;
pub const TIOCGWINSZ: u64 = 0x5413;
pub const TIOCSWINSZ: u64 = 0x5414;
pub const TIOCGPGRP: u64 = 0x540F;
pub const TIOCSPGRP: u64 = 0x5410;
pub const FIONREAD: u64 = 0x541B;
pub const FIONBIO: u64 = 0x5421;

// Well-known Linux ioctl numbers for block devices
pub const BLKGETSIZE: u64 = 0x1260;
pub const BLKGETSIZE64: u64 = 0x80081272;
pub const BLKFLSBUF: u64 = 0x1261;
pub const BLKROSET: u64 = 0x125D;
pub const BLKROGET: u64 = 0x125E;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Argument fixup: how to transform the Linux argument before passing to AIOS.
#[derive(Clone, Copy, PartialEq)]
pub enum ArgFixup {
    /// Pass argument unchanged.
    None,
    /// Argument is a pointer to a struct that needs size conversion.
    StructResize { linux_size: usize, aios_size: usize },
    /// Argument needs a constant added.
    AddOffset(i64),
    /// Argument is unused (set to 0).
    Zero,
}

/// A single translation entry.
#[derive(Clone, Copy)]
struct TranslationEntry {
    device_class: DeviceClass,
    linux_nr: u64,
    aios_nr: u64,
    fixup: ArgFixup,
}

/// Inner state.
struct Inner {
    entries: Vec<TranslationEntry>,
    translate_count: u64,
    miss_count: u64,
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            entries: Vec::new(),
            translate_count: 0,
            miss_count: 0,
        }
    }

    fn register(&mut self, class: DeviceClass, linux_nr: u64, aios_nr: u64, fixup: ArgFixup) {
        self.entries.push(TranslationEntry {
            device_class: class,
            linux_nr,
            aios_nr,
            fixup,
        });
    }

    fn translate(&mut self, linux_nr: u64) -> Option<(u64, ArgFixup)> {
        self.translate_count = self.translate_count.saturating_add(1);
        for entry in self.entries.iter() {
            if entry.linux_nr == linux_nr {
                return Some((entry.aios_nr, entry.fixup));
            }
        }
        self.miss_count = self.miss_count.saturating_add(1);
        None
    }

    fn translate_for_class(&mut self, class: DeviceClass, linux_nr: u64) -> Option<(u64, ArgFixup)> {
        self.translate_count = self.translate_count.saturating_add(1);
        for entry in self.entries.iter() {
            if entry.device_class == class && entry.linux_nr == linux_nr {
                return Some((entry.aios_nr, entry.fixup));
            }
        }
        self.miss_count = self.miss_count.saturating_add(1);
        None
    }

    fn populate_defaults(&mut self) {
        // Terminal ioctls (most map 1:1 for Linux compat)
        self.register(DeviceClass::Terminal, TCGETS, TCGETS, ArgFixup::None);
        self.register(DeviceClass::Terminal, TCSETS, TCSETS, ArgFixup::None);
        self.register(DeviceClass::Terminal, TCSETSW, TCSETSW, ArgFixup::None);
        self.register(DeviceClass::Terminal, TCSETSF, TCSETSF, ArgFixup::None);
        self.register(DeviceClass::Terminal, TIOCGWINSZ, TIOCGWINSZ, ArgFixup::None);
        self.register(DeviceClass::Terminal, TIOCSWINSZ, TIOCSWINSZ, ArgFixup::None);
        self.register(DeviceClass::Terminal, TIOCGPGRP, TIOCGPGRP, ArgFixup::None);
        self.register(DeviceClass::Terminal, TIOCSPGRP, TIOCSPGRP, ArgFixup::None);
        self.register(DeviceClass::Terminal, FIONREAD, FIONREAD, ArgFixup::None);
        self.register(DeviceClass::Terminal, FIONBIO, FIONBIO, ArgFixup::None);

        // Block device ioctls
        self.register(DeviceClass::Block, BLKGETSIZE, BLKGETSIZE, ArgFixup::None);
        self.register(DeviceClass::Block, BLKGETSIZE64, BLKGETSIZE64, ArgFixup::None);
        self.register(DeviceClass::Block, BLKFLSBUF, BLKFLSBUF, ArgFixup::Zero);
        self.register(DeviceClass::Block, BLKROSET, BLKROSET, ArgFixup::None);
        self.register(DeviceClass::Block, BLKROGET, BLKROGET, ArgFixup::None);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static IOCTL_COMPAT: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Translate a Linux ioctl number to an AIOS ioctl number.
pub fn translate(linux_nr: u64) -> Option<(u64, ArgFixup)> {
    let mut guard = IOCTL_COMPAT.lock();
    guard.as_mut().and_then(|inner| inner.translate(linux_nr))
}

/// Translate with a specific device class filter.
pub fn translate_for_class(class: DeviceClass, linux_nr: u64) -> Option<(u64, ArgFixup)> {
    let mut guard = IOCTL_COMPAT.lock();
    guard
        .as_mut()
        .and_then(|inner| inner.translate_for_class(class, linux_nr))
}

/// Full dispatch: translate the ioctl number and call through to the
/// main ioctl dispatcher with the AIOS-native number.
pub fn dispatch(fd: i32, linux_request: u64, arg: u64) -> i64 {
    let translation = {
        let mut guard = IOCTL_COMPAT.lock();
        guard.as_mut().and_then(|inner| inner.translate(linux_request))
    };

    match translation {
        Some((aios_nr, fixup)) => {
            let adjusted_arg = match fixup {
                ArgFixup::None => arg,
                ArgFixup::Zero => 0,
                ArgFixup::AddOffset(off) => (arg as i64 + off) as u64,
                ArgFixup::StructResize { .. } => arg, // Caller handles struct copy
            };
            super::ioctl::dispatch(fd as usize, aios_nr as u32, adjusted_arg as usize) as i64
        }
        None => -25, // ENOTTY
    }
}

/// Register a custom translation entry.
pub fn register(class: DeviceClass, linux_nr: u64, aios_nr: u64, fixup: ArgFixup) {
    let mut guard = IOCTL_COMPAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.register(class, linux_nr, aios_nr, fixup);
    }
}

/// Return (translate_count, miss_count).
pub fn stats() -> (u64, u64) {
    let guard = IOCTL_COMPAT.lock();
    guard.as_ref().map_or((0, 0), |inner| (inner.translate_count, inner.miss_count))
}

/// Initialize the ioctl compatibility translation layer.
pub fn init() {
    let mut guard = IOCTL_COMPAT.lock();
    let mut inner = Inner::new();
    inner.populate_defaults();
    let count = inner.entries.len();
    *guard = Some(inner);
    serial_println!("    ioctl_compat: {} ioctl translations registered", count);
}
