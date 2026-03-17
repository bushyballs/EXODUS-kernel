use crate::sync::Mutex;
/// Package Repository — repository index, metadata, version comparison,
/// dependency resolution, and mirror management for Genesis
///
/// Manages multiple package repositories with priority-based mirror
/// selection, semantic version comparison, topological dependency
/// resolution, and package metadata indexing.
///
/// Inspired by: APT repositories, Cargo registries, Alpine APK indexes.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Semantic Version (Q16-free since versions are integer triples)
// ---------------------------------------------------------------------------

/// Semantic version: major.minor.patch
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub pre_release: Option<String>,
}

impl SemVer {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        SemVer {
            major,
            minor,
            patch,
            pre_release: None,
        }
    }

    /// Parse "1.2.3" or "1.2.3-beta1"
    pub fn parse(s: &str) -> Option<Self> {
        let (version_part, pre) = if let Some(idx) = s.find('-') {
            (&s[..idx], Some(String::from(&s[idx + 1..])))
        } else {
            (s, None)
        };

        let mut parts = version_part.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);
        let patch = parts
            .next()
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(0);

        Some(SemVer {
            major,
            minor,
            patch,
            pre_release: pre,
        })
    }

    /// Format as "major.minor.patch"
    pub fn to_string(&self) -> String {
        if let Some(ref pre) = self.pre_release {
            alloc::format!("{}.{}.{}-{}", self.major, self.minor, self.patch, pre)
        } else {
            alloc::format!("{}.{}.{}", self.major, self.minor, self.patch)
        }
    }

    /// Compare two versions: -1 = less, 0 = equal, 1 = greater
    pub fn cmp_to(&self, other: &SemVer) -> i32 {
        if self.major != other.major {
            return if self.major > other.major { 1 } else { -1 };
        }
        if self.minor != other.minor {
            return if self.minor > other.minor { 1 } else { -1 };
        }
        if self.patch != other.patch {
            return if self.patch > other.patch { 1 } else { -1 };
        }
        // Pre-release sorts before release
        match (&self.pre_release, &other.pre_release) {
            (None, None) => 0,
            (Some(_), None) => -1,
            (None, Some(_)) => 1,
            (Some(a), Some(b)) => {
                if a < b {
                    -1
                } else if a > b {
                    1
                } else {
                    0
                }
            }
        }
    }

    /// Check if self satisfies a version requirement ">=1.2.0"
    pub fn satisfies(&self, req: &VersionReq) -> bool {
        match req.op {
            VersionOp::Exact => self.cmp_to(&req.version) == 0,
            VersionOp::GreaterEq => self.cmp_to(&req.version) >= 0,
            VersionOp::Greater => self.cmp_to(&req.version) > 0,
            VersionOp::LessEq => self.cmp_to(&req.version) <= 0,
            VersionOp::Less => self.cmp_to(&req.version) < 0,
            VersionOp::Compatible => {
                // ^1.2.3 means >=1.2.3 and <2.0.0
                self.cmp_to(&req.version) >= 0 && self.major == req.version.major
            }
            VersionOp::Tilde => {
                // ~1.2.3 means >=1.2.3 and <1.3.0
                self.cmp_to(&req.version) >= 0
                    && self.major == req.version.major
                    && self.minor == req.version.minor
            }
        }
    }

    /// Check if this is a pre-release version
    pub fn is_prerelease(&self) -> bool {
        self.pre_release.is_some()
    }
}

/// Version comparison operator
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionOp {
    Exact,      // =1.2.3
    GreaterEq,  // >=1.2.3
    Greater,    // >1.2.3
    LessEq,     // <=1.2.3
    Less,       // <1.2.3
    Compatible, // ^1.2.3
    Tilde,      // ~1.2.3
}

/// Version requirement
#[derive(Debug, Clone)]
pub struct VersionReq {
    pub op: VersionOp,
    pub version: SemVer,
}

