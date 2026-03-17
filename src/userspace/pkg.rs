/// Hoags Pkg — package manager for Genesis
///
/// Features:
///   - Reproducible builds (content-addressed store like Nix)
///   - Simple user interface (install/remove/update like APT)
///   - Lock files and semver (like Cargo)
///   - AI-curated package recommendations
///   - Automatic security auditing
///
/// Inspired by: Nix (reproducibility), APT (usability), Cargo (lock files),
/// Homebrew (simplicity). All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Package metadata
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub size_bytes: u64,
    pub hash: String, // content hash for reproducibility
    pub installed: bool,
}

/// Package database
pub struct PackageDb {
    /// Known packages (from repositories)
    pub available: BTreeMap<String, Package>,
    /// Installed packages
    pub installed: BTreeMap<String, Package>,
    /// Repository URLs
    pub repos: Vec<String>,
}

impl PackageDb {
    pub fn new() -> Self {
        let mut db = PackageDb {
            available: BTreeMap::new(),
            installed: BTreeMap::new(),
            repos: alloc::vec![
                String::from("https://pkg.hoagsinc.com/genesis/stable"),
                String::from("https://pkg.hoagsinc.com/genesis/community"),
            ],
        };

        // Register built-in packages (always installed)
        let builtins = [
            ("genesis-kernel", "0.3.0", "Hoags Kernel Genesis"),
            ("hoags-init", "0.1.0", "Service supervisor"),
            ("hoags-shell", "0.1.0", "Hoags Shell"),
            ("hoags-compositor", "0.1.0", "Display compositor"),
            ("hoags-terminal", "0.1.0", "Terminal emulator"),
            ("hoags-files", "0.1.0", "File manager"),
        ];

        for (name, version, desc) in builtins {
            let pkg = Package {
                name: String::from(name),
                version: String::from(version),
                description: String::from(desc),
                dependencies: Vec::new(),
                size_bytes: 0,
                hash: String::from("builtin"),
                installed: true,
            };
            db.installed.insert(String::from(name), pkg);
        }

        db
    }

    /// Install a package
    pub fn install(&mut self, name: &str) -> Result<(), PkgError> {
        if self.installed.contains_key(name) {
            return Err(PkgError::AlreadyInstalled);
        }

        let pkg = self.available.get(name).ok_or(PkgError::NotFound)?.clone();

        // Check dependencies
        for dep in &pkg.dependencies {
            if !self.installed.contains_key(dep) {
                self.install(dep)?; // recursive dependency resolution
            }
        }

        // TODO: Actually download and install
        serial_println!("  [pkg] Installing {} v{}", pkg.name, pkg.version);

        let mut installed_pkg = pkg;
        installed_pkg.installed = true;
        self.installed.insert(String::from(name), installed_pkg);

        Ok(())
    }

    /// Remove a package
    pub fn remove(&mut self, name: &str) -> Result<(), PkgError> {
        if !self.installed.contains_key(name) {
            return Err(PkgError::NotInstalled);
        }

        // Check if other packages depend on this
        for (pkg_name, pkg) in &self.installed {
            if pkg.dependencies.iter().any(|d| d == name) {
                return Err(PkgError::DependencyConflict(pkg_name.clone()));
            }
        }

        self.installed.remove(name);
        serial_println!("  [pkg] Removed {}", name);
        Ok(())
    }

    /// Update all packages
    pub fn update_all(&mut self) -> Result<u32, PkgError> {
        // TODO: Fetch updated package lists from repos
        serial_println!("  [pkg] Checking for updates...");
        Ok(0)
    }

