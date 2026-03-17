use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Multi-device message synchronisation subsystem for Genesis OS
///
/// Provides:
///   - Per-device sync state tracking
///   - Conflict detection and resolution (last-writer-wins + merge)
///   - Pending message queue with merge logic
///   - Sync lifecycle: start -> transfer -> resolve conflicts -> mark synced
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of pending messages queued per device before oldest are
/// dropped.
const MAX_PENDING_PER_DEVICE: usize = 2048;

/// Maximum tracked devices.
const MAX_DEVICES: usize = 32;

/// Sync session timeout in abstract time units (e.g. milliseconds).
const SYNC_TIMEOUT: u64 = 30_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Current sync status for a device.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Synced,
    Syncing,
    Pending,
    Conflict,
    Error,
}

/// Resolution strategy when two devices disagree about message state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConflictStrategy {
    /// Keep whichever version has the later timestamp.
    LastWriterWins,
    /// Merge both versions into the canonical set.
    Merge,
    /// Prefer the local device's version.
    PreferLocal,
    /// Prefer the remote device's version.
    PreferRemote,
}

/// A lightweight message reference used during sync (not the full payload).
#[derive(Clone)]
pub struct SyncMessage {
    pub msg_id: u64,
    pub conversation_id: u64,
    pub sender_hash: u64,
    pub timestamp: u64,
    pub content_hash: u64,
    /// True if the message has been deleted on the source device.
    pub deleted: bool,
}

/// Per-device synchronisation state.
pub struct SyncState {
    pub device_hash: u64,
    pub last_sync: u64,
    pub pending_count: usize,
    pub sync_status: SyncStatus,
    /// Messages waiting to be pushed to this device.
    pub pending_messages: Vec<SyncMessage>,
    /// Timestamp when the current sync session started (0 = no session).
    pub session_start: u64,
}

/// Conflict record produced when two devices have divergent state.
pub struct SyncConflict {
    pub msg_id: u64,
    pub local_hash: u64,
    pub remote_hash: u64,
    pub local_timestamp: u64,
    pub remote_timestamp: u64,
    pub resolved: bool,
    pub winning_hash: u64,
}

/// Top-level sync manager.
pub struct SyncManager {
    devices: Vec<SyncState>,
    conflicts: Vec<SyncConflict>,
    default_strategy: ConflictStrategy,
    next_conflict_id: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SYNC_MANAGER: Mutex<Option<SyncManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// SyncManager implementation
// ---------------------------------------------------------------------------

impl SyncManager {
    pub fn new() -> Self {
        Self {
            devices: vec![],
            conflicts: vec![],
            default_strategy: ConflictStrategy::LastWriterWins,
            next_conflict_id: 1,
        }
    }

    // ----- device registration -----

    /// Register a new device for sync.  No-op if already registered.
    pub fn register_device(&mut self, device_hash: u64) -> bool {
        if self.devices.len() >= MAX_DEVICES {
            serial_println!("[msg_sync] max devices reached");
            return false;
        }
        if self.find_device(device_hash).is_some() {
            return false; // already registered
        }
        self.devices.push(SyncState {
            device_hash,
            last_sync: 0,
            pending_count: 0,
            sync_status: SyncStatus::Pending,
            pending_messages: vec![],
            session_start: 0,
        });
        serial_println!("[msg_sync] registered device {:#X}", device_hash);
        true
    }