impl VersionReq {
    /// Parse ">=1.2.3", "^1.0", "~2.1.0", "=3.0.0", etc.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (op, rest) = if s.starts_with(">=") {
            (VersionOp::GreaterEq, &s[2..])
        } else if s.starts_with('>') {
            (VersionOp::Greater, &s[1..])
        } else if s.starts_with("<=") {
            (VersionOp::LessEq, &s[2..])
        } else if s.starts_with('<') {
            (VersionOp::Less, &s[1..])
        } else if s.starts_with('^') {
            (VersionOp::Compatible, &s[1..])
        } else if s.starts_with('~') {
            (VersionOp::Tilde, &s[1..])
        } else if s.starts_with('=') {
            (VersionOp::Exact, &s[1..])
        } else {
            (VersionOp::GreaterEq, s) // bare version means >=
        };

        SemVer::parse(rest.trim()).map(|v| VersionReq { op, version: v })
    }
}

// ---------------------------------------------------------------------------
// Package Metadata
// ---------------------------------------------------------------------------

/// Package metadata stored in the repository index
#[derive(Debug, Clone)]
pub struct PackageMeta {
    pub name: String,
    pub version: SemVer,
    pub description: String,
    pub license: String,
    pub homepage: String,
    pub maintainer: String,
    pub arch: PackageArch,
    pub size_bytes: u64,
    pub installed_size_bytes: u64,
    pub checksum_sha256: [u8; 32],
    pub dependencies: Vec<Dependency>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub build_date: u64,
    pub category: PackageCategory,
    pub priority: PackagePriority,
    pub is_essential: bool,
}

/// Package dependency with version constraint
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub version_req: Option<VersionReq>,
    pub optional: bool,
}

/// Target architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageArch {
    X86_64,
    Aarch64,
    Riscv64,
    Any,
}

impl PackageArch {
    pub fn name(self) -> &'static str {
        match self {
            PackageArch::X86_64 => "x86_64",
            PackageArch::Aarch64 => "aarch64",
            PackageArch::Riscv64 => "riscv64",
            PackageArch::Any => "any",
        }
    }
}

/// Package category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageCategory {
    System,
    Libraries,
    Development,
    Network,
    Multimedia,
    Desktop,
    Utilities,
    Games,
    Fonts,
    Documentation,
}

impl PackageCategory {
    pub fn name(self) -> &'static str {
        match self {
            PackageCategory::System => "system",
            PackageCategory::Libraries => "libs",
            PackageCategory::Development => "devel",
            PackageCategory::Network => "net",
            PackageCategory::Multimedia => "multimedia",
            PackageCategory::Desktop => "desktop",
            PackageCategory::Utilities => "utils",
            PackageCategory::Games => "games",
            PackageCategory::Fonts => "fonts",
            PackageCategory::Documentation => "doc",
        }
    }
}

/// Package priority
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackagePriority {
    Required,
    Important,
    Standard,
    Optional,
    Extra,
}

// ---------------------------------------------------------------------------
// Mirror Management
// ---------------------------------------------------------------------------

/// Mirror health tracking using Q16 fixed-point for latency score
#[derive(Debug, Clone)]
pub struct Mirror {
    pub url: String,
    pub name: String,
    pub country: String,
    pub enabled: bool,
    /// Latency score in Q16 fixed-point (lower is better)
    pub latency_q16: i32,
    /// Availability score in Q16 (0..65536 maps to 0.0..1.0)
    pub availability_q16: i32,
    pub total_requests: u64,
    pub failed_requests: u64,
    pub last_sync_epoch: u64,
    pub priority: u32,
}

/// Q16 constant: 1.0 = 65536
const Q16_ONE: i32 = 65536;
/// Q16 constant: 0.9 = 58982
const Q16_POINT_NINE: i32 = 58982;
/// Q16 constant: 0.1 = 6554
const Q16_POINT_ONE: i32 = 6554;

impl Mirror {
    pub fn new(url: &str, name: &str, country: &str, priority: u32) -> Self {
        Mirror {
            url: String::from(url),
            name: String::from(name),
            country: String::from(country),
            enabled: true,
            latency_q16: 100 * Q16_ONE, // default 100ms
            availability_q16: Q16_ONE,  // default 100%
            total_requests: 0,
            failed_requests: 0,
            last_sync_epoch: 0,
            priority,
        }
    }

    /// Record a successful request with latency in milliseconds
    pub fn record_success(&mut self, latency_ms: u32) {
        self.total_requests = self.total_requests.saturating_add(1);
        // Exponential moving average: new = 0.9 * old + 0.1 * sample
        let sample_q16 = (latency_ms as i32) * Q16_ONE;
        self.latency_q16 = ((self.latency_q16 as i64 * Q16_POINT_NINE as i64) >> 16) as i32
            + ((sample_q16 as i64 * Q16_POINT_ONE as i64) >> 16) as i32;
        self.update_availability();
    }

