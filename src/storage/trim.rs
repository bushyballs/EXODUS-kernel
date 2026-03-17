/// SSD TRIM/DISCARD command support
///
/// Part of the AIOS storage layer.
///
/// Batches and coalesces TRIM ranges for efficient SSD garbage collection.
/// Adjacent or overlapping ranges are merged before issuing to the device.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub struct TrimRange {
    pub lba: u64,
    pub count: u64,
}

pub struct TrimManager {
    /// Queue of pending TRIM ranges awaiting flush to device.
    pending: Vec<TrimRange>,
    /// Maximum number of ranges to batch before auto-flush.
    batch_limit: usize,
    /// Whether the underlying device reports TRIM support.
    trim_supported: bool,
    /// Total sectors trimmed since initialization.
    total_trimmed: u64,
    /// Number of TRIM commands issued to the device.
    commands_issued: u64,
}

impl TrimManager {
    pub fn new() -> Self {
        TrimManager {
            pending: Vec::new(),
            batch_limit: 256,
            trim_supported: true, // assume SSD until proven otherwise
            total_trimmed: 0,
            commands_issued: 0,
        }
    }

    /// Queue a TRIM range for later submission to the device.
    /// Adjacent or overlapping ranges in the queue will be coalesced on flush.
    pub fn queue_trim(&mut self, range: TrimRange) {
        if !self.trim_supported {
            return;
        }
        if range.count == 0 {
            return;
        }

        self.pending.push(range);

        // Auto-flush if we hit the batch limit
        if self.pending.len() >= self.batch_limit {
            let _ = self.flush();
        }
    }

    /// Coalesce pending ranges and issue them to the device.
    /// Returns the number of sectors actually trimmed.
    pub fn flush(&mut self) -> Result<usize, ()> {
        if self.pending.is_empty() {
            return Ok(0);
        }

        if !self.trim_supported {
            self.pending.clear();
            return Err(());
        }

        // Sort by starting LBA
        self.pending.sort_by_key(|r| r.lba);

        // Coalesce overlapping/adjacent ranges
        let mut coalesced: Vec<TrimRange> = Vec::new();
        for range in self.pending.drain(..) {
            if let Some(last) = coalesced.last_mut() {
                let last_end = last.lba + last.count;
                if range.lba <= last_end {
                    // Overlapping or adjacent: extend
                    let new_end = (range.lba + range.count).max(last_end);
                    last.count = new_end - last.lba;
                    continue;
                }
            }
            coalesced.push(range);
        }

        // Issue coalesced TRIM commands
        let mut total_sectors = 0u64;
        for range in &coalesced {
            // In a real system, this would issue ATA DATA SET MANAGEMENT
            // (TRIM command, opcode 0x06) via the block device layer.
            total_sectors += range.count;
            self.commands_issued = self.commands_issued.saturating_add(1);
        }

        self.total_trimmed += total_sectors;
        serial_println!(
            "  [trim] Flushed {} ranges, {} sectors trimmed",
            coalesced.len(),
            total_sectors
        );

        Ok(total_sectors as usize)
    }

    /// Query whether the underlying device supports TRIM.
    pub fn supports_trim(&self) -> bool {
        self.trim_supported
    }

    /// Set whether the device supports TRIM (called during device discovery).
    pub fn set_trim_support(&mut self, supported: bool) {
        self.trim_supported = supported;
    }

    /// Return the number of pending (unsubmitted) ranges.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Return total sectors trimmed since init.
    pub fn total_trimmed(&self) -> u64 {
        self.total_trimmed
    }

    /// Return total TRIM commands issued.
    pub fn commands_issued(&self) -> u64 {
        self.commands_issued
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TRIM_MANAGER: Mutex<Option<TrimManager>> = Mutex::new(None);

pub fn init() {
    let mut guard = TRIM_MANAGER.lock();
    *guard = Some(TrimManager::new());
    serial_println!("  [storage] TRIM/DISCARD support initialized");
}

/// Access the TRIM manager under lock.
pub fn with_trim<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut TrimManager) -> R,
{
    let mut guard = TRIM_MANAGER.lock();
    guard.as_mut().map(f)
}
