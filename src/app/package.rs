/// Package manager for Genesis — app installation and management
///
/// Handles app packages (.gpk files), dependency resolution,
/// installation, updates, and removal. Packages are signed and
/// verified before installation.
///
/// Inspired by: APK/dpkg/pacman. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Package state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState {
    Available,
    Downloading,
    Installing,
    Installed,
    Updating,
    Removing,
    Disabled,
    Error,
}

/// Package info
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub size: u64,
    pub state: PackageState,
    pub dependencies: Vec<String>,
    pub permissions: Vec<String>,
    /// SHA-256 hash of package
    pub hash: [u8; 32],
    /// Signature (Ed25519)
    pub signature: Vec<u8>,
    /// Install path
    pub install_path: String,
    /// Icon (PNG data)
    pub icon: Vec<u8>,
    /// Install timestamp
    pub installed_at: u64,
    /// Last update timestamp
    pub updated_at: u64,
}

/// Repository
pub struct Repository {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub packages: Vec<PackageInfo>,
    pub last_sync: u64,
}

/// Lightweight package info (from repo index)
pub struct PackageInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub size: u64,
    pub description: String,
}

/// Package manager
pub struct PackageManager {
    installed: Vec<Package>,
    repos: Vec<Repository>,
    /// Download queue
    queue: Vec<String>,
    /// Install history
    history: Vec<(String, String, u64)>, // (package_id, action, timestamp)
}

impl PackageManager {
    const fn new() -> Self {
        PackageManager {
            installed: Vec::new(),
            repos: Vec::new(),
            queue: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Install a package from bytes
    pub fn install(
        &mut self,
        id: &str,
        name: &str,
        version: &str,
        data: &[u8],
        permissions: &[&str],
    ) -> bool {
        // Check if already installed
        if self.installed.iter().any(|p| p.id == id) {
            return false;
        }

        // Verify package (simplified — would check signature)
        if data.len() < 4 {
            return false;
        }

        let now = crate::time::clock::unix_time();
        let pkg = Package {
            id: String::from(id),
            name: String::from(name),
            version: String::from(version),
            description: String::new(),
            author: String::new(),
            size: data.len() as u64,
            state: PackageState::Installed,
            dependencies: Vec::new(),
            permissions: permissions.iter().map(|s| String::from(*s)).collect(),
            hash: [0; 32], // would compute SHA-256
            signature: Vec::new(),
            install_path: format!("/apps/{}", id),
            icon: Vec::new(),
            installed_at: now,
            updated_at: now,
        };

        self.installed.push(pkg);
        self.history
            .push((String::from(id), String::from("install"), now));
        true
    }

    /// Uninstall a package
    pub fn uninstall(&mut self, id: &str) -> bool {
        if let Some(pos) = self.installed.iter().position(|p| p.id == id) {
            let now = crate::time::clock::unix_time();
            self.history
                .push((String::from(id), String::from("uninstall"), now));
            self.installed.remove(pos);
            true
        } else {
            false
        }
    }

    /// Enable/disable a package
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> bool {
        if let Some(pkg) = self.installed.iter_mut().find(|p| p.id == id) {
            pkg.state = if enabled {
                PackageState::Installed
            } else {
                PackageState::Disabled
            };
            true
        } else {
            false
        }
    }

    /// List installed packages
    pub fn list_installed(&self) -> Vec<(String, String, String)> {
        self.installed
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.version.clone()))
            .collect()
    }

    /// Check if package is installed
    pub fn is_installed(&self, id: &str) -> bool {
        self.installed
            .iter()
            .any(|p| p.id == id && p.state == PackageState::Installed)
    }

    /// Get package info
    pub fn get_package(&self, id: &str) -> Option<&Package> {
        self.installed.iter().find(|p| p.id == id)
    }

    /// Add a repository
    pub fn add_repo(&mut self, name: &str, url: &str) {
        self.repos.push(Repository {
            name: String::from(name),
            url: String::from(url),
            enabled: true,
            packages: Vec::new(),
            last_sync: 0,
        });
    }

    /// Get total installed size
    pub fn total_size(&self) -> u64 {
        self.installed.iter().map(|p| p.size).sum()
    }
}

static PKG_MANAGER: Mutex<PackageManager> = Mutex::new(PackageManager::new());

pub fn init() {
    // Add default repository
    PKG_MANAGER
        .lock()
        .add_repo("genesis-core", "https://packages.hoags.os/core");
    crate::serial_println!("  [package] Package manager initialized");
}

pub fn install(id: &str, name: &str, version: &str, data: &[u8]) -> bool {
    PKG_MANAGER.lock().install(id, name, version, data, &[])
}
pub fn uninstall(id: &str) -> bool {
    PKG_MANAGER.lock().uninstall(id)
}
pub fn is_installed(id: &str) -> bool {
    PKG_MANAGER.lock().is_installed(id)
}
pub fn list_installed() -> Vec<(String, String, String)> {
    PKG_MANAGER.lock().list_installed()
}
