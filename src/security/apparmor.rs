/// AppArmor-like profile-based access control for Genesis
///
/// Path-based mandatory access control that confines programs to a limited
/// set of resources. Each profile defines:
///   - File access rules (read/write/execute/append with path globs)
///   - Network access rules (protocol/address/port restrictions)
///   - Capability rules (which POSIX capabilities are allowed)
///   - Resource limits (memory, CPU, file handles)
///
/// Profiles operate in two modes:
///   - Enforce: violations are blocked and logged
///   - Complain: violations are logged but allowed (for policy development)
///
/// Inspired by: AppArmor (Ubuntu), TOMOYO Linux. All code is original.

use crate::serial_println;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use crate::sync::Mutex;

/// Global AppArmor engine state
static APPARMOR: Mutex<Option<AppArmorEngine>> = Mutex::new(None);

/// Maximum number of loaded profiles
const MAX_PROFILES: usize = 512;

/// Maximum number of rules per profile
const MAX_RULES_PER_PROFILE: usize = 256;

/// Maximum path length for rule matching
const MAX_PATH_LEN: usize = 512;

// ── Profile mode ──────────────────────────────────────────────────────

/// Profile enforcement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileMode {
    /// Violations are blocked and logged
    Enforce,
    /// Violations are logged but allowed (for development)
    Complain,
    /// Profile is loaded but inactive
    Disabled,
    /// Profile is being audited — all accesses logged, none blocked
    Audit,
}

// ── File access permissions ───────────────────────────────────────────

/// File access permission flags (packed into a u8)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilePerms {
    /// Read access
    pub read: bool,
    /// Write access
    pub write: bool,
    /// Execute access
    pub execute: bool,
    /// Append-only write
    pub append: bool,
    /// Memory-map executable
    pub mmap_exec: bool,
    /// Create new files
    pub create: bool,
    /// Delete files
    pub delete: bool,
    /// Change ownership
    pub chown: bool,
}

impl FilePerms {
    pub const NONE: Self = FilePerms {
        read: false, write: false, execute: false, append: false,
        mmap_exec: false, create: false, delete: false, chown: false,
    };

    pub const READ_ONLY: Self = FilePerms {
        read: true, write: false, execute: false, append: false,
        mmap_exec: false, create: false, delete: false, chown: false,
    };

    pub const READ_WRITE: Self = FilePerms {
        read: true, write: true, execute: false, append: false,
        mmap_exec: false, create: false, delete: false, chown: false,
    };

    pub const READ_EXEC: Self = FilePerms {
        read: true, write: false, execute: true, append: false,
        mmap_exec: true, create: false, delete: false, chown: false,
    };

    /// Check if `self` is a subset of `allowed`
    pub fn is_subset_of(&self, allowed: &FilePerms) -> bool {
        (!self.read || allowed.read)
            && (!self.write || allowed.write)
            && (!self.execute || allowed.execute)
            && (!self.append || allowed.append)
            && (!self.mmap_exec || allowed.mmap_exec)
            && (!self.create || allowed.create)
            && (!self.delete || allowed.delete)
            && (!self.chown || allowed.chown)
    }
}

// ── Network access rules ──────────────────────────────────────────────

/// Network protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetProtocol {
    Tcp,
    Udp,
    Icmp,
    Raw,
    Any,
}

/// Network access direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDirection {
    /// Outbound connections/sends
    Connect,
    /// Inbound listening/accepts
    Listen,
    /// Both directions
    Both,
}

/// Network rule — controls what network access a profile allows
#[derive(Debug, Clone)]
pub struct NetRule {
    /// Protocol
    pub protocol: NetProtocol,
    /// Direction
    pub direction: NetDirection,
    /// Address pattern (0 = any)
    pub address: u32,
    /// Address mask (for subnet matching)
    pub mask: u32,
    /// Port (0 = any)
    pub port: u16,
    /// Port range end (0 = single port)
    pub port_end: u16,
    /// Allow or deny
    pub allow: bool,
}

impl NetRule {
    /// Check if a network access matches this rule
    pub fn matches(&self, proto: NetProtocol, addr: u32, port: u16, dir: NetDirection) -> bool {
        // Protocol check
        if self.protocol != NetProtocol::Any && self.protocol != proto {
            return false;
        }
        // Direction check
        if self.direction != NetDirection::Both && self.direction != dir {
            return false;
        }
        // Address check (masked)
        if self.address != 0 && (addr & self.mask) != (self.address & self.mask) {
            return false;
        }
        // Port check
        if self.port != 0 {
            if self.port_end != 0 {
                if port < self.port || port > self.port_end {
                    return false;
                }
            } else if port != self.port {
                return false;
            }
        }
        true
    }
}

