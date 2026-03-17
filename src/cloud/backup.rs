use crate::sync::Mutex;
/// Cloud backup for Genesis
///
/// Automatic backup, incremental, encrypted,
/// selective backup, restore, backup scheduling.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum BackupState {
    Idle,
    Preparing,
    Uploading,
    Completed,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BackupType {
    Full,
    Incremental,
    Differential,
}

struct BackupJob {
    id: u32,
    backup_type: BackupType,
    state: BackupState,
    total_bytes: u64,
    uploaded_bytes: u64,
    files_total: u32,
    files_done: u32,
    encrypted: bool,
    timestamp: u64,
}

struct BackupConfig {
    enabled: bool,
    wifi_only: bool,
    charging_only: bool,
    schedule_hour: u8,
    include_photos: bool,
    include_apps: bool,
    include_settings: bool,
    include_messages: bool,
    max_backup_size_gb: u32,
}

struct BackupEngine {
    jobs: Vec<BackupJob>,
    config: BackupConfig,
    next_id: u32,
    total_backed_up_bytes: u64,
    last_successful: u64,
}

static BACKUP: Mutex<Option<BackupEngine>> = Mutex::new(None);

impl BackupEngine {
    fn new() -> Self {
        BackupEngine {
            jobs: Vec::new(),
            config: BackupConfig {
                enabled: true,
                wifi_only: true,
                charging_only: true,
                schedule_hour: 3,
                include_photos: true,
                include_apps: true,
                include_settings: true,
                include_messages: true,
                max_backup_size_gb: 50,
            },
            next_id: 1,
            total_backed_up_bytes: 0,
            last_successful: 0,
        }
    }

    fn start_backup(&mut self, btype: BackupType, timestamp: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.jobs.push(BackupJob {
            id,
            backup_type: btype,
            state: BackupState::Preparing,
            total_bytes: 0,
            uploaded_bytes: 0,
            files_total: 0,
            files_done: 0,
            encrypted: true,
            timestamp,
        });
        id
    }

    fn should_backup(&self, hour: u8, on_wifi: bool, charging: bool) -> bool {
        if !self.config.enabled {
            return false;
        }
        if self.config.wifi_only && !on_wifi {
            return false;
        }
        if self.config.charging_only && !charging {
            return false;
        }
        hour == self.config.schedule_hour
    }
}

pub fn init() {
    let mut b = BACKUP.lock();
    *b = Some(BackupEngine::new());
    serial_println!("    Cloud: encrypted backup service ready");
}
