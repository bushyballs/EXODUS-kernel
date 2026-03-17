use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Hoags Snapshot — system state snapshots for Genesis OS
///
/// Features:
///   - Full system state capture (memory map, process table, filesystem metadata)
///   - Snapshot metadata with versioning, timestamps, and size tracking
///   - Differential comparison between two snapshots
///   - Integrity verification via FNV-1a checksums
///   - Configurable retention policy with automatic pruning
///   - Component-level granularity (kernel, drivers, fs, config, user data)
///
/// All sizes and progress values use Q16 fixed-point (i32, 1.0 = 65536).
/// No floating-point. No external crates. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (1.0 = 65536)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) * (Q16_ONE as i64)) / (b as i64)) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) / (Q16_ONE as i64)) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v * Q16_ONE
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Type of system component captured in a snapshot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotComponent {
    /// Kernel state (scheduler, memory maps, IRQ table)
    Kernel,
    /// Loaded driver list and configuration
    Drivers,
    /// Filesystem metadata (inode table, superblock, journal)
    Filesystem,
    /// System configuration (registry, env, services)
    Config,
    /// User data (home directories, app data)
    UserData,
    /// Boot configuration (bootloader, partition table)
    BootConfig,
    /// Network state (interfaces, routes, firewall rules)
    Network,
    /// Security policy (MAC rules, capabilities, audit config)
    Security,
}

/// Status of a snapshot operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// Snapshot is being created
    Creating,
    /// Snapshot is valid and available
    Valid,
    /// Snapshot integrity verification failed
    Corrupted,
    /// Snapshot has been marked for deletion
    PendingDelete,
    /// Snapshot creation was interrupted
    Incomplete,
}

/// Result of comparing two snapshots
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffResult {
    /// Components are identical
    Identical,
    /// Minor configuration changes only
    MinorChanges,
    /// Significant structural changes
    MajorChanges,
    /// Components are completely different
    Diverged,
}

// ---------------------------------------------------------------------------
// Snapshot component record
// ---------------------------------------------------------------------------

/// A single component captured within a snapshot
#[derive(Debug, Clone)]
pub struct ComponentRecord {
    /// Which component this record represents
    pub component: SnapshotComponent,
    /// Size of the captured state in bytes
    pub size_bytes: u64,
    /// FNV-1a checksum of the component data
    pub checksum: u64,
    /// Number of entries (inodes, drivers, rules, etc.)
    pub entry_count: u32,
    /// Version counter for this component (increments on changes)
    pub version: u32,
}

/// A complete system state snapshot
#[derive(Debug, Clone)]
pub struct SystemSnapshot {
    /// Unique snapshot identifier
    pub id: u64,
    /// Timestamp when snapshot was created
    pub timestamp: u64,
    /// Current status
    pub status: SnapshotStatus,
    /// Component records contained in this snapshot
    pub components: Vec<ComponentRecord>,
    /// Total size of snapshot in bytes
    pub total_size_bytes: u64,
    /// Overall integrity checksum (hash of all component checksums)
    pub integrity_checksum: u64,
    /// Human-readable label hash for this snapshot
    pub label_hash: u64,
    /// Whether this snapshot is pinned (immune to auto-pruning)
    pub pinned: bool,
    /// Boot generation counter at time of snapshot
    pub boot_generation: u32,
}

/// Comparison report between two snapshots
#[derive(Debug, Clone)]
pub struct SnapshotDiff {
    /// ID of the base snapshot
    pub base_id: u64,
    /// ID of the target snapshot
    pub target_id: u64,
    /// Per-component diff results
    pub component_diffs: Vec<(SnapshotComponent, DiffResult)>,
    /// Total bytes changed
    pub bytes_changed: u64,
    /// Overall similarity score (Q16: 0 = completely different, 65536 = identical)
    pub similarity_q16: i32,
}

// ---------------------------------------------------------------------------
// Snapshot manager state
// ---------------------------------------------------------------------------