// ── Capability rules ──────────────────────────────────────────────────

/// POSIX-like capabilities that profiles can grant or deny
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapRule {
    NetAdmin,
    NetBind,
    NetRaw,
    SysAdmin,
    SysPtrace,
    SysChroot,
    SysBoot,
    SysModule,
    SysTime,
    DacOverride,
    DacReadSearch,
    Fowner,
    Kill,
    Setuid,
    Setgid,
    Mknod,
    Audit,
    SysRawio,
}

// ── File rule ─────────────────────────────────────────────────────────

/// File access rule — path pattern with permissions
#[derive(Debug, Clone)]
pub struct FileRule {
    /// Path pattern (supports `*` and `**` globbing)
    pub pattern: String,
    /// Permissions granted for matching paths
    pub perms: FilePerms,
    /// Allow or deny (deny takes priority)
    pub allow: bool,
    /// Owner-conditional: only applies if process owns the file
    pub owner_only: bool,
}

impl FileRule {
    /// Match a path against this rule's glob pattern
    pub fn matches_path(&self, path: &str) -> bool {
        glob_match(&self.pattern, path)
    }
}

// ── Resource limit ────────────────────────────────────────────────────

/// Resource limits that a profile can impose
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Maximum memory (bytes, 0 = unlimited)
    pub max_memory: u64,
    /// Maximum open file descriptors
    pub max_fds: u32,
    /// Maximum child processes
    pub max_procs: u32,
    /// Maximum file size (bytes, 0 = unlimited)
    pub max_file_size: u64,
    /// CPU time limit (ticks, 0 = unlimited)
    pub max_cpu_ticks: u64,
}

impl ResourceLimits {
    pub const UNLIMITED: Self = ResourceLimits {
        max_memory: 0,
        max_fds: 0,
        max_procs: 0,
        max_file_size: 0,
        max_cpu_ticks: 0,
    };
}

// ── Profile ───────────────────────────────────────────────────────────

/// An AppArmor profile — confines a program to specific resources
#[derive(Debug, Clone)]
pub struct Profile {
    /// Profile name (usually the program path)
    pub name: String,
    /// Enforcement mode
    pub mode: ProfileMode,
    /// File access rules (evaluated in order, first match wins)
    pub file_rules: Vec<FileRule>,
    /// Network access rules
    pub net_rules: Vec<NetRule>,
    /// Allowed capabilities
    pub capabilities: Vec<CapRule>,
    /// Denied capabilities (overrides allowed)
    pub denied_caps: Vec<CapRule>,
    /// Resource limits
    pub limits: ResourceLimits,
    /// Child profiles (for hat/subprofile support)
    pub children: Vec<String>,
    /// Whether this profile allows unconfined exec of children
    pub allow_unconfined_exec: bool,
    /// Transition rules: (exec pattern -> target profile name)
    pub transitions: Vec<(String, String)>,
    /// Violation count (for complain mode reporting)
    pub violation_count: u64,
    /// Load timestamp
    pub loaded_at: u64,
}

impl Profile {
    /// Create a new profile with default settings
    pub fn new(name: String, mode: ProfileMode) -> Self {
        Profile {
            name,
            mode,
            file_rules: Vec::new(),
            net_rules: Vec::new(),
            capabilities: Vec::new(),
            denied_caps: Vec::new(),
            limits: ResourceLimits::UNLIMITED,
            children: Vec::new(),
            allow_unconfined_exec: false,
            transitions: Vec::new(),
            violation_count: 0,
            loaded_at: 0,
        }
    }

    /// Add a file rule to this profile
    pub fn add_file_rule(&mut self, pattern: String, perms: FilePerms, allow: bool) {
        if self.file_rules.len() < MAX_RULES_PER_PROFILE {
            self.file_rules.push(FileRule {
                pattern,
                perms,
                allow,
                owner_only: false,
            });
        }
    }

    /// Add a network rule to this profile
    pub fn add_net_rule(&mut self, rule: NetRule) {
        if self.net_rules.len() < MAX_RULES_PER_PROFILE {
            self.net_rules.push(rule);
        }
    }

    /// Grant a capability
    pub fn allow_cap(&mut self, cap: CapRule) {
        if !self.capabilities.contains(&cap) {
            self.capabilities.push(cap);
        }
    }