    /// Record a failed request
    pub fn record_failure(&mut self) {
        self.total_requests = self.total_requests.saturating_add(1);
        self.failed_requests = self.failed_requests.saturating_add(1);
        self.update_availability();
    }

    fn update_availability(&mut self) {
        if self.total_requests == 0 {
            self.availability_q16 = Q16_ONE;
            return;
        }
        let success = self.total_requests - self.failed_requests;
        // availability = success / total, in Q16
        self.availability_q16 =
            ((success as i64 * Q16_ONE as i64) / self.total_requests as i64) as i32;
    }

    /// Composite score for mirror ranking (lower is better), Q16
    pub fn score_q16(&self) -> i32 {
        // score = latency * (2.0 - availability) + priority_penalty
        let inv_avail = 2 * Q16_ONE - self.availability_q16;
        let score = ((self.latency_q16 as i64 * inv_avail as i64) >> 16) as i32;
        score + (self.priority as i32) * Q16_ONE
    }
}

// ---------------------------------------------------------------------------
// Repository Index
// ---------------------------------------------------------------------------

/// A single package repository
#[derive(Debug, Clone)]
pub struct Repository {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub gpg_key_id: String,
    pub arch: PackageArch,
    pub suite: String,     // e.g., "stable", "testing"
    pub component: String, // e.g., "main", "contrib", "community"
    pub last_updated: u64,
    pub package_count: u32,
}

/// Dependency resolution result
#[derive(Debug, Clone)]
pub struct ResolvedSet {
    /// Packages to install, in dependency order
    pub install_order: Vec<String>,
    /// Total download size in bytes
    pub download_size: u64,
    /// Total installed size in bytes
    pub installed_size: u64,
    /// Any conflicts detected
    pub conflicts: Vec<String>,
}

/// Repository manager
pub struct RepoManager {
    pub repositories: Vec<Repository>,
    pub mirrors: Vec<Mirror>,
    pub index: BTreeMap<String, Vec<PackageMeta>>,
    pub installed: BTreeMap<String, SemVer>,
    pub pinned: BTreeMap<String, VersionReq>,
    pub update_interval_secs: u64,
    pub cache_dir: String,
    pub auto_update: bool,
}

impl RepoManager {
    pub const fn new() -> Self {
        RepoManager {
            repositories: Vec::new(),
            mirrors: Vec::new(),
            index: BTreeMap::new(),
            installed: BTreeMap::new(),
            pinned: BTreeMap::new(),
            update_interval_secs: 86400,
            cache_dir: String::new(),
            auto_update: true,
        }
    }

    /// Add a repository
    pub fn add_repo(&mut self, name: &str, url: &str, suite: &str, component: &str) {
        self.repositories.push(Repository {
            name: String::from(name),
            url: String::from(url),
            enabled: true,
            gpg_key_id: String::new(),
            arch: PackageArch::X86_64,
            suite: String::from(suite),
            component: String::from(component),
            last_updated: 0,
            package_count: 0,
        });
    }

    /// Add a mirror for downloads
    pub fn add_mirror(&mut self, url: &str, name: &str, country: &str, priority: u32) {
        self.mirrors.push(Mirror::new(url, name, country, priority));
    }

    /// Get the best mirror (lowest composite score)
    pub fn best_mirror(&self) -> Option<&Mirror> {
        self.mirrors
            .iter()
            .filter(|m| m.enabled)
            .min_by_key(|m| m.score_q16())
    }

    /// Register a package in the index
    pub fn index_package(&mut self, meta: PackageMeta) {
        let name = meta.name.clone();
        self.index.entry(name).or_insert_with(Vec::new).push(meta);
    }

    /// Look up a package by name, returning the latest version
    pub fn lookup(&self, name: &str) -> Option<&PackageMeta> {
        self.index.get(name).and_then(|versions| {
            versions
                .iter()
                .max_by(|a, b| match a.version.cmp_to(&b.version) {
                    x if x > 0 => core::cmp::Ordering::Greater,
                    x if x < 0 => core::cmp::Ordering::Less,
                    _ => core::cmp::Ordering::Equal,
                })
        })
    }

