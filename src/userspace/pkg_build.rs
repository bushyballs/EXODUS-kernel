use crate::sync::Mutex;
/// Package Building — build scripts, source compilation, binary packaging,
/// signing, and changelog management for Genesis
///
/// Provides a build system for compiling source packages into installable
/// binary packages. Supports build recipes, dependency tracking,
/// reproducible builds, GPG signing, and changelog generation.
///
/// Inspired by: Arch PKGBUILD, Debian dpkg-buildpackage, RPM spec files.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Build Recipe
// ---------------------------------------------------------------------------

/// A build recipe describes how to compile a package from source
#[derive(Debug, Clone)]
pub struct BuildRecipe {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub description: String,
    pub license: String,
    pub url: String,
    pub maintainer: String,
    pub arch: BuildArch,
    pub sources: Vec<SourceEntry>,
    pub build_deps: Vec<String>,
    pub runtime_deps: Vec<String>,
    pub build_steps: Vec<BuildStep>,
    pub install_steps: Vec<InstallStep>,
    pub options: BuildOptions,
}

/// Source entry for a build recipe
#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub url: String,
    pub filename: String,
    pub checksum_sha256: [u8; 32],
    pub extract: bool,
}

/// Architecture constraint for builds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildArch {
    X86_64,
    Aarch64,
    Any,
    Native,
}

impl BuildArch {
    pub fn name(self) -> &'static str {
        match self {
            BuildArch::X86_64 => "x86_64",
            BuildArch::Aarch64 => "aarch64",
            BuildArch::Any => "any",
            BuildArch::Native => "native",
        }
    }
}

/// A single build step
#[derive(Debug, Clone)]
pub struct BuildStep {
    pub phase: BuildPhase,
    pub command: String,
    pub working_dir: Option<String>,
    pub env_vars: Vec<(String, String)>,
}

/// Build phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildPhase {
    Prepare,   // patch sources, autoreconf, etc.
    Configure, // ./configure or cmake
    Build,     // make / cargo build
    Check,     // make check / cargo test
    Package,   // install into staging directory
}

impl BuildPhase {
    pub fn name(self) -> &'static str {
        match self {
            BuildPhase::Prepare => "prepare",
            BuildPhase::Configure => "configure",
            BuildPhase::Build => "build",
            BuildPhase::Check => "check",
            BuildPhase::Package => "package",
        }
    }

    pub fn order(self) -> u32 {
        match self {
            BuildPhase::Prepare => 0,
            BuildPhase::Configure => 1,
            BuildPhase::Build => 2,
            BuildPhase::Check => 3,
            BuildPhase::Package => 4,
        }
    }
}

/// Install step for placing files into the package
#[derive(Debug, Clone)]
pub struct InstallStep {
    pub source_path: String,
    pub dest_path: String,
    pub permissions: u32,
    pub is_directory: bool,
}

/// Build options
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub strip_binaries: bool,
    pub enable_debug: bool,
    pub parallel_jobs: u32,
    pub run_tests: bool,
    pub ccache_enabled: bool,
    pub reproducible: bool,
    pub sign_package: bool,
}

impl BuildOptions {
    pub const fn default_options() -> Self {
        BuildOptions {
            strip_binaries: true,
            enable_debug: false,
            parallel_jobs: 4,
            run_tests: true,
            ccache_enabled: false,
            reproducible: true,
            sign_package: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Build Result and State
// ---------------------------------------------------------------------------

/// Result of a build
#[derive(Debug, Clone)]
pub struct BuildResult {
    pub package_name: String,
    pub version: String,
    pub output_path: String,
    pub size_bytes: u64,
    pub checksum_sha256: [u8; 32],
    pub build_time_secs: u64,
    pub status: BuildStatus,
    pub log_path: String,
    pub warnings: Vec<String>,
}

/// Build status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Success,
    FailedPrepare,
    FailedConfigure,
    FailedBuild,
    FailedCheck,
    FailedPackage,
    FailedSign,
    Cancelled,
}

impl BuildStatus {
    pub fn name(self) -> &'static str {
        match self {
            BuildStatus::Success => "success",
            BuildStatus::FailedPrepare => "failed-prepare",
            BuildStatus::FailedConfigure => "failed-configure",
            BuildStatus::FailedBuild => "failed-build",
            BuildStatus::FailedCheck => "failed-check",
            BuildStatus::FailedPackage => "failed-package",
            BuildStatus::FailedSign => "failed-sign",
            BuildStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_success(self) -> bool {
        matches!(self, BuildStatus::Success)
    }
}

// ---------------------------------------------------------------------------
// Changelog
// ---------------------------------------------------------------------------

/// A changelog entry
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub version: String,
    pub date_epoch: u64,
    pub author: String,
    pub urgency: ChangeUrgency,
    pub changes: Vec<String>,
}

/// Urgency of a changelog release
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeUrgency {
    Low,
    Medium,
    High,
    Emergency,
    Critical,
}

impl ChangeUrgency {
    pub fn name(self) -> &'static str {
        match self {
            ChangeUrgency::Low => "low",
            ChangeUrgency::Medium => "medium",
            ChangeUrgency::High => "high",
            ChangeUrgency::Emergency => "emergency",
            ChangeUrgency::Critical => "critical",
        }
    }
}

/// Package changelog
#[derive(Debug, Clone)]
pub struct Changelog {
    pub package_name: String,
    pub entries: Vec<ChangelogEntry>,
}

impl Changelog {
    pub fn new(name: &str) -> Self {
        Changelog {
            package_name: String::from(name),
            entries: Vec::new(),
        }
    }