    /// Deny a capability (overrides allow)
    pub fn deny_cap(&mut self, cap: CapRule) {
        if !self.denied_caps.contains(&cap) {
            self.denied_caps.push(cap);
        }
    }

    /// Add an exec transition: when this profile execs `pattern`, transition to `target`
    pub fn add_transition(&mut self, exec_pattern: String, target_profile: String) {
        self.transitions.push((exec_pattern, target_profile));
    }

    /// Check file access against this profile
    pub fn check_file(&self, path: &str, requested: &FilePerms) -> AccessDecision {
        // Deny rules are checked first
        for rule in &self.file_rules {
            if !rule.allow && rule.matches_path(path) {
                return AccessDecision::Denied;
            }
        }
        // Then allow rules (first match wins)
        for rule in &self.file_rules {
            if rule.allow && rule.matches_path(path) {
                if requested.is_subset_of(&rule.perms) {
                    return AccessDecision::Allowed;
                } else {
                    return AccessDecision::PartialDeny;
                }
            }
        }
        // No matching rule — implicit deny
        AccessDecision::Denied
    }

    /// Check network access against this profile
    pub fn check_network(&self, proto: NetProtocol, addr: u32, port: u16, dir: NetDirection) -> AccessDecision {
        // Deny rules first
        for rule in &self.net_rules {
            if !rule.allow && rule.matches(proto, addr, port, dir) {
                return AccessDecision::Denied;
            }
        }
        // Allow rules
        for rule in &self.net_rules {
            if rule.allow && rule.matches(proto, addr, port, dir) {
                return AccessDecision::Allowed;
            }
        }
        AccessDecision::Denied
    }

    /// Check if a capability is allowed
    pub fn check_cap(&self, cap: CapRule) -> AccessDecision {
        if self.denied_caps.contains(&cap) {
            return AccessDecision::Denied;
        }
        if self.capabilities.contains(&cap) {
            return AccessDecision::Allowed;
        }
        AccessDecision::Denied
    }

    /// Look up exec transition target
    pub fn get_transition(&self, exec_path: &str) -> Option<&str> {
        for (pattern, target) in &self.transitions {
            if glob_match(pattern, exec_path) {
                return Some(target.as_str());
            }
        }
        None
    }
}

// ── Access decision ───────────────────────────────────────────────────

/// Result of an access check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    /// Access is allowed
    Allowed,
    /// Access is denied
    Denied,
    /// Some requested permissions denied (partial match)
    PartialDeny,
}

// ── Violation record ──────────────────────────────────────────────────

/// A recorded policy violation
#[derive(Debug, Clone)]
pub struct Violation {
    /// Profile that was violated
    pub profile_name: String,
    /// Process ID
    pub pid: u32,
    /// Type of violation
    pub vtype: ViolationType,
    /// Resource path or description
    pub resource: String,
    /// What was requested
    pub requested: String,
    /// Timestamp (tick count)
    pub timestamp: u64,
    /// Whether it was enforced (blocked) or just complained
    pub enforced: bool,
}

/// Type of policy violation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationType {
    FileAccess,
    NetworkAccess,
    Capability,
    ResourceLimit,
    ExecTransition,
}

// ── AppArmor Engine ───────────────────────────────────────────────────

/// The main AppArmor engine — manages profiles and enforcement
pub struct AppArmorEngine {
    /// Loaded profiles indexed by name
    profiles: BTreeMap<String, Profile>,
    /// Process-to-profile mapping (PID -> profile name)
    process_profiles: BTreeMap<u32, String>,
    /// Violation log (ring buffer)
    violations: Vec<Violation>,
    /// Maximum violation log entries
    max_violations: usize,
    /// Global enable flag
    enabled: bool,
    /// Default mode for new profiles
    default_mode: ProfileMode,
    /// Statistics
    stats: EngineStats,
}

/// Engine statistics
#[derive(Debug, Clone, Copy)]
pub struct EngineStats {
    pub total_checks: u64,
    pub total_allowed: u64,
    pub total_denied: u64,
    pub total_complained: u64,
    pub profiles_loaded: u32,
    pub processes_confined: u32,
}

impl AppArmorEngine {
    /// Create a new AppArmor engine
    pub fn new() -> Self {
        AppArmorEngine {
            profiles: BTreeMap::new(),
            process_profiles: BTreeMap::new(),
            violations: Vec::new(),
            max_violations: 4096,
            enabled: true,
            default_mode: ProfileMode::Enforce,
            stats: EngineStats {
                total_checks: 0,
                total_allowed: 0,
                total_denied: 0,
                total_complained: 0,
                profiles_loaded: 0,
                processes_confined: 0,
            },
        }
    }

