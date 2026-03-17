/// Log file rotation
///
/// Part of the AIOS logging infrastructure. Manages log file rotation
/// based on size thresholds. Tracks current file size and triggers
/// rotation when the threshold is exceeded, maintaining a configurable
/// number of rotated log files.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

/// Rotation event record
struct RotationEvent {
    timestamp: u64,
    old_file_index: u32,
    new_file_index: u32,
    bytes_at_rotation: u64,
}

/// Represents a rotated log file
struct RotatedFile {
    index: u32,
    size_bytes: u64,
    created_at: u64,
}

static ROTATE_TICK: Mutex<u64> = Mutex::new(0);

fn rot_tick() -> u64 {
    let mut t = ROTATE_TICK.lock();
    *t = t.saturating_add(1);
    *t
}

/// Manages log file rotation based on size or time.
pub struct LogRotation {
    max_size_bytes: u64,
    max_files: u32,
    current_size: u64,
    current_file_index: u32,
    rotated_files: Vec<RotatedFile>,
    rotation_history: Vec<RotationEvent>,
    total_rotations: u64,
    total_bytes_written: u64,
    compress_rotated: bool,
}

impl LogRotation {
    pub fn new(max_size: u64, max_files: u32) -> Self {
        let effective_size = if max_size == 0 { 1024 * 1024 } else { max_size }; // default 1MB
        let effective_files = if max_files == 0 { 5 } else { max_files };
        crate::serial_println!("[log::rotate] rotation created: max_size={} bytes, max_files={}",
            effective_size, effective_files);
        Self {
            max_size_bytes: effective_size,
            max_files: effective_files,
            current_size: 0,
            current_file_index: 0,
            rotated_files: Vec::new(),
            rotation_history: Vec::new(),
            total_rotations: 0,
            total_bytes_written: 0,
            compress_rotated: false,
        }
    }

    /// Check if rotation is needed and perform it.
    pub fn check_and_rotate(&mut self) -> bool {
        if self.current_size < self.max_size_bytes {
            return false;
        }

        let now = rot_tick();

        // Record the rotation event
        let old_index = self.current_file_index;
        let new_index = self.current_file_index + 1;

        self.rotation_history.push(RotationEvent {
            timestamp: now,
            old_file_index: old_index,
            new_file_index: new_index,
            bytes_at_rotation: self.current_size,
        });

        // Move current file to rotated list
        self.rotated_files.push(RotatedFile {
            index: old_index,
            size_bytes: self.current_size,
            created_at: now,
        });

        // Prune old rotated files if over limit
        while self.rotated_files.len() > self.max_files as usize {
            let removed = self.rotated_files.remove(0);
            crate::serial_println!("[log::rotate] pruned old log file index {} ({} bytes)",
                removed.index, removed.size_bytes);
        }

        // Start new file
        self.current_file_index = new_index;
        self.current_size = 0;
        self.total_rotations = self.total_rotations.saturating_add(1);

        crate::serial_println!("[log::rotate] rotated: file {} -> {} (rotation #{}, {} rotated files)",
            old_index, new_index, self.total_rotations, self.rotated_files.len());

        true
    }

    /// Record that bytes were written to the current log file.
    pub fn record_write(&mut self, bytes: u64) {
        self.current_size += bytes;
        self.total_bytes_written += bytes;
    }

    /// Get the current file size
    pub fn current_size(&self) -> u64 {
        self.current_size
    }

    /// Get the fill percentage of the current file
    pub fn fill_pct(&self) -> u8 {
        if self.max_size_bytes == 0 { return 0; }
        ((self.current_size * 100) / self.max_size_bytes) as u8
    }

    /// Get the number of rotated files
    pub fn rotated_count(&self) -> usize {
        self.rotated_files.len()
    }

    /// Get total bytes written across all rotations
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes_written
    }

    /// Get total rotation count
    pub fn rotation_count(&self) -> u64 {
        self.total_rotations
    }

    /// Set compression for rotated files
    pub fn set_compress(&mut self, enabled: bool) {
        self.compress_rotated = enabled;
    }

    /// Get the maximum file size threshold
    pub fn max_size(&self) -> u64 {
        self.max_size_bytes
    }

    /// Force a rotation regardless of current size
    pub fn force_rotate(&mut self) {
        let saved_max = self.max_size_bytes;
        self.max_size_bytes = 0;
        self.check_and_rotate();
        self.max_size_bytes = saved_max;
    }
}

static ROTATION: Mutex<Option<LogRotation>> = Mutex::new(None);

pub fn init() {
    let rotation = LogRotation::new(10 * 1024 * 1024, 5); // 10MB, 5 files
    let mut r = ROTATION.lock();
    *r = Some(rotation);
    crate::serial_println!("[log::rotate] rotation subsystem initialized");
}