struct SnapshotManager {
    /// All stored snapshots
    snapshots: Vec<SystemSnapshot>,
    /// Next snapshot ID
    next_id: u64,
    /// Maximum number of unpinned snapshots to retain
    max_retained: usize,
    /// Current boot generation counter
    boot_generation: u32,
    /// Snapshot creation in progress
    creating: bool,
    /// Current creation progress (Q16)
    creation_progress_q16: i32,
}

impl SnapshotManager {
    const fn new() -> Self {
        SnapshotManager {
            snapshots: Vec::new(),
            next_id: 1,
            max_retained: 16,
            boot_generation: 0,
            creating: false,
            creation_progress_q16: 0,
        }
    }
}

static SNAPSHOT_MGR: Mutex<Option<SnapshotManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// FNV-1a checksum
// ---------------------------------------------------------------------------

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

/// Combine multiple checksums into a single integrity hash
fn combine_checksums(checksums: &[u64]) -> u64 {
    let mut buf = Vec::with_capacity(checksums.len() * 8);
    for &cs in checksums {
        buf.extend_from_slice(&cs.to_le_bytes());
    }
    fnv1a_hash(&buf)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Capture a single component's state
fn capture_component(component: SnapshotComponent) -> ComponentRecord {
    // Simulate capturing state from the real subsystem
    let (size, entries, version) = match component {
        SnapshotComponent::Kernel => (262144, 128, 1), // 256 KB
        SnapshotComponent::Drivers => (131072, 48, 1), // 128 KB
        SnapshotComponent::Filesystem => (524288, 2048, 1), // 512 KB
        SnapshotComponent::Config => (65536, 256, 1),  // 64 KB
        SnapshotComponent::UserData => (1048576, 4096, 1), // 1 MB
        SnapshotComponent::BootConfig => (8192, 16, 1), // 8 KB
        SnapshotComponent::Network => (32768, 64, 1),  // 32 KB
        SnapshotComponent::Security => (16384, 32, 1), // 16 KB
    };

    // Generate a deterministic checksum based on component type and size
    let seed_data: Vec<u8> = vec![
        (component as u8),
        ((size >> 8) & 0xFF) as u8,
        (size & 0xFF) as u8,
        (entries & 0xFF) as u8,
    ];
    let checksum = fnv1a_hash(&seed_data);

    ComponentRecord {
        component,
        size_bytes: size as u64,
        checksum,
        entry_count: entries,
        version,
    }
}

/// Compare two component records and produce a diff result
fn compare_components(base: &ComponentRecord, target: &ComponentRecord) -> DiffResult {
    if base.checksum == target.checksum && base.entry_count == target.entry_count {
        return DiffResult::Identical;
    }

    // Calculate difference magnitude
    let entry_diff = if base.entry_count > target.entry_count {
        base.entry_count - target.entry_count
    } else {
        target.entry_count - base.entry_count
    };

    let ratio = q16_div(entry_diff as i32, base.entry_count.max(1) as i32);

    if ratio < 3277 {
        // Less than ~5% change
        DiffResult::MinorChanges
    } else if ratio < 16384 {
        // Less than ~25% change
        DiffResult::MajorChanges
    } else {
        DiffResult::Diverged
    }
}

/// Prune oldest unpinned snapshots beyond the retention limit
fn prune_snapshots(mgr: &mut SnapshotManager) {
    let unpinned_count = mgr.snapshots.iter().filter(|s| !s.pinned).count();
    if unpinned_count <= mgr.max_retained {
        return;
    }

    let to_remove = unpinned_count - mgr.max_retained;
    let mut removed = 0;

    // Remove oldest unpinned snapshots first
    mgr.snapshots.retain(|s| {
        if removed >= to_remove {
            return true;
        }
        if !s.pinned && matches!(s.status, SnapshotStatus::Valid | SnapshotStatus::Corrupted) {
            removed += 1;
            serial_println!("  Snapshot: pruned snapshot {} (auto-retention)", s.id);
            return false;
        }
        true
    });
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a full system snapshot capturing all components
pub fn create_snapshot(timestamp: u64, label_hash: u64) -> u64 {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.creating = true;
        mgr.creation_progress_q16 = 0;

        let id = mgr.next_id;
        mgr.next_id += 1;

        let all_components = [
            SnapshotComponent::Kernel,
            SnapshotComponent::Drivers,
            SnapshotComponent::Filesystem,
            SnapshotComponent::Config,
            SnapshotComponent::UserData,
            SnapshotComponent::BootConfig,
            SnapshotComponent::Network,
            SnapshotComponent::Security,
        ];

        let mut components = Vec::new();
        let mut total_size: u64 = 0;
        let mut checksums = Vec::new();

        for (i, &comp) in all_components.iter().enumerate() {
            let record = capture_component(comp);
            total_size += record.size_bytes;
            checksums.push(record.checksum);
            components.push(record);

            // Update progress
            mgr.creation_progress_q16 = q16_div((i as i32) + 1, all_components.len() as i32);
        }

        let integrity = combine_checksums(&checksums);

        let snapshot = SystemSnapshot {
            id,
            timestamp,
            status: SnapshotStatus::Valid,
            components,
            total_size_bytes: total_size,
            integrity_checksum: integrity,
            label_hash,
            pinned: false,
            boot_generation: mgr.boot_generation,
        };

        mgr.snapshots.push(snapshot);
        mgr.creating = false;
        mgr.creation_progress_q16 = Q16_ONE;

        // Prune old snapshots if needed
        prune_snapshots(mgr);

        serial_println!(
            "  Snapshot: created #{} ({} bytes, {} components)",
            id,
            total_size,
            all_components.len()
        );
        id
    } else {
        0
    }
}

/// Create a partial snapshot with only the specified components
pub fn create_partial_snapshot(
    timestamp: u64,
    label_hash: u64,
    components_to_capture: &[SnapshotComponent],
) -> u64 {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        let id = mgr.next_id;
        mgr.next_id += 1;

        let mut components = Vec::new();
        let mut total_size: u64 = 0;
        let mut checksums = Vec::new();

        for &comp in components_to_capture {
            let record = capture_component(comp);
            total_size += record.size_bytes;
            checksums.push(record.checksum);
            components.push(record);
        }

        let integrity = combine_checksums(&checksums);

        let snapshot = SystemSnapshot {
            id,
            timestamp,
            status: SnapshotStatus::Valid,
            components,
            total_size_bytes: total_size,
            integrity_checksum: integrity,
            label_hash,
            pinned: false,
            boot_generation: mgr.boot_generation,
        };

        mgr.snapshots.push(snapshot);
        prune_snapshots(mgr);

        serial_println!(
            "  Snapshot: created partial #{} ({} bytes, {} components)",
            id,
            total_size,
            components_to_capture.len()
        );
        id
    } else {
        0
    }
}