    /// Unregister a device.
    pub fn unregister_device(&mut self, device_hash: u64) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.device_hash != device_hash);
        self.devices.len() < before
    }

    // ----- helpers -----

    fn find_device(&self, device_hash: u64) -> Option<&SyncState> {
        self.devices.iter().find(|d| d.device_hash == device_hash)
    }

    fn find_device_mut(&mut self, device_hash: u64) -> Option<&mut SyncState> {
        self.devices
            .iter_mut()
            .find(|d| d.device_hash == device_hash)
    }

    // ----- sync lifecycle -----

    /// Begin a sync session for a device.
    pub fn start_sync(&mut self, device_hash: u64, now: u64) -> bool {
        if let Some(dev) = self.find_device_mut(device_hash) {
            if dev.sync_status == SyncStatus::Syncing {
                // Check for timeout on an existing session.
                if now.saturating_sub(dev.session_start) < SYNC_TIMEOUT {
                    serial_println!("[msg_sync] device {:#X} already syncing", device_hash);
                    return false;
                }
                // Timed out -- allow restart.
                serial_println!(
                    "[msg_sync] device {:#X} sync timed out, restarting",
                    device_hash
                );
            }
            dev.sync_status = SyncStatus::Syncing;
            dev.session_start = now;
            serial_println!("[msg_sync] started sync for device {:#X}", device_hash);
            true
        } else {
            false
        }
    }

    /// Enqueue a message to be synced to a device.
    pub fn enqueue_message(&mut self, device_hash: u64, msg: SyncMessage) -> bool {
        if let Some(dev) = self.find_device_mut(device_hash) {
            // Deduplicate: skip if we already have a pending entry for this msg_id.
            if dev.pending_messages.iter().any(|m| m.msg_id == msg.msg_id) {
                return false;
            }
            dev.pending_messages.push(msg);
            dev.pending_count = dev.pending_messages.len();
            // Prune oldest if over limit.
            if dev.pending_messages.len() > MAX_PENDING_PER_DEVICE {
                let excess = dev.pending_messages.len() - MAX_PENDING_PER_DEVICE;
                dev.pending_messages.drain(0..excess);
                dev.pending_count = dev.pending_messages.len();
            }
            if dev.sync_status == SyncStatus::Synced {
                dev.sync_status = SyncStatus::Pending;
            }
            true
        } else {
            false
        }
    }

    /// Detect conflicts between local and remote message sets.
    /// Returns the number of new conflicts detected.
    pub fn detect_conflicts(
        &mut self,
        local_msgs: &[SyncMessage],
        remote_msgs: &[SyncMessage],
    ) -> usize {
        let mut new_conflicts: usize = 0;
        for local in local_msgs {
            for remote in remote_msgs {
                if local.msg_id == remote.msg_id && local.content_hash != remote.content_hash {
                    // Already recorded?
                    let already = self
                        .conflicts
                        .iter()
                        .any(|c| c.msg_id == local.msg_id && !c.resolved);
                    if !already {
                        let conflict = SyncConflict {
                            msg_id: local.msg_id,
                            local_hash: local.content_hash,
                            remote_hash: remote.content_hash,
                            local_timestamp: local.timestamp,
                            remote_timestamp: remote.timestamp,
                            resolved: false,
                            winning_hash: 0,
                        };
                        self.conflicts.push(conflict);
                        self.next_conflict_id = self.next_conflict_id.saturating_add(1);
                        new_conflicts += 1;
                    }
                }
            }
        }
        if new_conflicts > 0 {
            serial_println!("[msg_sync] detected {} new conflict(s)", new_conflicts);
        }
        new_conflicts
    }

    /// Resolve a single conflict using the specified strategy.
    pub fn resolve_conflict(&mut self, msg_id: u64, strategy: ConflictStrategy) -> bool {
        if let Some(conflict) = self
            .conflicts
            .iter_mut()
            .find(|c| c.msg_id == msg_id && !c.resolved)
        {
            let winner = match strategy {
                ConflictStrategy::LastWriterWins => {
                    if conflict.local_timestamp >= conflict.remote_timestamp {
                        conflict.local_hash
                    } else {
                        conflict.remote_hash
                    }
                }
                ConflictStrategy::PreferLocal => conflict.local_hash,
                ConflictStrategy::PreferRemote => conflict.remote_hash,
                ConflictStrategy::Merge => {
                    // For merge we combine hashes deterministically.
                    conflict.local_hash ^ conflict.remote_hash
                }
            };
            conflict.winning_hash = winner;
            conflict.resolved = true;
            serial_println!(
                "[msg_sync] resolved conflict for msg {} -> winner hash {:#X}",
                msg_id,
                winner
            );
            true
        } else {
            false
        }
    }

    /// Resolve all outstanding conflicts using the default strategy.
    pub fn resolve_all_conflicts(&mut self) -> usize {
        let strategy = self.default_strategy;
        let unresolved: Vec<u64> = self
            .conflicts
            .iter()
            .filter(|c| !c.resolved)
            .map(|c| c.msg_id)
            .collect();
        let mut count: usize = 0;
        for msg_id in unresolved {
            if self.resolve_conflict(msg_id, strategy) {
                count += 1;
            }
        }
        count
    }

    /// Merge pending messages from one device's queue into a canonical list.
    /// Deduplicates by msg_id, keeping the version with the later timestamp.
    pub fn merge_messages(&self, canonical: &mut Vec<SyncMessage>, incoming: &[SyncMessage]) {
        for inc in incoming {
            let mut replaced = false;
            for existing in canonical.iter_mut() {
                if existing.msg_id == inc.msg_id {
                    // Keep the newer version.
                    if inc.timestamp > existing.timestamp {
                        *existing = inc.clone();
                    }
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                canonical.push(inc.clone());
            }
        }
        // Sort by timestamp ascending.
        canonical.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    }

    /// Get the current sync status for a device.
    pub fn get_sync_status(&self, device_hash: u64) -> Option<SyncStatus> {
        self.find_device(device_hash).map(|d| d.sync_status)
    }

    /// Mark a device as fully synced and clear its pending queue.
    pub fn mark_synced(&mut self, device_hash: u64, now: u64) -> bool {
        if let Some(dev) = self.find_device_mut(device_hash) {
            dev.sync_status = SyncStatus::Synced;
            dev.last_sync = now;
            dev.pending_messages.clear();
            dev.pending_count = 0;
            dev.session_start = 0;
            serial_println!("[msg_sync] device {:#X} marked synced", device_hash);
            true
        } else {
            false
        }
    }

    /// Get the number of pending messages for a device.
    pub fn pending_count(&self, device_hash: u64) -> usize {
        if let Some(dev) = self.find_device(device_hash) {
            dev.pending_count
        } else {
            0
        }
    }

    /// Total number of registered devices.
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Total unresolved conflicts.
    pub fn unresolved_conflict_count(&self) -> usize {
        self.conflicts.iter().filter(|c| !c.resolved).count()
    }

    /// Set the default conflict resolution strategy.
    pub fn set_default_strategy(&mut self, strategy: ConflictStrategy) {
        self.default_strategy = strategy;
    }
}