    /// Load a profile into the engine
    pub fn load_profile(&mut self, profile: Profile) -> bool {
        if self.profiles.len() >= MAX_PROFILES {
            serial_println!("  AppArmor: profile limit reached, cannot load '{}'", profile.name);
            return false;
        }
        let name = profile.name.clone();
        self.profiles.insert(name.clone(), profile);
        self.stats.profiles_loaded = self.profiles.len() as u32;
        serial_println!("  AppArmor: loaded profile '{}'", name);
        true
    }

    /// Unload a profile by name
    pub fn unload_profile(&mut self, name: &str) -> bool {
        if self.profiles.remove(name).is_some() {
            // Remove all process assignments for this profile
            self.process_profiles.retain(|_, v| v.as_str() != name);
            self.stats.profiles_loaded = self.profiles.len() as u32;
            self.stats.processes_confined = self.process_profiles.len() as u32;
            serial_println!("  AppArmor: unloaded profile '{}'", name);
            true
        } else {
            false
        }
    }

    /// Assign a process to a profile
    pub fn confine_process(&mut self, pid: u32, profile_name: &str) -> bool {
        if !self.profiles.contains_key(profile_name) {
            return false;
        }
        self.process_profiles.insert(pid, String::from(profile_name));
        self.stats.processes_confined = self.process_profiles.len() as u32;
        true
    }

    /// Release a process from confinement
    pub fn release_process(&mut self, pid: u32) {
        self.process_profiles.remove(&pid);
        self.stats.processes_confined = self.process_profiles.len() as u32;
    }

    /// Change a profile's mode
    pub fn set_profile_mode(&mut self, name: &str, mode: ProfileMode) -> bool {
        if let Some(profile) = self.profiles.get_mut(name) {
            profile.mode = mode;
            true
        } else {
            false
        }
    }

