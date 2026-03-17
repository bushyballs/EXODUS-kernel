/// Scoped storage for Genesis
///
/// Per-app storage sandboxing, shared storage with permission,
/// media collections, and document trees.
///
/// Inspired by: Android Scoped Storage, iOS sandboxing. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Storage scope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageScope {
    AppPrivate,   // Only this app can access
    AppCache,     // Temporary, can be cleared
    SharedImages, // Shared media collection
    SharedVideo,
    SharedAudio,
    SharedDocuments,
    SharedDownloads,
    External, // External SD card
}

/// A storage grant for an app
pub struct StorageGrant {
    pub app_id: String,
    pub scope: StorageScope,
    pub path: String,
    pub read_only: bool,
    pub granted_at: u64,
}

/// A file reference in scoped storage
pub struct FileRef {
    pub uri: String,
    pub display_name: String,
    pub mime_type: String,
    pub size: u64,
    pub modified: u64,
    pub owner_app: String,
}

/// Scoped storage manager
pub struct ScopedStorage {
    pub grants: Vec<StorageGrant>,
    pub files: BTreeMap<String, FileRef>,  // uri -> file
    pub app_quotas: BTreeMap<String, u64>, // app_id -> max bytes
    pub app_usage: BTreeMap<String, u64>,  // app_id -> used bytes
}

impl ScopedStorage {
    const fn new() -> Self {
        ScopedStorage {
            grants: Vec::new(),
            files: BTreeMap::new(),
            app_quotas: BTreeMap::new(),
            app_usage: BTreeMap::new(),
        }
    }

    pub fn grant_access(&mut self, app_id: &str, scope: StorageScope, path: &str) {
        self.grants.push(StorageGrant {
            app_id: String::from(app_id),
            scope,
            path: String::from(path),
            read_only: false,
            granted_at: crate::time::clock::unix_time(),
        });
    }

    pub fn has_access(&self, app_id: &str, scope: StorageScope) -> bool {
        self.grants
            .iter()
            .any(|g| g.app_id == app_id && g.scope == scope)
    }

    pub fn revoke_access(&mut self, app_id: &str, scope: StorageScope) {
        self.grants
            .retain(|g| !(g.app_id == app_id && g.scope == scope));
    }

    pub fn register_file(&mut self, uri: &str, file: FileRef) {
        let size = file.size;
        let owner = file.owner_app.clone();
        self.files.insert(String::from(uri), file);
        *self.app_usage.entry(owner).or_insert(0) += size;
    }

    pub fn remove_file(&mut self, uri: &str) -> bool {
        if let Some(file) = self.files.remove(uri) {
            if let Some(usage) = self.app_usage.get_mut(&file.owner_app) {
                *usage = usage.saturating_sub(file.size);
            }
            true
        } else {
            false
        }
    }

    pub fn set_quota(&mut self, app_id: &str, max_bytes: u64) {
        self.app_quotas.insert(String::from(app_id), max_bytes);
    }

    pub fn check_quota(&self, app_id: &str, additional: u64) -> bool {
        let quota = self.app_quotas.get(app_id).copied().unwrap_or(u64::MAX);
        let used = self.app_usage.get(app_id).copied().unwrap_or(0);
        used + additional <= quota
    }

    pub fn app_private_path(app_id: &str) -> String {
        format!("/data/app/{}/files", app_id)
    }

    pub fn app_cache_path(app_id: &str) -> String {
        format!("/data/app/{}/cache", app_id)
    }
}

static SCOPED: Mutex<ScopedStorage> = Mutex::new(ScopedStorage::new());

pub fn init() {
    crate::serial_println!("  [storage] Scoped storage initialized");
}

pub fn grant(app_id: &str, scope: StorageScope, path: &str) {
    SCOPED.lock().grant_access(app_id, scope, path);
}
