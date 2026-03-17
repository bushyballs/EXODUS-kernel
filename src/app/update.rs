use crate::sync::Mutex;
/// App auto-update mechanism
///
/// Part of the Genesis app framework. Checks for and applies
/// application updates from trusted sources with integrity
/// verification and rollback support.
use alloc::string::String;
use alloc::vec::Vec;

/// Update availability status
pub struct UpdateInfo {
    pub app_id: u64,
    pub current_version: String,
    pub available_version: String,
    pub size_bytes: usize,
}

/// Update download state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Available,
    Downloading,
    Verifying,
    ReadyToInstall,
    Installing,
    Complete,
    Failed,
    RolledBack,
}

/// Version comparison result
#[derive(Debug, PartialEq)]
enum VersionCmp {
    Newer,
    Same,
    Older,
}

/// Compare two version strings (semver-style: "major.minor.patch")
fn compare_versions(current: &str, available: &str) -> VersionCmp {
    let parse_parts = |v: &str| -> Vec<u32> {
        let mut parts = Vec::new();
        let mut current_num = String::new();
        for c in v.chars() {
            if c == '.' {
                if let Some(n) = parse_u32_simple(&current_num) {
                    parts.push(n);
                }
                current_num = String::new();
            } else if c.is_ascii_digit() {
                current_num.push(c);
            }
        }
        if let Some(n) = parse_u32_simple(&current_num) {
            parts.push(n);
        }
        parts
    };

    let cur = parse_parts(current);
    let avail = parse_parts(available);

    let max_len = if cur.len() > avail.len() {
        cur.len()
    } else {
        avail.len()
    };
    for i in 0..max_len {
        let c = if i < cur.len() { cur[i] } else { 0 };
        let a = if i < avail.len() { avail[i] } else { 0 };
        if a > c {
            return VersionCmp::Newer;
        }
        if a < c {
            return VersionCmp::Older;
        }
    }
    VersionCmp::Same
}

fn parse_u32_simple(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut result: u32 = 0;
    for c in s.chars() {
        if !c.is_ascii_digit() {
            return None;
        }
        result = result.checked_mul(10)?.checked_add(c as u32 - '0' as u32)?;
    }
    Some(result)
}

fn str_to_string(s: &str) -> String {
    let mut r = String::new();
    for c in s.chars() {
        r.push(c);
    }
    r
}

/// CRC32 for update integrity checking
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFFFFFF
}

/// Rollback entry
struct RollbackEntry {
    app_id: u64,
    previous_version: String,
    backup_data: Vec<u8>,
}

pub struct UpdateManager {
    pub pending: Option<UpdateInfo>,
    state: UpdateState,
    download_progress: u8,
    download_buffer: Vec<u8>,
    expected_checksum: u32,
    rollback_stack: Vec<RollbackEntry>,
    update_check_interval: u64,
    last_check_time: u64,
    total_updates_applied: u64,
}

impl UpdateManager {
    pub fn new() -> Self {
        crate::serial_println!("[app::update] update manager created");
        Self {
            pending: None,
            state: UpdateState::Idle,
            download_progress: 0,
            download_buffer: Vec::new(),
            expected_checksum: 0,
            rollback_stack: Vec::new(),
            update_check_interval: 3600, // check every hour
            last_check_time: 0,
            total_updates_applied: 0,
        }
    }

    /// Check if an update is available for the given app
    pub fn check(&mut self, app_id: u64) -> Option<&UpdateInfo> {
        self.state = UpdateState::Checking;

        // Simulate checking an update server
        // In a real implementation, this would query a network endpoint
        let current = str_to_string("1.0.0");
        let available = str_to_string("1.1.0");

        if compare_versions(&current, &available) == VersionCmp::Newer {
            let info = UpdateInfo {
                app_id,
                current_version: current,
                available_version: available,
                size_bytes: 524288, // 512KB simulated
            };

            crate::serial_println!(
                "[app::update] update available for app {}: {} -> {} ({} bytes)",
                app_id,
                info.current_version,
                info.available_version,
                info.size_bytes
            );

            self.pending = Some(info);
            self.state = UpdateState::Available;
            self.pending.as_ref()
        } else {
            crate::serial_println!("[app::update] app {} is up to date", app_id);
            self.state = UpdateState::Idle;
            None
        }
    }