    /// Look up a package matching a version requirement
    pub fn lookup_versioned(&self, name: &str, req: &VersionReq) -> Option<&PackageMeta> {
        self.index.get(name).and_then(|versions| {
            versions
                .iter()
                .filter(|m| m.version.satisfies(req))
                .max_by(|a, b| match a.version.cmp_to(&b.version) {
                    x if x > 0 => core::cmp::Ordering::Greater,
                    x if x < 0 => core::cmp::Ordering::Less,
                    _ => core::cmp::Ordering::Equal,
                })
        })
    }

    /// Search packages by name or description substring
    pub fn search(&self, query: &str) -> Vec<&PackageMeta> {
        let q = query.to_lowercase();
        let mut results = Vec::new();
        for versions in self.index.values() {
            if let Some(latest) = versions.last() {
                if latest.name.to_lowercase().contains(&q)
                    || latest.description.to_lowercase().contains(&q)
                {
                    results.push(latest);
                }
            }
        }
        results
    }

    /// Resolve dependencies for installing a package
    pub fn resolve_deps(&self, name: &str) -> Result<ResolvedSet, RepoError> {
        let mut visited: BTreeMap<String, bool> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        let mut total_download: u64 = 0;
        let mut total_installed: u64 = 0;
        let mut conflicts_found: Vec<String> = Vec::new();

        self.resolve_recursive(
            name,
            &mut visited,
            &mut order,
            &mut total_download,
            &mut total_installed,
            &mut conflicts_found,
        )?;

        Ok(ResolvedSet {
            install_order: order,
            download_size: total_download,
            installed_size: total_installed,
            conflicts: conflicts_found,
        })
    }

    fn resolve_recursive(
        &self,
        name: &str,
        visited: &mut BTreeMap<String, bool>,
        order: &mut Vec<String>,
        download: &mut u64,
        installed: &mut u64,
        conflicts: &mut Vec<String>,
    ) -> Result<(), RepoError> {
        // Cycle detection
        if let Some(&in_progress) = visited.get(name) {
            if in_progress {
                return Err(RepoError::CyclicDependency(String::from(name)));
            }
            return Ok(()); // already resolved
        }

        // Skip if already installed
        if self.installed.contains_key(name) {
            return Ok(());
        }

        visited.insert(String::from(name), true); // mark in-progress

        let meta = self
            .lookup(name)
            .ok_or(RepoError::PackageNotFound(String::from(name)))?;

        // Check conflicts
        for conflict in &meta.conflicts {
            if self.installed.contains_key(conflict) || visited.contains_key(conflict) {
                conflicts.push(alloc::format!("{} conflicts with {}", name, conflict));
            }
        }

        // Resolve each dependency first
        for dep in &meta.dependencies {
            if dep.optional {
                continue; // skip optional deps
            }
            self.resolve_recursive(&dep.name, visited, order, download, installed, conflicts)?;
        }

        visited.insert(String::from(name), false); // mark completed
        *download += meta.size_bytes;
        *installed += meta.installed_size_bytes;
        order.push(String::from(name));
        Ok(())
    }

    /// Pin a package to a specific version constraint
    pub fn pin_package(&mut self, name: &str, req: &str) -> Result<(), RepoError> {
        let vr = VersionReq::parse(req).ok_or(RepoError::InvalidVersion(String::from(req)))?;
        self.pinned.insert(String::from(name), vr);
        Ok(())
    }

    /// Check which installed packages have updates available
    pub fn check_updates(&self) -> Vec<(String, SemVer, SemVer)> {
        let mut updates = Vec::new();
        for (name, installed_ver) in &self.installed {
            if let Some(latest) = self.lookup(name) {
                if latest.version.cmp_to(installed_ver) > 0 {
                    // Respect pin constraints
                    if let Some(pin) = self.pinned.get(name) {
                        if !latest.version.satisfies(pin) {
                            continue;
                        }
                    }
                    updates.push((name.clone(), installed_ver.clone(), latest.version.clone()));
                }
            }
        }
        updates
    }

    /// List all enabled repositories
    pub fn list_repos(&self) -> Vec<&Repository> {
        self.repositories.iter().filter(|r| r.enabled).collect()
    }

