use crate::sync::Mutex;
/// Preference sync — import, export, cloud backup, migration, versioning
///
/// Handles serialization of preference stores into a portable format,
/// schema version migration between OS releases, and backup snapshots
/// for cloud or local recovery. All data is represented as byte arrays
/// and simple integers (no floats).
///
/// Versioning uses a semantic triple: (major, minor, patch) as u16.
/// Migration rules transform preferences from one version to the next.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Semantic version for preference schemas
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrefVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl PrefVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Check if this version is compatible with another (same major)
    pub fn compatible_with(&self, other: &PrefVersion) -> bool {
        self.major == other.major
    }

    /// Pack into a single u64 for comparison
    pub fn as_u64(&self) -> u64 {
        ((self.major as u64) << 32) | ((self.minor as u64) << 16) | (self.patch as u64)
    }

    /// Format as "major.minor.patch" string
    pub fn to_string(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Current preferences schema version
pub const CURRENT_VERSION: PrefVersion = PrefVersion::new(1, 0, 0);

/// A serialized preference entry for export
#[derive(Clone, Debug)]
pub struct ExportEntry {
    pub key: String,
    pub serialized_value: String,
    pub user_set: bool,
}

/// A full preference export bundle
#[derive(Clone, Debug)]
pub struct ExportBundle {
    /// Schema version of the exporting system
    pub version: PrefVersion,
    /// Timestamp (monotonic tick) of the export
    pub timestamp: u64,
    /// Human-readable label for this bundle
    pub label: String,
    /// All exported preference entries
    pub entries: Vec<ExportEntry>,
    /// Checksum of all entries (simple hash for integrity)
    pub checksum: u32,
}

impl ExportBundle {
    /// Compute a simple checksum over all entry keys and values
    fn compute_checksum(entries: &[ExportEntry]) -> u32 {
        let mut hash: u32 = 0x811C_9DC5; // FNV-1a offset basis
        for entry in entries {
            for b in entry.key.as_bytes() {
                hash ^= *b as u32;
                hash = hash.wrapping_mul(0x0100_0193); // FNV prime
            }
            for b in entry.serialized_value.as_bytes() {
                hash ^= *b as u32;
                hash = hash.wrapping_mul(0x0100_0193);
            }
        }
        hash
    }

    /// Verify the bundle's integrity
    pub fn verify(&self) -> bool {
        let computed = Self::compute_checksum(&self.entries);
        computed == self.checksum
    }
}

/// A migration rule — transforms preferences from one version to the next
#[derive(Clone, Debug)]
pub struct MigrationRule {
    /// Source version this rule migrates from
    pub from: PrefVersion,
    /// Target version this rule migrates to
    pub to: PrefVersion,
    /// Description of what this migration does
    pub description: String,
    /// Keys to rename: (old_key, new_key)
    pub renames: Vec<(String, String)>,
    /// Keys to remove
    pub removals: Vec<String>,
    /// Keys to add with default values: (key, serialized_value)
    pub additions: Vec<(String, String)>,
    /// Keys whose value type changes: (key, new_serialized_default)
    pub type_changes: Vec<(String, String)>,
}

impl MigrationRule {
    pub fn new(from: PrefVersion, to: PrefVersion, description: &str) -> Self {
        Self {
            from,
            to,
            description: String::from(description),
            renames: vec![],
            removals: vec![],
            additions: vec![],
            type_changes: vec![],
        }
    }

    /// Builder: add a key rename
    pub fn rename(mut self, old_key: &str, new_key: &str) -> Self {
        self.renames
            .push((String::from(old_key), String::from(new_key)));
        self
    }

    /// Builder: add a key removal
    pub fn remove(mut self, key: &str) -> Self {
        self.removals.push(String::from(key));
        self
    }

    /// Builder: add a new key with default
    pub fn add(mut self, key: &str, serialized_value: &str) -> Self {
        self.additions
            .push((String::from(key), String::from(serialized_value)));
        self
    }

    /// Builder: add a type change
    pub fn change_type(mut self, key: &str, new_default: &str) -> Self {
        self.type_changes
            .push((String::from(key), String::from(new_default)));
        self
    }

    /// Apply this migration to a set of export entries
    pub fn apply(&self, entries: &mut Vec<ExportEntry>) -> u32 {
        let mut changes = 0u32;

        // Apply renames
        for (old_key, new_key) in &self.renames {
            if let Some(entry) = entries.iter_mut().find(|e| e.key == *old_key) {
                serial_println!("[MIGRATE] Rename '{}' -> '{}'", old_key, new_key);
                entry.key = new_key.clone();
                changes += 1;
            }
        }

        // Apply removals
        let before = entries.len();
        entries.retain(|e| !self.removals.iter().any(|r| r == &e.key));
        let removed = (before - entries.len()) as u32;
        if removed > 0 {
            serial_println!("[MIGRATE] Removed {} deprecated keys", removed);
            changes += removed;
        }

        // Apply additions (only if key doesn't already exist)
        for (key, value) in &self.additions {
            if !entries.iter().any(|e| e.key == *key) {
                entries.push(ExportEntry {
                    key: key.clone(),
                    serialized_value: value.clone(),
                    user_set: false,
                });
                changes += 1;
                serial_println!("[MIGRATE] Added new key '{}'", key);
            }
        }

        // Apply type changes (replace value with new default if type differs)
        for (key, new_default) in &self.type_changes {
            if let Some(entry) = entries.iter_mut().find(|e| e.key == *key) {
                // Check if the existing value's type prefix differs from new default
                let old_prefix = entry.serialized_value.split(':').next().unwrap_or("");
                let new_prefix = new_default.split(':').next().unwrap_or("");
                if old_prefix != new_prefix {
                    serial_println!(
                        "[MIGRATE] Type change for '{}': {} -> {}",
                        key,
                        old_prefix,
                        new_prefix
                    );
                    entry.serialized_value = new_default.clone();
                    entry.user_set = false;
                    changes += 1;
                }
            }
        }

        changes
    }
}

/// A cloud backup snapshot
#[derive(Clone, Debug)]
pub struct BackupSnapshot {
    /// Unique snapshot ID
    pub id: u32,
    /// Version at time of backup
    pub version: PrefVersion,
    /// Timestamp of the backup
    pub timestamp: u64,
    /// Size in (simulated) bytes
    pub size_bytes: u32,
    /// Label for this snapshot
    pub label: String,
    /// The serialized bundle data (entries as key=value lines)
    pub data: Vec<(String, String)>,
    /// Checksum of the data
    pub checksum: u32,
}

/// The preference sync manager
pub struct PrefSyncManager {
    /// Migration rules ordered by source version
    migrations: Vec<MigrationRule>,
    /// Cloud backup snapshots
    backups: Vec<BackupSnapshot>,
    /// Next backup snapshot ID
    next_backup_id: u32,
    /// Current schema version
    current_version: PrefVersion,
    /// Total exports performed
    total_exports: u64,
    /// Total imports performed
    total_imports: u64,
    /// Total migrations applied
    total_migrations: u64,
    /// Total backups created
    total_backups: u64,
    /// Maximum number of backups to retain
    max_backups: usize,
}

impl PrefSyncManager {
    pub fn new() -> Self {
        Self {
            migrations: vec![],
            backups: vec![],
            next_backup_id: 1,
            current_version: CURRENT_VERSION,
            total_exports: 0,
            total_imports: 0,
            total_migrations: 0,
            total_backups: 0,
            max_backups: 32,
        }
    }

    /// Register a migration rule
    pub fn register_migration(&mut self, rule: MigrationRule) {
        serial_println!(
            "[SYNC] Registered migration {} -> {}: {}",
            rule.from.to_string(),
            rule.to.to_string(),
            rule.description
        );
        self.migrations.push(rule);
        // Keep sorted by source version
        self.migrations
            .sort_by(|a, b| a.from.as_u64().cmp(&b.from.as_u64()));
    }

    /// Export all preferences into a bundle
    pub fn export(
        &mut self,
        entries: &[(String, String)],
        user_flags: &BTreeMap<String, bool>,
        label: &str,
        timestamp: u64,
    ) -> ExportBundle {
        let export_entries: Vec<ExportEntry> = entries
            .iter()
            .map(|(k, v)| ExportEntry {
                key: k.clone(),
                serialized_value: v.clone(),
                user_set: user_flags.get(k).copied().unwrap_or(false),
            })
            .collect();

        let checksum = ExportBundle::compute_checksum(&export_entries);

        self.total_exports = self.total_exports.saturating_add(1);
        serial_println!(
            "[SYNC] Exported {} preferences (v{}, checksum 0x{:08X})",
            export_entries.len(),
            self.current_version.to_string(),
            checksum
        );

        ExportBundle {
            version: self.current_version,
            timestamp,
            label: String::from(label),
            entries: export_entries,
            checksum,
        }
    }

    /// Import preferences from a bundle, applying migrations if needed
    pub fn import(&mut self, bundle: &ExportBundle) -> Result<Vec<(String, String, bool)>, String> {
        // Verify integrity
        if !bundle.verify() {
            return Err(String::from("Bundle checksum verification failed"));
        }

        // Check version compatibility
        if !bundle.version.compatible_with(&self.current_version) {
            return Err(format!(
                "Incompatible major version: bundle={}, current={}",
                bundle.version.to_string(),
                self.current_version.to_string()
            ));
        }

        let mut entries = bundle.entries.clone();

        // Apply migrations if bundle version < current
        if bundle.version.as_u64() < self.current_version.as_u64() {
            let mut current = bundle.version;
            for rule in &self.migrations {
                if rule.from == current && rule.to.as_u64() <= self.current_version.as_u64() {
                    let changes = rule.apply(&mut entries);
                    serial_println!(
                        "[SYNC] Applied migration {} -> {}: {} changes",
                        rule.from.to_string(),
                        rule.to.to_string(),
                        changes
                    );
                    current = rule.to;
                    self.total_migrations = self.total_migrations.saturating_add(1);
                }
            }
        }

        self.total_imports = self.total_imports.saturating_add(1);

        // Return as (key, serialized_value, user_set) tuples
        let result: Vec<(String, String, bool)> = entries
            .iter()
            .map(|e| (e.key.clone(), e.serialized_value.clone(), e.user_set))
            .collect();

        serial_println!("[SYNC] Imported {} preferences from bundle", result.len());
        Ok(result)
    }

    /// Create a cloud backup snapshot
    pub fn create_backup(
        &mut self,
        entries: &[(String, String)],
        label: &str,
        timestamp: u64,
    ) -> u32 {
        let id = self.next_backup_id;
        self.next_backup_id = self.next_backup_id.saturating_add(1);

        // Compute size estimate (sum of key + value bytes)
        let size_bytes: u32 = entries
            .iter()
            .map(|(k, v)| (k.len() + v.len() + 2) as u32)
            .sum();

        // Compute checksum
        let mut hash: u32 = 0x811C_9DC5;
        for (k, v) in entries {
            for b in k.as_bytes() {
                hash ^= *b as u32;
                hash = hash.wrapping_mul(0x0100_0193);
            }
            for b in v.as_bytes() {
                hash ^= *b as u32;
                hash = hash.wrapping_mul(0x0100_0193);
            }
        }

        let data: Vec<(String, String)> = entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        self.backups.push(BackupSnapshot {
            id,
            version: self.current_version,
            timestamp,
            size_bytes,
            label: String::from(label),
            data,
            checksum: hash,
        });

        self.total_backups = self.total_backups.saturating_add(1);

        // Evict oldest backups if over limit
        while self.backups.len() > self.max_backups {
            let evicted = self.backups.remove(0);
            serial_println!("[SYNC] Evicted old backup #{}", evicted.id);
        }

        serial_println!(
            "[SYNC] Created backup #{} '{}' ({} bytes, {} entries)",
            id,
            label,
            size_bytes,
            entries.len()
        );

        id
    }

    /// Restore preferences from a backup snapshot
    pub fn restore_backup(&self, backup_id: u32) -> Option<Vec<(String, String)>> {
        self.backups.iter().find(|b| b.id == backup_id).map(|b| {
            serial_println!(
                "[SYNC] Restoring backup #{} '{}' ({} entries)",
                b.id,
                b.label,
                b.data.len()
            );
            b.data.clone()
        })
    }

    /// List all backup snapshots
    pub fn list_backups(&self) -> Vec<(u32, String, u64, u32)> {
        self.backups
            .iter()
            .map(|b| (b.id, b.label.clone(), b.timestamp, b.size_bytes))
            .collect()
    }

    /// Delete a specific backup
    pub fn delete_backup(&mut self, backup_id: u32) -> bool {
        if let Some(pos) = self.backups.iter().position(|b| b.id == backup_id) {
            self.backups.remove(pos);
            serial_println!("[SYNC] Deleted backup #{}", backup_id);
            true
        } else {
            false
        }
    }

    /// Get the current schema version
    pub fn version(&self) -> PrefVersion {
        self.current_version
    }

    /// Get sync statistics
    pub fn stats(&self) -> (u64, u64, u64, u64, usize) {
        (
            self.total_exports,
            self.total_imports,
            self.total_migrations,
            self.total_backups,
            self.backups.len(),
        )
    }

    /// Set maximum backup retention count
    pub fn set_max_backups(&mut self, max: usize) {
        self.max_backups = max;
    }

    /// Get total stored backup size in bytes
    pub fn total_backup_size(&self) -> u64 {
        self.backups.iter().map(|b| b.size_bytes as u64).sum()
    }
}

static SYNC_MGR: Mutex<Option<PrefSyncManager>> = Mutex::new(None);

/// Initialize the preference sync manager
pub fn init() {
    let mut lock = SYNC_MGR.lock();
    *lock = Some(PrefSyncManager::new());
    serial_println!(
        "[SYNC] Preference sync manager initialized (v{})",
        CURRENT_VERSION.to_string()
    );
}

/// Get a reference to the global sync manager
pub fn get_manager() -> &'static Mutex<Option<PrefSyncManager>> {
    &SYNC_MGR
}
