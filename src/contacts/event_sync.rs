use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SyncState {
    Idle,
    Syncing,
    Error,
    Conflict,
}

#[derive(Clone, Copy)]
pub struct SyncAccount {
    pub id: u8,
    pub server_hash: u64,
    pub username_hash: u64,
    pub last_sync: u64,
    pub sync_token: u64,
    pub state: SyncState,
    pub calendar_count: u8,
}

impl SyncAccount {
    pub fn new(id: u8, server_hash: u64, username_hash: u64) -> Self {
        Self {
            id,
            server_hash,
            username_hash,
            last_sync: 0,
            sync_token: 0,
            state: SyncState::Idle,
            calendar_count: 0,
        }
    }
}

pub struct EventSyncEngine {
    accounts: Vec<SyncAccount>,
    pending_changes: Vec<u32>,
    conflict_count: u32,
}

impl EventSyncEngine {
    pub fn new() -> Self {
        Self {
            accounts: vec![],
            pending_changes: vec![],
            conflict_count: 0,
        }
    }

    pub fn register_account(&mut self, server_hash: u64, username_hash: u64) -> u8 {
        let id = self.accounts.len() as u8;
        let account = SyncAccount::new(id, server_hash, username_hash);
        self.accounts.push(account);
        id
    }

    pub fn start_sync(&mut self, account_id: u8, current_time: u64) -> bool {
        if let Some(account) = self.accounts.get_mut(account_id as usize) {
            if account.state == SyncState::Idle {
                account.state = SyncState::Syncing;
                account.last_sync = current_time;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn resolve_conflict(&mut self, event_id: u32, keep_local: bool) -> bool {
        if let Some(pos) = self.pending_changes.iter().position(|&id| id == event_id) {
            if keep_local {
                // Keep the local version, mark for upload
                // In a real implementation, this would queue the upload
            } else {
                // Discard local changes
                self.pending_changes.remove(pos);
            }

            if self.conflict_count > 0 {
                self.conflict_count = self.conflict_count.saturating_sub(1);
            }
            true
        } else {
            false
        }
    }

    pub fn get_pending_count(&self) -> usize {
        self.pending_changes.len()
    }

    pub fn mark_synced(&mut self, account_id: u8, sync_token: u64) -> bool {
        if let Some(account) = self.accounts.get_mut(account_id as usize) {
            account.state = SyncState::Idle;
            account.sync_token = sync_token;
            true
        } else {
            false
        }
    }

    pub fn add_pending_change(&mut self, event_id: u32) {
        if !self.pending_changes.contains(&event_id) {
            self.pending_changes.push(event_id);
        }
    }

    pub fn report_conflict(&mut self, event_id: u32) {
        self.add_pending_change(event_id);
        self.conflict_count = self.conflict_count.saturating_add(1);
    }

    pub fn set_account_state(&mut self, account_id: u8, state: SyncState) -> bool {
        if let Some(account) = self.accounts.get_mut(account_id as usize) {
            account.state = state;
            true
        } else {
            false
        }
    }

    pub fn get_account(&self, account_id: u8) -> Option<&SyncAccount> {
        self.accounts.get(account_id as usize)
    }

    pub fn total_accounts(&self) -> usize {
        self.accounts.len()
    }

    pub fn total_conflicts(&self) -> u32 {
        self.conflict_count
    }
}

static EVENT_SYNC: Mutex<Option<EventSyncEngine>> = Mutex::new(None);

pub fn init() {
    let mut sync = EVENT_SYNC.lock();
    *sync = Some(EventSyncEngine::new());
    serial_println!("[CONTACTS] Event sync engine initialized");
}

pub fn get_sync() -> &'static Mutex<Option<EventSyncEngine>> {
    &EVENT_SYNC
}
