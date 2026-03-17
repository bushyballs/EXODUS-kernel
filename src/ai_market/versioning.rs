/// AI Model Version Management for Genesis
///
/// Tracks installed model versions, manages update policies, supports
/// rollback, version pinning, and changelog tracking. Ensures users
/// always have control over which model version is active.
///
/// Version comparisons use semantic-style u32 encoding:
///   major.minor.patch packed as (major << 20) | (minor << 10) | patch
///
/// Original implementation for Hoags OS.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 helpers ────────────────────────────────────────────────────────────

pub type Q16 = i32;

fn q16_from_int(v: i32) -> Q16 {
    v << 16
}

fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

// ── Version encoding ───────────────────────────────────────────────────────

/// Pack major.minor.patch into a single u32.
/// Layout: bits [31..20] = major, [19..10] = minor, [9..0] = patch
pub fn pack_version(major: u32, minor: u32, patch: u32) -> u32 {
    ((major & 0xFFF) << 20) | ((minor & 0x3FF) << 10) | (patch & 0x3FF)
}

/// Unpack a version u32 into (major, minor, patch).
pub fn unpack_version(v: u32) -> (u32, u32, u32) {
    let major = (v >> 20) & 0xFFF;
    let minor = (v >> 10) & 0x3FF;
    let patch = v & 0x3FF;
    (major, minor, patch)
}

// ── Enums ──────────────────────────────────────────────────────────────────

/// Update policy for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPolicy {
    /// Automatically download and install updates.
    AutoUpdate,
    /// Notify the user but do not auto-install.
    NotifyOnly,
    /// User must manually check and install updates.
    ManualOnly,
    /// Version is locked; never update.
    Pinned,
}

/// Result of a version comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionComparison {
    Newer,
    Same,
    Older,
    Incompatible,
}

/// Update availability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    UpdateAvailable,
    MajorUpdateAvailable,
    SecurityUpdate,
    Deprecated,
    Unknown,
}

// ── Core structs ───────────────────────────────────────────────────────────

/// A specific version of a model.
#[derive(Clone)]
pub struct ModelVersion {
    pub model_id: u32,
    pub version: u32,
    pub changelog_hash: u64,
    pub release_date: u64,
    pub size_bytes: u64,
    pub min_ram_mb: u64,
    pub compatible_hw: Vec<u64>,
    pub is_security_fix: bool,
    pub is_deprecated: bool,
    pub parent_version: u32,
}

/// Tracks the currently installed version for a model.
#[derive(Clone)]
pub struct InstalledVersion {
    pub model_id: u32,
    pub installed_version: u32,
    pub policy: VersionPolicy,
    pub pinned_version: Option<u32>,
    pub install_date: u64,
    pub last_checked: u64,
    pub auto_rollback: bool,
}

/// An available update for an installed model.
#[derive(Clone)]
pub struct AvailableUpdate {
    pub model_id: u32,
    pub current_version: u32,
    pub latest_version: u32,
    pub size_bytes: u64,
    pub status: UpdateStatus,
    pub changelog_hash: u64,
    pub is_security: bool,
}

/// Rollback history entry.
#[derive(Clone)]
pub struct RollbackEntry {
    pub model_id: u32,
    pub from_version: u32,
    pub to_version: u32,
    pub timestamp: u64,
    pub reason_hash: u64,
}

// ── Global state ───────────────────────────────────────────────────────────

static VERSION_MANAGER: Mutex<Option<VersionManager>> = Mutex::new(None);

struct VersionManager {
    /// All known versions across all models.
    versions: Vec<ModelVersion>,
    /// Currently installed model versions.
    installed: Vec<InstalledVersion>,
    /// Rollback history log.
    rollback_log: Vec<RollbackEntry>,
    /// Default policy for new models.
    default_policy: VersionPolicy,
}

impl VersionManager {
    fn new() -> Self {
        VersionManager {
            versions: Vec::new(),
            installed: Vec::new(),
            rollback_log: Vec::new(),
            default_policy: VersionPolicy::NotifyOnly,
        }
    }

