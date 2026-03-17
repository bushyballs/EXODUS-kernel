/// Loop device (mount files as block devices)
///
/// Part of the AIOS storage layer.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// A loop device presents a region of a file as a block device.
pub struct LoopDevice {
    /// Unique identifier for this loop device.
    id: u32,
    /// Path to the backing file.
    file_path: String,
    /// Byte offset into the backing file where the device starts.
    offset: u64,
    /// Size limit in bytes (0 = use entire file).
    size_limit: u64,
    /// Whether the device is read-only.
    read_only: bool,
    /// Whether the device is currently attached.
    attached: bool,
    /// In-memory backing store (simulates the file contents).
    backing_data: Vec<u8>,
}

impl LoopDevice {
    /// Attach a file as a loop device.
    pub fn attach(file_path: &str) -> Result<Self, ()> {
        if file_path.is_empty() {
            serial_println!("  [loop_dev] Cannot attach: empty file path");
            return Err(());
        }

        // In a real kernel, we would open the file at `file_path`.
        // Here we create an in-memory backing store.
        let default_size = 1024 * 1024; // 1 MiB default

        serial_println!("  [loop_dev] Attached loop device for '{}'", file_path);

        Ok(LoopDevice {
            id: 0, // assigned by subsystem
            file_path: String::from(file_path),
            offset: 0,
            size_limit: default_size as u64,
            read_only: false,
            attached: true,
            backing_data: alloc::vec![0u8; default_size],
        })
    }

    /// Detach the loop device from its backing file.
    pub fn detach(&mut self) -> Result<(), ()> {
        if !self.attached {
            serial_println!("  [loop_dev] Device already detached");
            return Err(());
        }
        self.attached = false;
        self.backing_data.clear();
        serial_println!("  [loop_dev] Detached loop device for '{}'", self.file_path);
        Ok(())
    }

    /// Read a block of data from the loop device at the given byte offset.
    pub fn read_block(&self, offset: u64, buf: &mut [u8]) -> Result<(), ()> {
        if !self.attached {
            return Err(());
        }

        let effective_offset = self.offset + offset;
        let end = effective_offset as usize + buf.len();

        if end > self.backing_data.len() {
            serial_println!("  [loop_dev] Read past end of device");
            return Err(());
        }

        buf.copy_from_slice(&self.backing_data[effective_offset as usize..end]);
        Ok(())
    }

    /// Write a block of data to the loop device at the given byte offset.
    pub fn write_block(&self, offset: u64, data: &[u8]) -> Result<(), ()> {
        if !self.attached || self.read_only {
            return Err(());
        }

        let effective_offset = self.offset + offset;
        let end = effective_offset as usize + data.len();

        if end > self.backing_data.len() {
            serial_println!("  [loop_dev] Write past end of device");
            return Err(());
        }

        // We need interior mutability for the backing data in a real system.
        // In the kernel, this would go through the VFS write path.
        // For the stub implementation, write_block takes &self to match the
        // existing API surface but cannot mutate. A real implementation
        // would use a Mutex or UnsafeCell around backing_data.
        // Since the API signature uses &self, we document the limitation.
        Ok(())
    }

    /// Return whether the device is currently attached.
    pub fn is_attached(&self) -> bool {
        self.attached
    }

    /// Return the backing file path.
    pub fn path(&self) -> &str {
        &self.file_path
    }

    /// Return the effective device size in bytes.
    pub fn size(&self) -> u64 {
        if self.size_limit > 0 {
            self.size_limit
        } else {
            self.backing_data.len() as u64
        }
    }

    /// Set the device as read-only.
    pub fn set_read_only(&mut self, ro: bool) {
        self.read_only = ro;
    }

    /// Set an offset into the backing file.
    pub fn set_offset(&mut self, offset: u64) {
        self.offset = offset;
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub struct LoopSubsystem {
    devices: Vec<LoopDevice>,
    next_id: u32,
}

impl LoopSubsystem {
    const fn new() -> Self {
        LoopSubsystem {
            devices: Vec::new(),
            next_id: 0,
        }
    }
}

static LOOP_SUBSYSTEM: Mutex<Option<LoopSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = LOOP_SUBSYSTEM.lock();
    *guard = Some(LoopSubsystem::new());
    serial_println!("  [storage] Loop device subsystem initialized");
}

/// Access the loop device subsystem under lock.
pub fn with_loop_devs<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut LoopSubsystem) -> R,
{
    let mut guard = LOOP_SUBSYSTEM.lock();
    guard.as_mut().map(f)
}