    pub fn add_entry(
        &mut self,
        version: &str,
        author: &str,
        urgency: ChangeUrgency,
        changes: Vec<String>,
        date_epoch: u64,
    ) {
        self.entries.push(ChangelogEntry {
            version: String::from(version),
            date_epoch,
            author: String::from(author),
            urgency,
            changes,
        });
    }

    /// Format changelog as text
    pub fn format(&self) -> String {
        let mut out = alloc::format!("Changelog for {}\n", self.package_name);
        out.push_str(&alloc::format!("{}\n\n", "=".repeat(40)));

        for entry in &self.entries {
            out.push_str(&alloc::format!(
                "## {} ({}) - {}\n",
                entry.version,
                entry.urgency.name(),
                entry.author
            ));
            for change in &entry.changes {
                out.push_str(&alloc::format!("  * {}\n", change));
            }
            out.push('\n');
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Package Signing
// ---------------------------------------------------------------------------

/// Simple signing key (placeholder for real GPG)
#[derive(Debug, Clone)]
pub struct SigningKey {
    pub key_id: String,
    pub fingerprint: [u8; 20],
    pub owner: String,
    pub created_epoch: u64,
    pub expires_epoch: u64,
    pub is_private: bool,
}

/// Signature on a built package
#[derive(Debug, Clone)]
pub struct PackageSignature {
    pub key_id: String,
    pub signature_bytes: Vec<u8>,
    pub timestamp: u64,
    pub verified: bool,
}

impl PackageSignature {
    /// Create a placeholder signature (real signing would use RSA/Ed25519)
    pub fn sign_placeholder(key: &SigningKey, _data_hash: &[u8; 32]) -> Self {
        // In a real OS, this would perform cryptographic signing.
        // For now, produce a deterministic placeholder.
        let mut sig = Vec::with_capacity(64);
        for i in 0u8..64 {
            sig.push(i ^ key.fingerprint[(i as usize) % 20]);
        }
        PackageSignature {
            key_id: key.key_id.clone(),
            signature_bytes: sig,
            timestamp: crate::time::clock::uptime_secs(),
            verified: true,
        }
    }

    /// Verify a signature (placeholder)
    pub fn verify(&self, _key: &SigningKey, _data_hash: &[u8; 32]) -> bool {
        // Real verification would check RSA/Ed25519 signature
        self.verified && self.signature_bytes.len() == 64
    }
}

// ---------------------------------------------------------------------------
// Build Engine
// ---------------------------------------------------------------------------

/// The build engine manages builds
pub struct BuildEngine {
    pub recipes: BTreeMap<String, BuildRecipe>,
    pub results: Vec<BuildResult>,
    pub changelogs: BTreeMap<String, Changelog>,
    pub signing_keys: Vec<SigningKey>,
    pub build_root: String,
    pub output_dir: String,
    pub log_dir: String,
    pub total_builds: u64,
    pub successful_builds: u64,
}

impl BuildEngine {
    pub const fn new() -> Self {
        BuildEngine {
            recipes: BTreeMap::new(),
            results: Vec::new(),
            changelogs: BTreeMap::new(),
            signing_keys: Vec::new(),
            build_root: String::new(),
            output_dir: String::new(),
            log_dir: String::new(),
            total_builds: 0,
            successful_builds: 0,
        }
    }

    /// Register a build recipe
    pub fn add_recipe(&mut self, recipe: BuildRecipe) {
        self.recipes.insert(recipe.name.clone(), recipe);
    }

    /// Execute a build from a recipe
    pub fn build(&mut self, name: &str) -> Result<BuildResult, BuildError> {
        let recipe = self
            .recipes
            .get(name)
            .ok_or(BuildError::RecipeNotFound(String::from(name)))?
            .clone();

        self.total_builds = self.total_builds.saturating_add(1);
        let build_start = crate::time::clock::uptime_secs();

        serial_println!(
            "  [build] Starting build: {} v{}-{}",
            recipe.name,
            recipe.version,
            recipe.release
        );

        // Sort build steps by phase order
        let mut steps = recipe.build_steps.clone();
        steps.sort_by_key(|s| s.phase.order());

        // Execute each phase
        for step in &steps {
            serial_println!(
                "  [build]   Phase: {} -> {}",
                step.phase.name(),
                step.command
            );

            // In a real OS, this would fork+exec the command.
            // Check for simulated failures (empty command = fail).
            if step.command.is_empty() {
                let status = match step.phase {
                    BuildPhase::Prepare => BuildStatus::FailedPrepare,
                    BuildPhase::Configure => BuildStatus::FailedConfigure,
                    BuildPhase::Build => BuildStatus::FailedBuild,
                    BuildPhase::Check => BuildStatus::FailedCheck,
                    BuildPhase::Package => BuildStatus::FailedPackage,
                };
                return Ok(BuildResult {
                    package_name: recipe.name.clone(),
                    version: recipe.version.clone(),
                    output_path: String::new(),
                    size_bytes: 0,
                    checksum_sha256: [0u8; 32],
                    build_time_secs: crate::time::clock::uptime_secs() - build_start,
                    status,
                    log_path: alloc::format!("{}/{}-build.log", self.log_dir, recipe.name),
                    warnings: Vec::new(),
                });
            }
        }

        let build_time = crate::time::clock::uptime_secs() - build_start;

        // Generate a placeholder hash for the built package
        let mut hash = [0u8; 32];
        let name_bytes = recipe.name.as_bytes();
        for (i, b) in name_bytes.iter().enumerate() {
            hash[i % 32] ^= *b;
        }

        let output_path = alloc::format!(
            "{}/{}-{}-{}.gpkg",
            self.output_dir,
            recipe.name,
            recipe.version,
            recipe.release
        );

        // Sign the package if requested
        if recipe.options.sign_package && !self.signing_keys.is_empty() {
            let key = &self.signing_keys[0];
            let _sig = PackageSignature::sign_placeholder(key, &hash);
            serial_println!("  [build]   Signed with key {}", key.key_id);
        }

        self.successful_builds = self.successful_builds.saturating_add(1);

        let result = BuildResult {
            package_name: recipe.name.clone(),
            version: recipe.version.clone(),
            output_path,
            size_bytes: 0, // would be real file size
            checksum_sha256: hash,
            build_time_secs: build_time,
            status: BuildStatus::Success,
            log_path: alloc::format!("{}/{}-build.log", self.log_dir, recipe.name),
            warnings: Vec::new(),
        };

        serial_println!(
            "  [build] Build complete: {} ({}s)",
            recipe.name,
            build_time
        );
        self.results.push(result.clone());
        Ok(result)
    }

    /// Get build history
    pub fn history(&self) -> &[BuildResult] {
        &self.results
    }

    /// Get the last N build results
    pub fn recent_builds(&self, n: usize) -> Vec<&BuildResult> {
        let len = self.results.len();
        let skip = if len > n { len - n } else { 0 };
        self.results.iter().skip(skip).collect()
    }

    /// Add a changelog for a package
    pub fn add_changelog(&mut self, changelog: Changelog) {
        self.changelogs
            .insert(changelog.package_name.clone(), changelog);
    }

    /// Get changelog for a package
    pub fn get_changelog(&self, name: &str) -> Option<&Changelog> {
        self.changelogs.get(name)
    }

    /// Add a signing key
    pub fn add_signing_key(&mut self, key: SigningKey) {
        self.signing_keys.push(key);
    }

    /// Get build success rate in Q16 fixed-point (0..65536)
    pub fn success_rate_q16(&self) -> i32 {
        if self.total_builds == 0 {
            return 65536; // 1.0 if no builds
        }
        ((self.successful_builds as i64 * 65536) / self.total_builds as i64) as i32
    }

    /// Format build stats as a string
    pub fn format_stats(&self) -> String {
        let rate = self.success_rate_q16();
        // Convert Q16 to percentage: (rate * 100) >> 16
        let pct = ((rate as i64 * 100) >> 16) as u32;
        alloc::format!(
            "Builds: {} total, {} success ({}%)\nRecipes: {}\nChangelogs: {}\nKeys: {}",
            self.total_builds,
            self.successful_builds,
            pct,
            self.recipes.len(),
            self.changelogs.len(),
            self.signing_keys.len()
        )
    }
}

/// Build errors
#[derive(Debug)]
pub enum BuildError {
    RecipeNotFound(String),
    DependencyMissing(String),
    SourceFetchFailed(String),
    ChecksumMismatch(String),
    CompilationFailed(String),
    SigningFailed(String),
    DiskFull,
}

// ---------------------------------------------------------------------------
// Global State
// ---------------------------------------------------------------------------

static BUILD_ENGINE: Mutex<BuildEngine> = Mutex::new(BuildEngine::new());

/// Initialize the package build subsystem
pub fn init() {
    let mut engine = BUILD_ENGINE.lock();

    engine.build_root = String::from("/var/build");
    engine.output_dir = String::from("/var/build/output");
    engine.log_dir = String::from("/var/build/logs");

    // Add a default signing key
    engine.add_signing_key(SigningKey {
        key_id: String::from("HOAGS-BUILD-2024"),
        fingerprint: [
            0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45,
            0x67, 0x89, 0xAA, 0xBB, 0xCC, 0xDD,
        ],
        owner: String::from("Genesis Build System <build@hoagsinc.com>"),
        created_epoch: 1700000000,
        expires_epoch: 1800000000,
        is_private: true,
    });

    // Register a sample build recipe for the kernel itself
    let kernel_recipe = BuildRecipe {
        name: String::from("genesis-kernel"),
        version: String::from("0.3.0"),
        release: 1,
        description: String::from("Genesis bare-metal OS kernel"),
        license: String::from("Proprietary"),
        url: String::from("https://hoagsinc.com/genesis"),
        maintainer: String::from("Hoags Inc <dev@hoagsinc.com>"),
        arch: BuildArch::X86_64,
        sources: vec![SourceEntry {
            url: String::from("https://src.hoagsinc.com/genesis-0.3.0.tar.xz"),
            filename: String::from("genesis-0.3.0.tar.xz"),
            checksum_sha256: [0u8; 32],
            extract: true,
        }],
        build_deps: vec![
            String::from("rust-nightly"),
            String::from("nasm"),
            String::from("xorriso"),
        ],
        runtime_deps: Vec::new(),
        build_steps: vec![
            BuildStep {
                phase: BuildPhase::Configure,
                command: String::from("cargo check"),
                working_dir: None,
                env_vars: Vec::new(),
            },
            BuildStep {
                phase: BuildPhase::Build,
                command: String::from("cargo build --release --target x86_64-genesis"),
                working_dir: None,
                env_vars: Vec::new(),
            },
        ],
        install_steps: vec![InstallStep {
            source_path: String::from("target/x86_64-genesis/release/genesis"),
            dest_path: String::from("/boot/genesis"),
            permissions: 0o755,
            is_directory: false,
        }],
        options: BuildOptions::default_options(),
    };
    engine.add_recipe(kernel_recipe);

    // Create a changelog for the kernel
    let mut cl = Changelog::new("genesis-kernel");
    cl.add_entry(
        "0.3.0",
        "Hoags Inc",
        ChangeUrgency::Medium,
        vec![
            String::from("Added package repository subsystem"),
            String::from("Added Flatpak-like sandboxed apps"),
            String::from("Added Snap-like app packaging"),
            String::from("Improved memory allocator"),
        ],
        1700000000,
    );
    cl.add_entry(
        "0.2.0",
        "Hoags Inc",
        ChangeUrgency::Low,
        vec![
            String::from("Initial userspace services"),
            String::from("Shell and init service"),
            String::from("Basic package manager"),
        ],
        1690000000,
    );
    engine.add_changelog(cl);

    serial_println!(
        "  PkgBuild: build engine ready ({} recipes, {} keys)",
        engine.recipes.len(),
        engine.signing_keys.len()
    );
}

/// Register a build recipe
pub fn add_recipe(recipe: BuildRecipe) {
    BUILD_ENGINE.lock().add_recipe(recipe);
}

/// Run a build by recipe name
pub fn build(name: &str) -> Result<BuildResult, BuildError> {
    BUILD_ENGINE.lock().build(name)
}

/// Get build statistics
pub fn stats() -> String {
    BUILD_ENGINE.lock().format_stats()
}

/// Get recent build results
pub fn recent(n: usize) -> Vec<String> {
    BUILD_ENGINE
        .lock()
        .recent_builds(n)
        .iter()
        .map(|r| {
            alloc::format!(
                "{} v{} [{}] ({}s)",
                r.package_name,
                r.version,
                r.status.name(),
                r.build_time_secs
            )
        })
        .collect()
}