    fn get_latest_version(&self, model_id: u32) -> Option<&ModelVersion> {
        self.versions
            .iter()
            .filter(|v| v.model_id == model_id && !v.is_deprecated)
            .max_by_key(|v| v.version)
    }

    fn get_version(&self, model_id: u32, version: u32) -> Option<&ModelVersion> {
        self.versions
            .iter()
            .find(|v| v.model_id == model_id && v.version == version)
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialize the versioning subsystem.
pub fn init() {
    let mut mgr = VERSION_MANAGER.lock();
    *mgr = Some(VersionManager::new());
    serial_println!("    AI Market versioning initialized");
}

/// Register a new model version in the catalog.
pub fn register_version(version: ModelVersion) {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");

    // Avoid duplicates
    let exists = mgr
        .versions
        .iter()
        .any(|v| v.model_id == version.model_id && v.version == version.version);
    if !exists {
        mgr.versions.push(version);
    }
}

/// Register that a model has been installed at a specific version.
pub fn register_install(model_id: u32, version: u32) {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");

    // Update existing entry or create new one
    if let Some(inst) = mgr.installed.iter_mut().find(|i| i.model_id == model_id) {
        inst.installed_version = version;
        inst.install_date = 0; // kernel timestamp
    } else {
        mgr.installed.push(InstalledVersion {
            model_id,
            installed_version: version,
            policy: mgr.default_policy,
            pinned_version: None,
            install_date: 0,
            last_checked: 0,
            auto_rollback: false,
        });
    }
}

/// Check for updates across all installed models.
/// Returns a list of models that have newer versions available.
pub fn check_updates() -> Vec<AvailableUpdate> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    let mut updates = Vec::new();

    for inst in &mgr.installed {
        // Skip pinned models
        if inst.policy == VersionPolicy::Pinned {
            continue;
        }

        if let Some(latest) = mgr.get_latest_version(inst.model_id) {
            if latest.version > inst.installed_version {
                let (cur_major, _, _) = unpack_version(inst.installed_version);
                let (new_major, _, _) = unpack_version(latest.version);

                let status = if latest.is_security_fix {
                    UpdateStatus::SecurityUpdate
                } else if latest.is_deprecated {
                    UpdateStatus::Deprecated
                } else if new_major > cur_major {
                    UpdateStatus::MajorUpdateAvailable
                } else {
                    UpdateStatus::UpdateAvailable
                };

                updates.push(AvailableUpdate {
                    model_id: inst.model_id,
                    current_version: inst.installed_version,
                    latest_version: latest.version,
                    size_bytes: latest.size_bytes,
                    status,
                    changelog_hash: latest.changelog_hash,
                    is_security: latest.is_security_fix,
                });
            }
        }
    }

    // Sort: security updates first, then by model id
    updates.sort_by(|a, b| {
        b.is_security
            .cmp(&a.is_security)
            .then(a.model_id.cmp(&b.model_id))
    });

    updates
}

/// Get the changelog hash for a specific model version.
pub fn get_changelog(model_id: u32, version: u32) -> Option<u64> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    mgr.get_version(model_id, version).map(|v| v.changelog_hash)
}

/// Rollback a model to a previous version.
/// Returns true if the rollback was successful.
pub fn rollback(model_id: u32, target_version: u32, reason_hash: u64) -> bool {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");

    // Verify target version exists
    let target_exists = mgr
        .versions
        .iter()
        .any(|v| v.model_id == model_id && v.version == target_version);

    if !target_exists {
        return false;
    }

    // Find the installed entry
    if let Some(inst) = mgr.installed.iter_mut().find(|i| i.model_id == model_id) {
        let from_version = inst.installed_version;

        // Can only rollback to an older version
        if target_version >= from_version {
            return false;
        }

        // Record rollback in the log
        mgr.rollback_log.push(RollbackEntry {
            model_id,
            from_version,
            to_version: target_version,
            timestamp: 0, // kernel timestamp
            reason_hash,
        });

        inst.installed_version = target_version;
        true
    } else {
        false
    }
}