    /// Check file access for a process
    pub fn check_file_access(&mut self, pid: u32, path: &str, perms: &FilePerms) -> AccessDecision {
        self.stats.total_checks = self.stats.total_checks.saturating_add(1);

        if !self.enabled {
            self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
            return AccessDecision::Allowed;
        }

        let profile_name = match self.process_profiles.get(&pid) {
            Some(name) => name.clone(),
            None => {
                // Unconfined process — allow
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
        };

        let (decision, mode) = {
            let profile = match self.profiles.get(&profile_name) {
                Some(p) => p,
                None => {
                    self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                    return AccessDecision::Allowed;
                }
            };
            if profile.mode == ProfileMode::Disabled {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
            (profile.check_file(path, perms), profile.mode)
        };

        self.apply_decision(decision, mode, pid, &profile_name, ViolationType::FileAccess, path)
    }

    /// Check network access for a process
    pub fn check_net_access(
        &mut self, pid: u32, proto: NetProtocol, addr: u32, port: u16, dir: NetDirection,
    ) -> AccessDecision {
        self.stats.total_checks = self.stats.total_checks.saturating_add(1);

        if !self.enabled {
            self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
            return AccessDecision::Allowed;
        }

        let profile_name = match self.process_profiles.get(&pid) {
            Some(name) => name.clone(),
            None => {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
        };

        let (decision, mode) = {
            let profile = match self.profiles.get(&profile_name) {
                Some(p) => p,
                None => {
                    self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                    return AccessDecision::Allowed;
                }
            };
            if profile.mode == ProfileMode::Disabled {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
            (profile.check_network(proto, addr, port, dir), profile.mode)
        };

        let resource = format!("net:{}:{}", addr, port);
        self.apply_decision(decision, mode, pid, &profile_name, ViolationType::NetworkAccess, &resource)
    }

    /// Check capability access for a process
    pub fn check_capability(&mut self, pid: u32, cap: CapRule) -> AccessDecision {
        self.stats.total_checks = self.stats.total_checks.saturating_add(1);

        if !self.enabled {
            self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
            return AccessDecision::Allowed;
        }

        let profile_name = match self.process_profiles.get(&pid) {
            Some(name) => name.clone(),
            None => {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
        };

        let (decision, mode) = {
            let profile = match self.profiles.get(&profile_name) {
                Some(p) => p,
                None => {
                    self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                    return AccessDecision::Allowed;
                }
            };
            if profile.mode == ProfileMode::Disabled {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                return AccessDecision::Allowed;
            }
            (profile.check_cap(cap), profile.mode)
        };

        let resource = format!("cap:{:?}", cap);
        self.apply_decision(decision, mode, pid, &profile_name, ViolationType::Capability, &resource)
    }

    /// Handle exec transitions — returns the new profile name if a transition applies
    pub fn handle_exec(&mut self, pid: u32, exec_path: &str) -> Option<String> {
        let profile_name = self.process_profiles.get(&pid)?.clone();
        let target = {
            let profile = self.profiles.get(&profile_name)?;
            profile.get_transition(exec_path).map(String::from)
        };
        if let Some(ref target_name) = target {
            if self.profiles.contains_key(target_name) {
                self.process_profiles.insert(pid, target_name.clone());
            }
        }
        target
    }

    /// Apply an access decision considering the profile mode
    fn apply_decision(
        &mut self,
        decision: AccessDecision,
        mode: ProfileMode,
        pid: u32,
        profile_name: &str,
        vtype: ViolationType,
        resource: &str,
    ) -> AccessDecision {
        match decision {
            AccessDecision::Allowed => {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                if mode == ProfileMode::Audit {
                    // Audit mode: log all accesses
                    self.record_violation(profile_name, pid, vtype, resource, false);
                }
                AccessDecision::Allowed
            }
            AccessDecision::Denied | AccessDecision::PartialDeny => {
                match mode {
                    ProfileMode::Enforce => {
                        self.stats.total_denied = self.stats.total_denied.saturating_add(1);
                        self.record_violation(profile_name, pid, vtype, resource, true);
                        AccessDecision::Denied
                    }
                    ProfileMode::Complain | ProfileMode::Audit => {
                        self.stats.total_complained = self.stats.total_complained.saturating_add(1);
                        self.record_violation(profile_name, pid, vtype, resource, false);
                        // In complain/audit mode, allow despite the violation
                        AccessDecision::Allowed
                    }
                    ProfileMode::Disabled => {
                        self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                        AccessDecision::Allowed
                    }
                }
            }
        }
    }

    /// Record a violation
    fn record_violation(
        &mut self,
        profile_name: &str,
        pid: u32,
        vtype: ViolationType,
        resource: &str,
        enforced: bool,
    ) {
        // Trim violation log if full
        if self.violations.len() >= self.max_violations {
            // Remove oldest quarter
            let drain_count = self.max_violations / 4;
            self.violations.drain(0..drain_count);
        }

        if let Some(profile) = self.profiles.get_mut(profile_name) {
            profile.violation_count = profile.violation_count.saturating_add(1);
        }

        self.violations.push(Violation {
            profile_name: String::from(profile_name),
            pid,
            vtype,
            resource: String::from(resource),
            requested: String::new(),
            timestamp: 0, // filled in by caller if needed
            enforced,
        });
    }

    /// Get violations for a profile
    pub fn get_violations(&self, profile_name: &str) -> Vec<&Violation> {
        self.violations.iter().filter(|v| v.profile_name == profile_name).collect()
    }

    /// Get engine statistics
    pub fn stats(&self) -> &EngineStats {
        &self.stats
    }

    /// Get number of loaded profiles
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }

    /// List all profile names
    pub fn list_profiles(&self) -> Vec<&str> {
        self.profiles.keys().map(|k| k.as_str()).collect()
    }

    /// Enable or disable the engine globally
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

// ── Glob pattern matching ─────────────────────────────────────────────

/// Simple glob pattern matching for path rules
/// Supports `*` (any segment), `**` (any path), `?` (single char)
fn glob_match(pattern: &str, path: &str) -> bool {
    let pat = pattern.as_bytes();
    let txt = path.as_bytes();
    glob_match_recursive(pat, txt, 0, 0)
}

fn glob_match_recursive(pat: &[u8], txt: &[u8], mut pi: usize, mut ti: usize) -> bool {
    while pi < pat.len() && ti < txt.len() {
        match pat[pi] {
            b'?' => {
                // Match any single character
                pi += 1;
                ti += 1;
            }
            b'*' => {
                // Check for `**` (match across path separators)
                if pi + 1 < pat.len() && pat[pi + 1] == b'*' {
                    // `**` — match everything including `/`
                    pi += 2;
                    if pi < pat.len() && pat[pi] == b'/' {
                        pi += 1; // skip trailing slash in `**/`
                    }
                    // Try matching rest of pattern at every position
                    for i in ti..=txt.len() {
                        if glob_match_recursive(pat, txt, pi, i) {
                            return true;
                        }
                    }
                    return false;
                }
                // Single `*` — match within a single path segment (no `/`)
                pi += 1;
                for i in ti..=txt.len() {
                    if i > ti && i <= txt.len() && txt[i - 1] == b'/' {
                        break; // stop at path separator
                    }
                    if glob_match_recursive(pat, txt, pi, i) {
                        return true;
                    }
                }
                return false;
            }
            c => {
                if c != txt[ti] {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    // Skip trailing stars in pattern
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len() && ti == txt.len()
}

// ── Default profiles ──────────────────────────────────────────────────

/// Create a restrictive default profile for untrusted programs
pub fn default_untrusted_profile(name: &str) -> Profile {
    let mut p = Profile::new(String::from(name), ProfileMode::Enforce);
    // Allow read access to /lib and /usr/lib
    p.add_file_rule(String::from("/lib/**"), FilePerms::READ_ONLY, true);
    p.add_file_rule(String::from("/usr/lib/**"), FilePerms::READ_ONLY, true);
    // Allow read-exec for own binary directory
    p.add_file_rule(format!("{}/**", name), FilePerms::READ_EXEC, true);
    // Allow read/write to /tmp
    p.add_file_rule(String::from("/tmp/**"), FilePerms::READ_WRITE, true);
    // Deny everything else (implicit)
    // No network access
    // No capabilities
    p
}

/// Create a profile for system daemons
pub fn default_daemon_profile(name: &str) -> Profile {
    let mut p = Profile::new(String::from(name), ProfileMode::Enforce);
    // Read access to system directories
    p.add_file_rule(String::from("/lib/**"), FilePerms::READ_ONLY, true);
    p.add_file_rule(String::from("/usr/**"), FilePerms::READ_ONLY, true);
    p.add_file_rule(String::from("/etc/**"), FilePerms::READ_ONLY, true);
    // Read-write to daemon's own data directory
    p.add_file_rule(format!("/var/run/{}/**", name), FilePerms::READ_WRITE, true);
    p.add_file_rule(format!("/var/log/{}/**", name), FilePerms {
        read: true, write: true, execute: false, append: true,
        mmap_exec: false, create: true, delete: false, chown: false,
    }, true);
    // Allow TCP listen on high ports
    p.add_net_rule(NetRule {
        protocol: NetProtocol::Tcp,
        direction: NetDirection::Listen,
        address: 0,
        mask: 0,
        port: 1024,
        port_end: 65535,
        allow: true,
    });
    // Allow basic capabilities
    p.allow_cap(CapRule::NetBind);
    p
}

// ── Public API ────────────────────────────────────────────────────────

/// Load a profile into the global engine
pub fn load_profile(profile: Profile) -> bool {
    let mut guard = APPARMOR.lock();
    if let Some(engine) = guard.as_mut() {
        engine.load_profile(profile)
    } else {
        false
    }
}

/// Confine a process to a named profile
pub fn confine_process(pid: u32, profile_name: &str) -> bool {
    let mut guard = APPARMOR.lock();
    if let Some(engine) = guard.as_mut() {
        engine.confine_process(pid, profile_name)
    } else {
        false
    }
}

/// Check file access for a process
pub fn check_file(pid: u32, path: &str, perms: &FilePerms) -> AccessDecision {
    let mut guard = APPARMOR.lock();
    if let Some(engine) = guard.as_mut() {
        engine.check_file_access(pid, path, perms)
    } else {
        AccessDecision::Allowed
    }
}

/// Check network access for a process
pub fn check_net(pid: u32, proto: NetProtocol, addr: u32, port: u16, dir: NetDirection) -> AccessDecision {
    let mut guard = APPARMOR.lock();
    if let Some(engine) = guard.as_mut() {
        engine.check_net_access(pid, proto, addr, port, dir)
    } else {
        AccessDecision::Allowed
    }
}

/// Check capability for a process
pub fn check_cap(pid: u32, cap: CapRule) -> AccessDecision {
    let mut guard = APPARMOR.lock();
    if let Some(engine) = guard.as_mut() {
        engine.check_capability(pid, cap)
    } else {
        AccessDecision::Allowed
    }
}

/// Initialize the AppArmor engine
pub fn init() {
    let mut guard = APPARMOR.lock();
    *guard = Some(AppArmorEngine::new());
    serial_println!("  AppArmor: profile-based access control initialized");
}