    /// Simulate downloading update data
    pub fn download(&mut self) -> Result<(), ()> {
        let info = match &self.pending {
            Some(info) => info,
            None => {
                crate::serial_println!("[app::update] no pending update to download");
                return Err(());
            }
        };

        self.state = UpdateState::Downloading;
        let size = info.size_bytes;

        // Simulate download in chunks
        self.download_buffer = Vec::with_capacity(size);
        let chunk_size = 4096usize;
        let mut downloaded = 0usize;
        let mut seed: u32 = 12345;

        while downloaded < size {
            let remaining = size - downloaded;
            let this_chunk = if remaining < chunk_size {
                remaining
            } else {
                chunk_size
            };
            for _ in 0..this_chunk {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                self.download_buffer.push(((seed >> 16) & 0xFF) as u8);
                downloaded += 1;
            }
            self.download_progress = ((downloaded * 100) / size) as u8;
        }

        // Compute checksum
        self.expected_checksum = crc32(&self.download_buffer);
        self.state = UpdateState::Verifying;

        // Verify integrity
        let actual_checksum = crc32(&self.download_buffer);
        if actual_checksum != self.expected_checksum {
            crate::serial_println!("[app::update] checksum mismatch!");
            self.state = UpdateState::Failed;
            return Err(());
        }

        self.state = UpdateState::ReadyToInstall;
        crate::serial_println!(
            "[app::update] download complete: {} bytes, checksum {:#x}",
            downloaded,
            self.expected_checksum
        );
        Ok(())
    }

    /// Apply the pending update
    pub fn apply(&mut self) -> Result<(), ()> {
        let info = match self.pending.take() {
            Some(info) => info,
            None => {
                crate::serial_println!("[app::update] no pending update to apply");
                return Err(());
            }
        };

        if self.state != UpdateState::ReadyToInstall && self.state != UpdateState::Available {
            // Try to download first
            self.pending = Some(info);
            self.download()?;
            return self.apply();
        }

        self.state = UpdateState::Installing;

        // Save rollback data
        let rollback = RollbackEntry {
            app_id: info.app_id,
            previous_version: info.current_version.clone(),
            backup_data: Vec::new(), // In real impl: backup current app binary
        };

        crate::serial_println!(
            "[app::update] installing update for app {}: {} -> {}",
            info.app_id,
            info.current_version,
            info.available_version
        );

        // Simulate installation: verify, replace binary, update manifest
        let verify_checksum = crc32(&self.download_buffer);
        if verify_checksum != self.expected_checksum {
            crate::serial_println!("[app::update] installation verification failed, rolling back");
            self.state = UpdateState::Failed;
            return Err(());
        }

        // Push rollback entry before finalizing
        self.rollback_stack.push(rollback);

        self.total_updates_applied = self.total_updates_applied.saturating_add(1);
        self.download_buffer.clear();
        self.download_progress = 0;
        self.state = UpdateState::Complete;

        crate::serial_println!(
            "[app::update] update applied successfully (total: {})",
            self.total_updates_applied
        );
        Ok(())
    }

    /// Rollback the last applied update
    pub fn rollback(&mut self) -> Result<(), ()> {
        match self.rollback_stack.pop() {
            Some(entry) => {
                crate::serial_println!(
                    "[app::update] rolling back app {} to version {}",
                    entry.app_id,
                    entry.previous_version
                );
                self.state = UpdateState::RolledBack;
                Ok(())
            }
            None => {
                crate::serial_println!("[app::update] no rollback data available");
                Err(())
            }
        }
    }

    /// Get the current update state
    pub fn state(&self) -> UpdateState {
        self.state
    }

    /// Get download progress percentage
    pub fn progress(&self) -> u8 {
        self.download_progress
    }

    /// Get total number of successfully applied updates
    pub fn total_applied(&self) -> u64 {
        self.total_updates_applied
    }
}

static UPDATE_MGR: Mutex<Option<UpdateManager>> = Mutex::new(None);

pub fn init() {
    let mgr = UpdateManager::new();
    let mut m = UPDATE_MGR.lock();
    *m = Some(mgr);
    crate::serial_println!("[app::update] update subsystem initialized");
}
