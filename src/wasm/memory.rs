/// Linear memory management
///
/// Part of the AIOS.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

/// WASM page size (64 KiB).
pub const WASM_PAGE_SIZE: usize = 65536;

/// Maximum number of pages (4 GiB / 64 KiB = 65536).
pub const MAX_PAGES: u32 = 65536;

/// A WASM linear memory instance (growable byte array).
pub struct LinearMemory {
    data: Vec<u8>,
    current_pages: u32,
    max_pages: Option<u32>,
}

impl LinearMemory {
    pub fn new(initial_pages: u32, max_pages: Option<u32>) -> Self {
        let pages = initial_pages.min(MAX_PAGES);
        let size = pages as usize * WASM_PAGE_SIZE;
        LinearMemory {
            data: vec![0u8; size],
            current_pages: pages,
            max_pages,
        }
    }

    /// Grow memory by delta pages. Returns previous size (in pages) or error.
    pub fn grow(&mut self, delta: u32) -> Result<u32, ()> {
        let old_pages = self.current_pages;
        let new_pages = old_pages.checked_add(delta).ok_or(())?;

        // Check maximum
        if let Some(max) = self.max_pages {
            if new_pages > max {
                return Err(());
            }
        }
        if new_pages > MAX_PAGES {
            return Err(());
        }

        // Extend the backing store
        let additional = delta as usize * WASM_PAGE_SIZE;
        self.data.resize(self.data.len() + additional, 0);
        self.current_pages = new_pages;

        crate::serial_println!(
            "[wasm/memory] grow {} -> {} pages ({} KiB)",
            old_pages, new_pages, new_pages as usize * 64
        );

        Ok(old_pages)
    }

    /// Read bytes from linear memory at offset.
    ///
    /// Returns a slice of the requested region, or an empty slice if out of bounds.
    pub fn read(&self, offset: u32, len: u32) -> &[u8] {
        let start = offset as usize;
        let end = start + len as usize;
        if end > self.data.len() {
            // Out of bounds: return empty slice
            &[]
        } else {
            &self.data[start..end]
        }
    }

    /// Write bytes into linear memory at offset.
    ///
    /// Silently truncates if the write would exceed memory bounds.
    pub fn write(&mut self, offset: u32, data: &[u8]) {
        let start = offset as usize;
        let end = start + data.len();
        if end <= self.data.len() {
            self.data[start..end].copy_from_slice(data);
        } else if start < self.data.len() {
            // Partial write up to boundary
            let available = self.data.len() - start;
            self.data[start..start + available].copy_from_slice(&data[..available]);
        }
    }

    /// Current size in pages.
    pub fn size_pages(&self) -> u32 {
        self.current_pages
    }

    /// Current size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }

    /// Read a single byte.
    pub fn load_u8(&self, addr: u32) -> u8 {
        if (addr as usize) < self.data.len() {
            self.data[addr as usize]
        } else {
            0
        }
    }

    /// Write a single byte.
    pub fn store_u8(&mut self, addr: u32, val: u8) {
        if (addr as usize) < self.data.len() {
            self.data[addr as usize] = val;
        }
    }

    /// Read a little-endian u32.
    pub fn load_u32(&self, addr: u32) -> u32 {
        let bytes = self.read(addr, 4);
        if bytes.len() == 4 {
            (bytes[0] as u32)
                | ((bytes[1] as u32) << 8)
                | ((bytes[2] as u32) << 16)
                | ((bytes[3] as u32) << 24)
        } else {
            0
        }
    }

    /// Write a little-endian u32.
    pub fn store_u32(&mut self, addr: u32, val: u32) {
        let bytes = val.to_le_bytes();
        self.write(addr, &bytes);
    }

    /// Read a little-endian u64.
    pub fn load_u64(&self, addr: u32) -> u64 {
        let bytes = self.read(addr, 8);
        if bytes.len() == 8 {
            u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        } else {
            0
        }
    }

    /// Write a little-endian u64.
    pub fn store_u64(&mut self, addr: u32, val: u64) {
        let bytes = val.to_le_bytes();
        self.write(addr, &bytes);
    }
}

pub fn init() {
    crate::serial_println!("[wasm] linear memory manager ready");
}