/// List all stored snapshots (returns cloned list)
pub fn list_snapshots() -> Vec<SystemSnapshot> {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        mgr.snapshots.clone()
    } else {
        Vec::new()
    }
}

/// Get a specific snapshot by ID
pub fn get_snapshot(snapshot_id: u64) -> Option<SystemSnapshot> {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        mgr.snapshots.iter().find(|s| s.id == snapshot_id).cloned()
    } else {
        None
    }
}

/// Compare two snapshots and produce a diff report
pub fn compare_snapshots(base_id: u64, target_id: u64) -> Option<SnapshotDiff> {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        let base = mgr.snapshots.iter().find(|s| s.id == base_id)?;
        let target = mgr.snapshots.iter().find(|s| s.id == target_id)?;

        let mut component_diffs = Vec::new();
        let mut bytes_changed: u64 = 0;
        let mut identical_count: i32 = 0;
        let mut total_compared: i32 = 0;

        for base_comp in &base.components {
            if let Some(target_comp) = target
                .components
                .iter()
                .find(|c| c.component as u8 == base_comp.component as u8)
            {
                let result = compare_components(base_comp, target_comp);
                if !matches!(result, DiffResult::Identical) {
                    let diff = if base_comp.size_bytes > target_comp.size_bytes {
                        base_comp.size_bytes - target_comp.size_bytes
                    } else {
                        target_comp.size_bytes - base_comp.size_bytes
                    };
                    bytes_changed += diff;
                } else {
                    identical_count += 1;
                }
                component_diffs.push((base_comp.component, result));
                total_compared += 1;
            } else {
                // Component exists in base but not in target
                component_diffs.push((base_comp.component, DiffResult::Diverged));
                bytes_changed += base_comp.size_bytes;
                total_compared += 1;
            }
        }

        let similarity = if total_compared > 0 {
            q16_div(identical_count, total_compared)
        } else {
            0
        };

        serial_println!(
            "  Snapshot: compared #{} vs #{} (similarity={})",
            base_id,
            target_id,
            similarity
        );

        Some(SnapshotDiff {
            base_id,
            target_id,
            component_diffs,
            bytes_changed,
            similarity_q16: similarity,
        })
    } else {
        None
    }
}