// ---------------------------------------------------------------------------
// Public API (through the global mutex)
// ---------------------------------------------------------------------------

pub fn register_device(device_hash: u64) -> bool {
    let mut guard = SYNC_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.register_device(device_hash)
    } else {
        false
    }
}

pub fn start_sync(device_hash: u64, now: u64) -> bool {
    let mut guard = SYNC_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.start_sync(device_hash, now)
    } else {
        false
    }
}

pub fn resolve_conflict(msg_id: u64, strategy: ConflictStrategy) -> bool {
    let mut guard = SYNC_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.resolve_conflict(msg_id, strategy)
    } else {
        false
    }
}

pub fn merge_messages(canonical: &mut Vec<SyncMessage>, incoming: &[SyncMessage]) {
    let guard = SYNC_MANAGER.lock();
    if let Some(mgr) = guard.as_ref() {
        mgr.merge_messages(canonical, incoming);
    }
}

pub fn get_sync_status(device_hash: u64) -> Option<SyncStatus> {
    let guard = SYNC_MANAGER.lock();
    guard.as_ref()?.get_sync_status(device_hash)
}

pub fn mark_synced(device_hash: u64, now: u64) -> bool {
    let mut guard = SYNC_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.mark_synced(device_hash, now)
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = SYNC_MANAGER.lock();
    *guard = Some(SyncManager::new());
    serial_println!("[msg_sync] initialised");
}
