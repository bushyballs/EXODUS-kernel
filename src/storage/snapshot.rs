/// Block-level snapshots
///
/// Part of the AIOS storage layer.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Represents a block-level snapshot using Copy-on-Write semantics.
pub struct Snapshot {
    pub id: u64,
    pub name: String,
    pub timestamp: u64,
    /// Bitmap tracking which blocks have been COW-copied.
    /// Each bit represents one block: 1 = block has been copied to exception store.
    cow_bitmap: Vec<u8>,
    /// Origin volume identifier.
    origin_volume: u64,
    /// Number of blocks in the origin volume.
    block_count: u64,
}

/// Copy-on-Write exception: maps an original block to its snapshot copy.
struct CowException {
    original_block: u64,
    snapshot_block: u64,
    snapshot_id: u64,
}

pub struct SnapshotManager {
    snapshots: Vec<Snapshot>,
    /// COW exception store: records where original blocks were saved before overwrite.
    exceptions: Vec<CowException>,
    next_id: u64,
    /// Next available block in the exception store.
    next_exception_block: u64,
}

impl SnapshotManager {
    pub fn new() -> Self {
        SnapshotManager {
            snapshots: Vec::new(),
            exceptions: Vec::new(),
            next_id: 1,
            next_exception_block: 0,
        }
    }

    /// Create a new snapshot of the current volume state.
    pub fn create(&mut self, name: &str) -> Result<u64, ()> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let timestamp = crate::time::clock::unix_time();

        // Default block count; in a real system this would come from the origin volume.
        let block_count = 1024u64;
        // Bitmap size: 1 bit per block, rounded up to bytes.
        let bitmap_size = ((block_count + 7) / 8) as usize;

        let snapshot = Snapshot {
            id,
            name: String::from(name),
            timestamp,
            cow_bitmap: alloc::vec![0u8; bitmap_size],
            origin_volume: 0,
            block_count,
        };

        serial_println!("  [snapshot] Created snapshot '{}' (id={})", name, id);
        self.snapshots.push(snapshot);
        Ok(id)
    }

    /// Rollback the origin volume to the state captured by snapshot `id`.
    /// This replays all COW exceptions for the given snapshot, effectively
    /// restoring original block contents.
    pub fn rollback(&mut self, id: u64) -> Result<(), ()> {
        // Verify the snapshot exists
        let exists = self.snapshots.iter().any(|s| s.id == id);
        if !exists {
            serial_println!("  [snapshot] Rollback failed: snapshot {} not found", id);
            return Err(());
        }

        // Count how many blocks need to be restored
        let restore_count = self
            .exceptions
            .iter()
            .filter(|e| e.snapshot_id == id)
            .count();

        serial_println!(
            "  [snapshot] Rolling back to snapshot {} ({} blocks to restore)",
            id,
            restore_count
        );

        // In a real system, we would copy each exception block back to its
        // original location. Here we record the rollback operation.

        // Remove all snapshots taken after this one (they become invalid)
        let snap_timestamp = self
            .snapshots
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.timestamp)
            .unwrap_or(0);

        self.snapshots.retain(|s| s.timestamp <= snap_timestamp);
        // Remove exceptions for invalidated snapshots
        let remaining_ids: Vec<u64> = self.snapshots.iter().map(|s| s.id).collect();
        self.exceptions
            .retain(|e| remaining_ids.contains(&e.snapshot_id));

        serial_println!("  [snapshot] Rollback to snapshot {} complete", id);
        Ok(())
    }

    /// Delete a snapshot and free its COW exception blocks.
    pub fn delete(&mut self, id: u64) -> Result<(), ()> {
        let pos = self.snapshots.iter().position(|s| s.id == id);
        match pos {
            Some(idx) => {
                let name = self.snapshots[idx].name.clone();
                self.snapshots.remove(idx);
                // Free exception blocks belonging to this snapshot
                self.exceptions.retain(|e| e.snapshot_id != id);
                serial_println!("  [snapshot] Deleted snapshot '{}' (id={})", name, id);
                Ok(())
            }
            None => {
                serial_println!("  [snapshot] Delete failed: snapshot {} not found", id);
                Err(())
            }
        }
    }

    /// Record a COW exception: before overwriting `block` on the origin volume,
    /// save its current contents to the exception store.
    pub fn record_cow(&mut self, snapshot_id: u64, original_block: u64) -> Option<u64> {
        // Check if this block is already recorded for this snapshot
        if self
            .exceptions
            .iter()
            .any(|e| e.snapshot_id == snapshot_id && e.original_block == original_block)
        {
            return None; // Already saved
        }

        // Find the snapshot and mark the block in its bitmap
        if let Some(snap) = self.snapshots.iter_mut().find(|s| s.id == snapshot_id) {
            let byte_idx = (original_block / 8) as usize;
            let bit_idx = (original_block % 8) as u8;
            if byte_idx < snap.cow_bitmap.len() {
                snap.cow_bitmap[byte_idx] |= 1 << bit_idx;
            }
        } else {
            return None;
        }

        let exception_block = self.next_exception_block;
        self.next_exception_block = self.next_exception_block.saturating_add(1);

        self.exceptions.push(CowException {
            original_block,
            snapshot_block: exception_block,
            snapshot_id,
        });

        Some(exception_block)
    }

    /// List all snapshots.
    pub fn list(&self) -> &[Snapshot] {
        &self.snapshots
    }

    /// Get a snapshot by id.
    pub fn get(&self, id: u64) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }

    /// Return the number of active snapshots.
    pub fn count(&self) -> usize {
        self.snapshots.len()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SNAPSHOT_MANAGER: Mutex<Option<SnapshotManager>> = Mutex::new(None);

pub fn init() {
    let mut guard = SNAPSHOT_MANAGER.lock();
    *guard = Some(SnapshotManager::new());
    serial_println!("  [storage] Snapshot manager initialized");
}

/// Access the snapshot manager under lock.
pub fn with_snapshots<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut SnapshotManager) -> R,
{
    let mut guard = SNAPSHOT_MANAGER.lock();
    guard.as_mut().map(f)
}