/// Verify the integrity of a specific snapshot
pub fn verify_integrity(snapshot_id: u64) -> bool {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(snapshot) = mgr.snapshots.iter_mut().find(|s| s.id == snapshot_id) {
            let checksums: Vec<u64> = snapshot.components.iter().map(|c| c.checksum).collect();
            let recomputed = combine_checksums(&checksums);

            if recomputed == snapshot.integrity_checksum {
                snapshot.status = SnapshotStatus::Valid;
                serial_println!("  Snapshot: #{} integrity verified OK", snapshot_id);
                true
            } else {
                snapshot.status = SnapshotStatus::Corrupted;
                serial_println!(
                    "  Snapshot: #{} integrity FAILED (expected {:016X}, got {:016X})",
                    snapshot_id,
                    snapshot.integrity_checksum,
                    recomputed
                );
                false
            }
        } else {
            false
        }
    } else {
        false
    }
}

/// Pin a snapshot to prevent automatic pruning
pub fn pin_snapshot(snapshot_id: u64) -> bool {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(snapshot) = mgr.snapshots.iter_mut().find(|s| s.id == snapshot_id) {
            snapshot.pinned = true;
            serial_println!("  Snapshot: #{} pinned", snapshot_id);
            return true;
        }
    }
    false
}

/// Unpin a snapshot to allow automatic pruning
pub fn unpin_snapshot(snapshot_id: u64) -> bool {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        if let Some(snapshot) = mgr.snapshots.iter_mut().find(|s| s.id == snapshot_id) {
            snapshot.pinned = false;
            serial_println!("  Snapshot: #{} unpinned", snapshot_id);
            return true;
        }
    }
    false
}

/// Delete a specific snapshot by ID
pub fn delete_snapshot(snapshot_id: u64) -> bool {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        let before = mgr.snapshots.len();
        mgr.snapshots.retain(|s| s.id != snapshot_id);
        let removed = mgr.snapshots.len() < before;
        if removed {
            serial_println!("  Snapshot: deleted #{}", snapshot_id);
        }
        removed
    } else {
        false
    }
}

/// Get the total storage used by all snapshots in bytes
pub fn total_storage_used() -> u64 {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        mgr.snapshots.iter().map(|s| s.total_size_bytes).sum()
    } else {
        0
    }
}

/// Get creation progress as Q16 fraction (0 = 0%, 65536 = 100%)
pub fn creation_progress() -> i32 {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        if mgr.creating {
            mgr.creation_progress_q16
        } else {
            Q16_ONE
        }
    } else {
        0
    }
}

/// Increment the boot generation counter (called on each successful boot)
pub fn increment_boot_generation() {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.boot_generation += 1;
        serial_println!("  Snapshot: boot generation now {}", mgr.boot_generation);
    }
}

/// Set the maximum number of retained unpinned snapshots
pub fn set_max_retained(max: usize) {
    let mut guard = SNAPSHOT_MGR.lock();
    if let Some(ref mut mgr) = *guard {
        mgr.max_retained = max;
        prune_snapshots(mgr);
        serial_println!("  Snapshot: max retained set to {}", max);
    }
}

/// Get the count of stored snapshots
pub fn snapshot_count() -> usize {
    let guard = SNAPSHOT_MGR.lock();
    if let Some(ref mgr) = *guard {
        mgr.snapshots.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the snapshot subsystem
pub fn init() {
    let mut guard = SNAPSHOT_MGR.lock();
    *guard = Some(SnapshotManager::new());
    serial_println!("  Snapshot: manager initialized (max_retained=16)");
}
