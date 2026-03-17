/// Sync engine for Genesis
///
/// Data synchronization across devices, conflict resolution,
/// incremental sync, and offline queue.
///
/// Inspired by: Android SyncAdapter, iCloud. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Sync state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Idle,
    Syncing,
    Error,
    Paused,
}

/// Conflict resolution strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    LastWriteWins,
    ServerWins,
    ClientWins,
    Manual,
}

/// A sync item
pub struct SyncItem {
    pub id: String,
    pub collection: String,
    pub version: u64,
    pub data_hash: u64,
    pub modified_at: u64,
    pub synced: bool,
}

/// Sync collection (a category of data to sync)
pub struct SyncCollection {
    pub name: String,
    pub enabled: bool,
    pub interval_secs: u64,
    pub last_sync: u64,
    pub item_count: u64,
    pub conflict_strategy: ConflictStrategy,
}

/// Sync engine
pub struct SyncEngine {
    pub state: SyncState,
    pub collections: Vec<SyncCollection>,
    pub pending_items: Vec<SyncItem>,
    pub conflicts: Vec<(SyncItem, SyncItem)>, // local, remote
    pub total_synced: u64,
    pub total_conflicts: u64,
    pub wifi_only: bool,
    pub battery_threshold: u8,
}

impl SyncEngine {
    const fn new() -> Self {
        SyncEngine {
            state: SyncState::Idle,
            collections: Vec::new(),
            pending_items: Vec::new(),
            conflicts: Vec::new(),
            total_synced: 0,
            total_conflicts: 0,
            wifi_only: true,
            battery_threshold: 15,
        }
    }

    pub fn add_collection(&mut self, name: &str, interval: u64) {
        self.collections.push(SyncCollection {
            name: String::from(name),
            enabled: true,
            interval_secs: interval,
            last_sync: 0,
            item_count: 0,
            conflict_strategy: ConflictStrategy::LastWriteWins,
        });
    }

    pub fn queue_item(&mut self, id: &str, collection: &str, version: u64, hash: u64) {
        self.pending_items.push(SyncItem {
            id: String::from(id),
            collection: String::from(collection),
            version,
            data_hash: hash,
            modified_at: crate::time::clock::unix_time(),
            synced: false,
        });
    }

    pub fn sync_now(&mut self) {
        if self.state == SyncState::Syncing {
            return;
        }
        self.state = SyncState::Syncing;

        // Process pending items
        let mut synced = 0u64;
        for item in &mut self.pending_items {
            if !item.synced {
                item.synced = true;
                synced += 1;
            }
        }
        self.total_synced = self.total_synced.saturating_add(synced);

        // Remove synced items
        self.pending_items.retain(|i| !i.synced);

        // Update collection timestamps
        let now = crate::time::clock::unix_time();
        for col in &mut self.collections {
            if col.enabled {
                col.last_sync = now;
            }
        }

        self.state = SyncState::Idle;
    }

    pub fn check_due(&self) -> Vec<&SyncCollection> {
        let now = crate::time::clock::unix_time();
        self.collections
            .iter()
            .filter(|c| c.enabled && (now - c.last_sync) >= c.interval_secs)
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending_items.len()
    }

    pub fn pause(&mut self) {
        self.state = SyncState::Paused;
    }
    pub fn resume(&mut self) {
        self.state = SyncState::Idle;
    }
}

static ENGINE: Mutex<SyncEngine> = Mutex::new(SyncEngine::new());

pub fn init() {
    let mut engine = ENGINE.lock();
    engine.add_collection("contacts", 3600);
    engine.add_collection("calendar", 1800);
    engine.add_collection("settings", 7200);
    engine.add_collection("bookmarks", 3600);
    engine.add_collection("notes", 1800);
    crate::serial_println!(
        "  [crossdevice] Sync engine initialized ({} collections)",
        engine.collections.len()
    );
}
