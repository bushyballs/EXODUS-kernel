/// Function/element tables
///
/// Part of the AIOS.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

/// WASM table of function references for indirect calls.
///
/// Tables store references (currently function indices) and are used
/// by call_indirect to dispatch function calls at runtime.
pub struct FuncTable {
    entries: Vec<Option<u32>>,
    max_size: Option<u32>,
}

impl FuncTable {
    pub fn new(initial_size: u32, max_size: Option<u32>) -> Self {
        let size = initial_size as usize;
        FuncTable {
            entries: vec![None; size],
            max_size,
        }
    }

    /// Get the function index at a table slot.
    ///
    /// Returns None if the slot is empty or out of bounds.
    pub fn get(&self, index: u32) -> Option<u32> {
        let idx = index as usize;
        if idx < self.entries.len() {
            self.entries[idx]
        } else {
            None
        }
    }

    /// Set a table slot to a function index.
    ///
    /// Silently ignores out-of-bounds writes.
    pub fn set(&mut self, index: u32, func_idx: u32) {
        let idx = index as usize;
        if idx < self.entries.len() {
            self.entries[idx] = Some(func_idx);
        }
    }

    /// Grow the table by delta entries, initializing new slots to None.
    /// Returns the previous size, or Err if growth would exceed max_size.
    pub fn grow(&mut self, delta: u32) -> Result<u32, ()> {
        let old_size = self.entries.len() as u32;
        let new_size = old_size.checked_add(delta).ok_or(())?;

        if let Some(max) = self.max_size {
            if new_size > max {
                return Err(());
            }
        }

        self.entries.resize(new_size as usize, None);

        crate::serial_println!(
            "[wasm/table] grow {} -> {} slots",
            old_size, new_size
        );

        Ok(old_size)
    }

    /// Current table size (number of slots).
    pub fn size(&self) -> u32 {
        self.entries.len() as u32
    }

    /// Clear a table slot (set to None).
    pub fn clear(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.entries.len() {
            self.entries[idx] = None;
        }
    }

    /// Initialize a range of table entries from an element segment.
    pub fn init_segment(&mut self, table_offset: u32, func_indices: &[u32]) {
        for (i, &func_idx) in func_indices.iter().enumerate() {
            let slot = table_offset as usize + i;
            if slot < self.entries.len() {
                self.entries[slot] = Some(func_idx);
            }
        }
    }

    /// Copy entries from another table.
    pub fn copy_from(&mut self, dst_offset: u32, src: &FuncTable, src_offset: u32, count: u32) {
        for i in 0..count {
            let src_idx = (src_offset + i) as usize;
            let dst_idx = (dst_offset + i) as usize;
            if src_idx < src.entries.len() && dst_idx < self.entries.len() {
                self.entries[dst_idx] = src.entries[src_idx];
            }
        }
    }
}

pub fn init() {
    crate::serial_println!("[wasm] function table manager ready");
}
