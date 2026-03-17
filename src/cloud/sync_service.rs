use crate::sync::Mutex;
/// Cloud sync service for Genesis
///
/// Real-time sync, conflict resolution,
/// selective sync, bandwidth management.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SyncStatus {
    UpToDate,
    Syncing,
    Pending,
    Conflict,
    Error,
}

struct SyncItem {
    id: u32,
    path_hash: u64,
    local_version: u64,
    remote_version: u64,
    status: SyncStatus,
    size_bytes: u64,
    last_sync: u64,
}

struct SyncService {
    items: Vec<SyncItem>,
    syncing: bool,
    bandwidth_limit_kbps: Option<u32>,
    total_synced_bytes: u64,
    conflicts: u32,
}

static SYNC_SVC: Mutex<Option<SyncService>> = Mutex::new(None);

impl SyncService {
    fn new() -> Self {
        SyncService {
            items: Vec::new(),
            syncing: false,
            bandwidth_limit_kbps: None,
            total_synced_bytes: 0,
            conflicts: 0,
        }
    }

    fn needs_sync(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|i| i.local_version != i.remote_version)
            .map(|i| i.id)
            .collect()
    }

    fn resolve_conflict(&mut self, item_id: u32, use_local: bool) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == item_id) {
            if use_local {
                item.remote_version = item.local_version;
            } else {
                item.local_version = item.remote_version;
            }
            item.status = SyncStatus::UpToDate;
            self.conflicts = self.conflicts.saturating_sub(1);
        }
    }
}

pub fn init() {
    let mut s = SYNC_SVC.lock();
    *s = Some(SyncService::new());
    serial_println!("    Cloud: sync service ready");
}