    /// Enable or disable a repository by name
    pub fn set_repo_enabled(&mut self, name: &str, enabled: bool) -> bool {
        for repo in &mut self.repositories {
            if repo.name == name {
                repo.enabled = enabled;
                return true;
            }
        }
        false
    }

    /// Get repository statistics
    pub fn stats(&self) -> RepoStats {
        let total_pkgs = self.index.values().map(|v| v.len() as u32).sum();
        let total_repos = self.repositories.len() as u32;
        let enabled_repos = self.repositories.iter().filter(|r| r.enabled).count() as u32;
        let active_mirrors = self.mirrors.iter().filter(|m| m.enabled).count() as u32;
        let installed_count = self.installed.len() as u32;
        let pinned_count = self.pinned.len() as u32;

        RepoStats {
            total_packages: total_pkgs,
            total_repos,
            enabled_repos,
            active_mirrors,
            installed_count,
            pinned_count,
        }
    }
}

/// Repository statistics
#[derive(Debug)]
pub struct RepoStats {
    pub total_packages: u32,
    pub total_repos: u32,
    pub enabled_repos: u32,
    pub active_mirrors: u32,
    pub installed_count: u32,
    pub pinned_count: u32,
}

/// Repository errors
#[derive(Debug)]
pub enum RepoError {
    PackageNotFound(String),
    InvalidVersion(String),
    CyclicDependency(String),
    MirrorUnavailable,
    IndexCorrupted,
    GpgVerifyFailed(String),
    NetworkError(String),
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static REPO_MANAGER: Mutex<RepoManager> = Mutex::new(RepoManager::new());

/// Initialize the package repository subsystem
pub fn init() {
    let mut mgr = REPO_MANAGER.lock();

    mgr.cache_dir = String::from("/var/cache/pkg");

    // Default repositories
    mgr.add_repo(
        "genesis-core",
        "https://pkg.hoagsinc.com/genesis",
        "stable",
        "main",
    );
    mgr.add_repo(
        "genesis-community",
        "https://pkg.hoagsinc.com/genesis",
        "stable",
        "community",
    );
    mgr.add_repo(
        "genesis-testing",
        "https://pkg.hoagsinc.com/genesis",
        "testing",
        "main",
    );
    mgr.set_repo_enabled("genesis-testing", false);

    // Default mirrors
    mgr.add_mirror("https://mirror1.hoagsinc.com/genesis", "US-East", "US", 0);
    mgr.add_mirror("https://mirror2.hoagsinc.com/genesis", "US-West", "US", 0);
    mgr.add_mirror(
        "https://mirror-eu.hoagsinc.com/genesis",
        "EU-Central",
        "DE",
        1,
    );

    // Register core installed packages
    mgr.installed
        .insert(String::from("genesis-kernel"), SemVer::new(0, 3, 0));
    mgr.installed
        .insert(String::from("hoags-init"), SemVer::new(0, 1, 0));
    mgr.installed
        .insert(String::from("hoags-shell"), SemVer::new(0, 1, 0));

    let stats = mgr.stats();
    serial_println!(
        "  PkgRepo: {} repos ({} enabled), {} mirrors, {} installed",
        stats.total_repos,
        stats.enabled_repos,
        stats.active_mirrors,
        stats.installed_count
    );
}

/// Add a repository
pub fn add_repo(name: &str, url: &str, suite: &str, component: &str) {
    REPO_MANAGER.lock().add_repo(name, url, suite, component);
}

/// Search for packages
pub fn search(query: &str) -> Vec<String> {
    REPO_MANAGER
        .lock()
        .search(query)
        .iter()
        .map(|m| alloc::format!("{} ({}): {}", m.name, m.version.to_string(), m.description))
        .collect()
}

/// Resolve dependencies for a package
pub fn resolve(name: &str) -> Result<ResolvedSet, RepoError> {
    REPO_MANAGER.lock().resolve_deps(name)
}

/// Check for available updates
pub fn check_updates() -> Vec<(String, String, String)> {
    REPO_MANAGER
        .lock()
        .check_updates()
        .iter()
        .map(|(n, old, new)| (n.clone(), old.to_string(), new.to_string()))
        .collect()
}

/// Pin a package version
pub fn pin(name: &str, constraint: &str) -> Result<(), RepoError> {
    REPO_MANAGER.lock().pin_package(name, constraint)
}