/// Pin a model to a specific version, preventing automatic updates.
pub fn pin_version(model_id: u32, version: u32) -> bool {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");

    // Verify the version exists
    let version_exists = mgr
        .versions
        .iter()
        .any(|v| v.model_id == model_id && v.version == version);

    if !version_exists {
        return false;
    }

    if let Some(inst) = mgr.installed.iter_mut().find(|i| i.model_id == model_id) {
        inst.policy = VersionPolicy::Pinned;
        inst.pinned_version = Some(version);
        true
    } else {
        false
    }
}

/// Set the update policy for a specific installed model.
pub fn set_policy(model_id: u32, policy: VersionPolicy) -> bool {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");

    if let Some(inst) = mgr.installed.iter_mut().find(|i| i.model_id == model_id) {
        inst.policy = policy;
        // Clear pin if moving away from Pinned
        if policy != VersionPolicy::Pinned {
            inst.pinned_version = None;
        }
        true
    } else {
        false
    }
}

/// Compare two versions and return the relationship.
pub fn compare_versions(version_a: u32, version_b: u32) -> VersionComparison {
    let (a_major, a_minor, a_patch) = unpack_version(version_a);
    let (b_major, b_minor, b_patch) = unpack_version(version_b);

    if a_major != b_major {
        // Major version difference may indicate incompatibility
        if a_major > b_major {
            return VersionComparison::Newer;
        } else {
            return VersionComparison::Older;
        }
    }

    // Same major version: compare minor then patch
    if a_minor > b_minor {
        VersionComparison::Newer
    } else if a_minor < b_minor {
        VersionComparison::Older
    } else if a_patch > b_patch {
        VersionComparison::Newer
    } else if a_patch < b_patch {
        VersionComparison::Older
    } else {
        VersionComparison::Same
    }
}

/// Get the currently installed version of a model.
pub fn get_installed_version(model_id: u32) -> Option<u32> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    mgr.installed
        .iter()
        .find(|i| i.model_id == model_id)
        .map(|i| i.installed_version)
}

/// Get all available versions for a model, sorted newest first.
pub fn get_available_versions(model_id: u32) -> Vec<ModelVersion> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    let mut versions: Vec<ModelVersion> = mgr
        .versions
        .iter()
        .filter(|v| v.model_id == model_id)
        .cloned()
        .collect();

    versions.sort_by(|a, b| b.version.cmp(&a.version));
    versions
}

/// Set the default update policy for newly installed models.
pub fn set_default_policy(policy: VersionPolicy) {
    let mut guard = VERSION_MANAGER.lock();
    let mgr = guard.as_mut().expect("version manager not initialized");
    mgr.default_policy = policy;
}

/// Get the update policy for a specific model.
pub fn get_policy(model_id: u32) -> Option<VersionPolicy> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    mgr.installed
        .iter()
        .find(|i| i.model_id == model_id)
        .map(|i| i.policy)
}

/// Get the full rollback history for a model.
pub fn get_rollback_history(model_id: u32) -> Vec<RollbackEntry> {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    mgr.rollback_log
        .iter()
        .filter(|r| r.model_id == model_id)
        .cloned()
        .collect()
}

/// Check hardware compatibility for a specific model version.
/// Returns true if the given hw_hash is in the compatible list.
pub fn check_hw_compat(model_id: u32, version: u32, hw_hash: u64) -> bool {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    if let Some(ver) = mgr.get_version(model_id, version) {
        // Empty compatible_hw means "compatible with everything"
        if ver.compatible_hw.is_empty() {
            return true;
        }
        ver.compatible_hw.iter().any(|h| *h == hw_hash)
    } else {
        false
    }
}

/// Check minimum RAM requirement for a model version.
pub fn check_ram_requirement(model_id: u32, version: u32, available_ram_mb: u64) -> bool {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");

    if let Some(ver) = mgr.get_version(model_id, version) {
        available_ram_mb >= ver.min_ram_mb
    } else {
        false
    }
}

/// Get total number of registered versions across all models.
pub fn total_versions() -> usize {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");
    mgr.versions.len()
}

/// Get total number of installed models.
pub fn installed_count() -> usize {
    let guard = VERSION_MANAGER.lock();
    let mgr = guard.as_ref().expect("version manager not initialized");
    mgr.installed.len()
}