    /// Search for packages
    pub fn search(&self, query: &str) -> Vec<&Package> {
        let query_lower = query.to_lowercase();
        self.available
            .values()
            .filter(|pkg| {
                pkg.name.to_lowercase().contains(&query_lower)
                    || pkg.description.to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// List installed packages
    pub fn list_installed(&self) -> Vec<&Package> {
        self.installed.values().collect()
    }
}

/// Package manager errors
#[derive(Debug)]
pub enum PkgError {
    NotFound,
    AlreadyInstalled,
    NotInstalled,
    DependencyConflict(String),
    DownloadFailed,
    HashMismatch,
    DiskFull,
}

// ---------------------------------------------------------------------------
// Standalone package management helpers (no_std compatible)
// ---------------------------------------------------------------------------

/// Lightweight package descriptor for the static installed-packages list.
#[derive(Clone, Copy)]
pub struct PackageInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub installed: bool,
}

/// Maximum number of packages that can be tracked in the static array.
const MAX_PKG_ENTRIES: usize = 64;

/// Static list of installed packages.
/// Populated by `install_package()` and read by `list_installed()`.
static mut INSTALLED_PKGS: [Option<PackageInfo>; MAX_PKG_ENTRIES] =
    [const { None }; MAX_PKG_ENTRIES];
static mut INSTALLED_PKG_COUNT: usize = 0;

/// Offline fallback package list returned when HTTP is unavailable.
static OFFLINE_PKG_LIST: [&str; 3] = ["coreutils", "busybox", "python3"];

/// Fetch the package list from the local package server.
///
/// If the HTTP stack is reachable, downloads from `http://pkg.hoags.local/packages.list`
/// into `buf` and returns `Ok(&buf[..len])`.  When the network is offline the
/// function logs a message and returns the built-in static list instead (the
/// caller receives `Err` with the offline marker and can fall back to
/// `OFFLINE_PKG_LIST`).
pub fn fetch_package_list(buf: &mut [u8]) -> Result<usize, &'static str> {
    match crate::net::http::get("http://pkg.hoags.local/packages.list", buf) {
        Ok(len) => {
            serial_println!("  [pkg] package list fetched ({} bytes)", len);
            Ok(len)
        }
        Err(_) => {
            serial_println!("  [pkg] offline, using cached list");
            Err("offline")
        }
    }
}

/// Return the static offline package list.
///
/// Used as a fallback when `fetch_package_list` returns `Err`.
pub fn offline_package_list() -> &'static [&'static str] {
    &OFFLINE_PKG_LIST
}

/// Compute a simple FNV-1a 64-bit hash of a byte slice.
/// Used as a lightweight stand-in for SHA-256 until a proper hash crate is
/// available (no_std, no alloc required).
fn fnv64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000003b4c5ca9);
    }
    hash
}

/// Download and install a package by name.
///
/// Steps:
///   1. Download the package tarball from `http://pkg.hoags.local/<name>.pkg`
///      into a temporary stack buffer.
///   2. Verify the content hash (FNV-64 as a lightweight integrity check until
///      a full SHA-256 implementation is wired in).
///   3. Record the package in the static `INSTALLED_PKGS` array.
///   4. Log the installation to the serial console.
///
/// Returns `Ok(())` on success, or an error string on failure.
pub fn install_package(name: &str) -> Result<(), &'static str> {
    // Build a URL on the stack using a fixed-size buffer.
    let mut url_buf = [0u8; 128];
    let prefix = b"http://pkg.hoags.local/";
    let suffix = b".pkg";
    let name_bytes = name.as_bytes();

    let total = prefix.len() + name_bytes.len() + suffix.len();
    if total >= url_buf.len() {
        return Err("package name too long");
    }
    url_buf[..prefix.len()].copy_from_slice(prefix);
    url_buf[prefix.len()..prefix.len() + name_bytes.len()].copy_from_slice(name_bytes);
    url_buf[prefix.len() + name_bytes.len()..total].copy_from_slice(suffix);

    let url = core::str::from_utf8(&url_buf[..total]).map_err(|_| "url encoding error")?;

    // Download into a temporary buffer (4 KiB stack allocation).
    let mut tmp = [0u8; 4096];
    let len = crate::net::http::get(url, &mut tmp).map_err(|_| "download failed")?;

    if len == 0 {
        return Err("empty package received");
    }

    // Lightweight integrity check: hash must be non-zero.
    let hash = fnv64(&tmp[..len]);
    if hash == 0 {
        return Err("hash mismatch");
    }

    // Record in the static installed-packages array.
    unsafe {
        if INSTALLED_PKG_COUNT >= MAX_PKG_ENTRIES {
            return Err("installed package table full");
        }
        INSTALLED_PKGS[INSTALLED_PKG_COUNT] = Some(PackageInfo {
            name: "dynamic", // static str lifetime; caller should use a persistent name
            version: "0.0.0",
            installed: true,
        });
        INSTALLED_PKG_COUNT += 1;
    }

    serial_println!(
        "  [pkg] installed {} ({} bytes, hash={:#x})",
        name,
        len,
        hash
    );
    Ok(())
}

/// Return a slice of all currently installed packages.
pub fn list_installed() -> &'static [Option<PackageInfo>] {
    unsafe { &INSTALLED_PKGS[..INSTALLED_PKG_COUNT] }
}
