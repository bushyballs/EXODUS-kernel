/// SELinux-like type enforcement for Genesis
///
/// Mandatory access control using security contexts (labels):
///   - Every process has a domain (type)
///   - Every resource has a type
///   - Policy rules define what domains can do to what types
///   - Domain transitions control how processes change domains on exec
///
/// Components:
///   1. Type Enforcement (TE) — the core access control
///   2. Access Vector Cache (AVC) — fast lookup cache for policy decisions
///   3. Domain transitions — controlled privilege changes
///   4. Policy loading — compile and load policy rules
///
/// Inspired by: SELinux, SMACK. All code is original.

use crate::serial_println;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use crate::sync::Mutex;

/// Global SELinux engine
static SELINUX: Mutex<Option<SELinuxEngine>> = Mutex::new(None);

/// Maximum types in the system
const MAX_TYPES: usize = 1024;

/// Maximum rules in policy
const MAX_RULES: usize = 8192;

/// AVC cache size (must be power of 2)
const AVC_CACHE_SIZE: usize = 512;

/// AVC hash mask
const AVC_HASH_MASK: usize = AVC_CACHE_SIZE - 1;

// ── Security context ──────────────────────────────────────────────────

/// A security context — the label assigned to every subject and object
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecurityContext {
    /// User identity (e.g., "system_u", "user_u")
    pub user: String,
    /// Role (e.g., "system_r", "user_r")
    pub role: String,
    /// Type/domain (e.g., "httpd_t", "user_home_t")
    pub stype: String,
    /// Sensitivity level (MLS/MCS)
    pub level: u16,
    /// Category set (MLS/MCS) — bitmask of categories
    pub categories: u64,
}

impl SecurityContext {
    /// Create a new security context
    pub fn new(user: &str, role: &str, stype: &str, level: u16) -> Self {
        SecurityContext {
            user: String::from(user),
            role: String::from(role),
            stype: String::from(stype),
            level,
            categories: 0,
        }
    }

    /// Create a kernel context
    pub fn kernel() -> Self {
        Self::new("system_u", "system_r", "kernel_t", 0)
    }

    /// Create an init process context
    pub fn init() -> Self {
        Self::new("system_u", "system_r", "init_t", 0)
    }

    /// Create an unconfined context
    pub fn unconfined() -> Self {
        Self::new("system_u", "system_r", "unconfined_t", 0)
    }

    /// Check MLS dominance: does self dominate other?
    pub fn dominates(&self, other: &SecurityContext) -> bool {
        self.level >= other.level && (self.categories & other.categories) == other.categories
    }
}

// ── Object class ──────────────────────────────────────────────────────

/// Object classes — what kind of object is being accessed
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObjectClass {
    File,
    Directory,
    Socket,
    Process,
    Device,
    Pipe,
    SharedMemory,
    Semaphore,
    MessageQueue,
    Capability,
    System,
    Kernel,
}

// ── Access vector (permissions) ───────────────────────────────────────

/// Access vector — bitmask of permissions for an object class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessVector(pub u32);

impl AccessVector {
    pub const NONE: Self = AccessVector(0);

    // File permissions
    pub const FILE_READ: Self = AccessVector(0x0001);
    pub const FILE_WRITE: Self = AccessVector(0x0002);
    pub const FILE_EXECUTE: Self = AccessVector(0x0004);
    pub const FILE_CREATE: Self = AccessVector(0x0008);
    pub const FILE_UNLINK: Self = AccessVector(0x0010);
    pub const FILE_RENAME: Self = AccessVector(0x0020);
    pub const FILE_APPEND: Self = AccessVector(0x0040);
    pub const FILE_GETATTR: Self = AccessVector(0x0080);
    pub const FILE_SETATTR: Self = AccessVector(0x0100);
    pub const FILE_LOCK: Self = AccessVector(0x0200);
    pub const FILE_IOCTL: Self = AccessVector(0x0400);
    pub const FILE_MMAP: Self = AccessVector(0x0800);

    // Process permissions
    pub const PROC_FORK: Self = AccessVector(0x0001);
    pub const PROC_SIGNAL: Self = AccessVector(0x0002);
    pub const PROC_PTRACE: Self = AccessVector(0x0004);
    pub const PROC_TRANSITION: Self = AccessVector(0x0008);
    pub const PROC_EXECUTE: Self = AccessVector(0x0010);
    pub const PROC_SETCURRENT: Self = AccessVector(0x0020);

    // Socket permissions
    pub const SOCK_CONNECT: Self = AccessVector(0x0001);
    pub const SOCK_LISTEN: Self = AccessVector(0x0002);
    pub const SOCK_ACCEPT: Self = AccessVector(0x0004);
    pub const SOCK_BIND: Self = AccessVector(0x0008);
    pub const SOCK_SEND: Self = AccessVector(0x0010);
    pub const SOCK_RECV: Self = AccessVector(0x0020);

    /// Combine two access vectors
    pub fn union(self, other: AccessVector) -> AccessVector {
        AccessVector(self.0 | other.0)
    }

    /// Check if all bits in `required` are set
    pub fn contains(self, required: AccessVector) -> bool {
        (self.0 & required.0) == required.0
    }

    /// Check if any bits overlap
    pub fn intersects(self, other: AccessVector) -> bool {
        (self.0 & other.0) != 0
    }
}

// ── Type Enforcement rule ─────────────────────────────────────────────

/// Rule effect
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffect {
    /// Allow access
    Allow,
    /// Audit on allow (log successful access)
    AuditAllow,
    /// Deny access (explicit)
    Deny,
    /// Don't audit on deny (suppress denial logs)
    DontAudit,
}

/// A type enforcement rule
#[derive(Debug, Clone)]
pub struct TERule {
    /// Source type (domain)
    pub source: String,
    /// Target type
    pub target: String,
    /// Object class
    pub class: ObjectClass,
    /// Allowed permissions
    pub permissions: AccessVector,
    /// Rule effect
    pub effect: RuleEffect,
}

// ── Domain transition rule ────────────────────────────────────────────

/// Domain transition — when source domain execs target type, transition to new domain
#[derive(Debug, Clone)]
pub struct TransitionRule {
    /// Current domain
    pub source_domain: String,
    /// Executable file type
    pub exec_type: String,
    /// New domain after exec
    pub target_domain: String,
    /// Whether this is a default transition (auto) or requires explicit request
    pub auto_transition: bool,
}

// ── Type declaration ──────────────────────────────────────────────────

/// Type attributes
#[derive(Debug, Clone)]
pub struct TypeDecl {
    /// Type name
    pub name: String,
    /// Attributes this type has (e.g., "domain", "file_type")
    pub attributes: Vec<String>,
    /// Whether this is a domain (process) type
    pub is_domain: bool,
}

// ── Access Vector Cache (AVC) ─────────────────────────────────────────

/// AVC entry — cached policy decision
#[derive(Debug, Clone)]
struct AvcEntry {
    /// Source type
    source: String,
    /// Target type
    target: String,
    /// Object class
    class: ObjectClass,
    /// Allowed permissions (cached result)
    allowed: AccessVector,
    /// Denied permissions (cached result)
    denied: AccessVector,
    /// Audit on allow mask
    audit_allow: AccessVector,
    /// Don't audit on deny mask
    dont_audit: AccessVector,
    /// Entry is valid
    valid: bool,
    /// Hit count (for stats)
    hits: u64,
}

impl AvcEntry {
    const fn empty() -> Self {
        AvcEntry {
            source: String::new(),
            target: String::new(),
            class: ObjectClass::File,
            allowed: AccessVector::NONE,
            denied: AccessVector::NONE,
            audit_allow: AccessVector::NONE,
            dont_audit: AccessVector::NONE,
            valid: false,
            hits: 0,
        }
    }
}

/// AVC statistics
#[derive(Debug, Clone, Copy)]
pub struct AvcStats {
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

// ── SELinux Engine ────────────────────────────────────────────────────

/// SELinux enforcement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    /// Violations are blocked
    Enforcing,
    /// Violations are logged but allowed
    Permissive,
    /// SELinux is disabled
    Disabled,
}

/// Access check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessResult {
    Allowed,
    Denied,
    AuditAllow,
    DontAuditDeny,
}

/// The main SELinux engine
pub struct SELinuxEngine {
    /// Enforcement mode
    mode: EnforcementMode,
    /// Type declarations
    types: BTreeMap<String, TypeDecl>,
    /// Type enforcement rules
    te_rules: Vec<TERule>,
    /// Domain transition rules
    transitions: Vec<TransitionRule>,
    /// Process security contexts (PID -> context)
    process_contexts: BTreeMap<u32, SecurityContext>,
    /// File security contexts (path -> context)
    file_contexts: BTreeMap<String, SecurityContext>,
    /// Access vector cache
    avc: Vec<AvcEntry>,
    /// AVC statistics
    avc_stats: AvcStats,
    /// Policy version
    policy_version: u32,
    /// Policy loaded flag
    policy_loaded: bool,
    /// Denial log (ring buffer)
    denials: Vec<Denial>,
    /// Max denial log entries
    max_denials: usize,
    /// Engine statistics
    stats: EngineStats,
}

/// A logged denial
#[derive(Debug, Clone)]
pub struct Denial {
    /// Source context
    pub source: SecurityContext,
    /// Target context
    pub target: SecurityContext,
    /// Object class
    pub class: ObjectClass,
    /// Requested permissions
    pub requested: AccessVector,
    /// Permissions that were denied
    pub denied: AccessVector,
    /// PID of the process
    pub pid: u32,
    /// Timestamp
    pub timestamp: u64,
    /// Whether it was enforced (blocked) or permissive (allowed)
    pub enforced: bool,
}

/// Engine statistics
#[derive(Debug, Clone, Copy)]
pub struct EngineStats {
    pub total_checks: u64,
    pub total_allowed: u64,
    pub total_denied: u64,
    pub types_defined: u32,
    pub rules_loaded: u32,
    pub transitions_defined: u32,
}

impl SELinuxEngine {
    /// Create a new SELinux engine
    pub fn new(mode: EnforcementMode) -> Self {
        let mut avc = Vec::with_capacity(AVC_CACHE_SIZE);
        for _ in 0..AVC_CACHE_SIZE {
            avc.push(AvcEntry::empty());
        }
        SELinuxEngine {
            mode,
            types: BTreeMap::new(),
            te_rules: Vec::new(),
            transitions: Vec::new(),
            process_contexts: BTreeMap::new(),
            file_contexts: BTreeMap::new(),
            avc,
            avc_stats: AvcStats { lookups: 0, hits: 0, misses: 0, evictions: 0 },
            policy_version: 0,
            policy_loaded: false,
            denials: Vec::new(),
            max_denials: 2048,
            stats: EngineStats {
                total_checks: 0, total_allowed: 0, total_denied: 0,
                types_defined: 0, rules_loaded: 0, transitions_defined: 0,
            },
        }
    }

    /// Declare a new type
    pub fn declare_type(&mut self, name: &str, attributes: Vec<String>, is_domain: bool) -> bool {
        if self.types.len() >= MAX_TYPES {
            return false;
        }
        self.types.insert(String::from(name), TypeDecl {
            name: String::from(name),
            attributes,
            is_domain,
        });
        self.stats.types_defined = self.types.len() as u32;
        true
    }

    /// Add a type enforcement rule
    pub fn add_te_rule(&mut self, rule: TERule) -> bool {
        if self.te_rules.len() >= MAX_RULES {
            return false;
        }
        self.te_rules.push(rule);
        self.stats.rules_loaded = self.te_rules.len() as u32;
        // Invalidate AVC since policy changed
        self.avc_flush();
        true
    }

    /// Add a domain transition rule
    pub fn add_transition(&mut self, rule: TransitionRule) -> bool {
        self.transitions.push(rule);
        self.stats.transitions_defined = self.transitions.len() as u32;
        true
    }

    /// Assign a security context to a process
    pub fn set_process_context(&mut self, pid: u32, ctx: SecurityContext) {
        self.process_contexts.insert(pid, ctx);
    }

    /// Get a process's security context
    pub fn get_process_context(&self, pid: u32) -> Option<&SecurityContext> {
        self.process_contexts.get(&pid)
    }

    /// Assign a security context to a file path
    pub fn set_file_context(&mut self, path: &str, ctx: SecurityContext) {
        self.file_contexts.insert(String::from(path), ctx);
    }

    /// Get a file's security context (longest prefix match)
    pub fn get_file_context(&self, path: &str) -> Option<&SecurityContext> {
        // Try exact match first
        if let Some(ctx) = self.file_contexts.get(path) {
            return Some(ctx);
        }
        // Try longest prefix match
        let mut best: Option<&SecurityContext> = None;
        let mut best_len = 0;
        for (prefix, ctx) in &self.file_contexts {
            if path.starts_with(prefix.as_str()) && prefix.len() > best_len {
                best = Some(ctx);
                best_len = prefix.len();
            }
        }
        best
    }

    /// Core access check: can source_type access target_type with given permissions?
    pub fn check_access(
        &mut self,
        pid: u32,
        source: &SecurityContext,
        target: &SecurityContext,
        class: ObjectClass,
        requested: AccessVector,
    ) -> AccessResult {
        self.stats.total_checks = self.stats.total_checks.saturating_add(1);

        if self.mode == EnforcementMode::Disabled {
            self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
            return AccessResult::Allowed;
        }

        // Check AVC first
        if let Some(result) = self.avc_lookup(&source.stype, &target.stype, class, requested) {
            return self.apply_result(result, pid, source, target, class, requested);
        }

        // AVC miss — compute from policy
        let result = self.compute_access(&source.stype, &target.stype, class, requested);

        // Cache the result
        self.avc_insert(&source.stype, &target.stype, class, result);

        self.apply_result(result, pid, source, target, class, requested)
    }

    /// Compute access decision from policy rules (AVC miss path)
    fn compute_access(
        &self,
        source_type: &str,
        target_type: &str,
        class: ObjectClass,
        requested: AccessVector,
    ) -> AccessResult {
        let mut allowed = AccessVector::NONE;
        let mut audit_allow = AccessVector::NONE;
        let mut dont_audit = AccessVector::NONE;
        let mut explicit_deny = false;

        for rule in &self.te_rules {
            // Check if rule matches source and target types
            if !self.type_matches(&rule.source, source_type) {
                continue;
            }
            if !self.type_matches(&rule.target, target_type) {
                continue;
            }
            if rule.class != class {
                continue;
            }

            match rule.effect {
                RuleEffect::Allow => {
                    allowed = allowed.union(rule.permissions);
                }
                RuleEffect::AuditAllow => {
                    allowed = allowed.union(rule.permissions);
                    audit_allow = audit_allow.union(rule.permissions);
                }
                RuleEffect::Deny => {
                    if rule.permissions.intersects(requested) {
                        explicit_deny = true;
                    }
                }
                RuleEffect::DontAudit => {
                    dont_audit = dont_audit.union(rule.permissions);
                }
            }
        }

        if explicit_deny {
            if dont_audit.contains(requested) {
                return AccessResult::DontAuditDeny;
            }
            return AccessResult::Denied;
        }

        if allowed.contains(requested) {
            if audit_allow.intersects(requested) {
                return AccessResult::AuditAllow;
            }
            return AccessResult::Allowed;
        }

        // Default deny
        if dont_audit.contains(requested) {
            AccessResult::DontAuditDeny
        } else {
            AccessResult::Denied
        }
    }

    /// Check if a type name matches (exact or attribute-based)
    fn type_matches(&self, rule_type: &str, actual_type: &str) -> bool {
        if rule_type == actual_type {
            return true;
        }
        // Check if rule_type is an attribute that actual_type has
        if let Some(decl) = self.types.get(actual_type) {
            for attr in &decl.attributes {
                if attr == rule_type {
                    return true;
                }
            }
        }
        false
    }

    /// Apply result considering enforcement mode
    fn apply_result(
        &mut self,
        result: AccessResult,
        pid: u32,
        source: &SecurityContext,
        target: &SecurityContext,
        class: ObjectClass,
        requested: AccessVector,
    ) -> AccessResult {
        match result {
            AccessResult::Allowed => {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                AccessResult::Allowed
            }
            AccessResult::AuditAllow => {
                self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                // Log the allowed access for auditing
                self.log_denial(source, target, class, requested, AccessVector::NONE, pid, false);
                AccessResult::Allowed
            }
            AccessResult::Denied => {
                self.log_denial(source, target, class, requested, requested, pid,
                    self.mode == EnforcementMode::Enforcing);
                if self.mode == EnforcementMode::Enforcing {
                    self.stats.total_denied = self.stats.total_denied.saturating_add(1);
                    AccessResult::Denied
                } else {
                    self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                    AccessResult::Allowed
                }
            }
            AccessResult::DontAuditDeny => {
                // Deny without logging
                if self.mode == EnforcementMode::Enforcing {
                    self.stats.total_denied = self.stats.total_denied.saturating_add(1);
                    AccessResult::Denied
                } else {
                    self.stats.total_allowed = self.stats.total_allowed.saturating_add(1);
                    AccessResult::Allowed
                }
            }
        }
    }

    /// Log a denial
    fn log_denial(
        &mut self,
        source: &SecurityContext,
        target: &SecurityContext,
        class: ObjectClass,
        requested: AccessVector,
        denied: AccessVector,
        pid: u32,
        enforced: bool,
    ) {
        if self.denials.len() >= self.max_denials {
            let drain_count = self.max_denials / 4;
            self.denials.drain(0..drain_count);
        }
        self.denials.push(Denial {
            source: source.clone(),
            target: target.clone(),
            class,
            requested,
            denied,
            pid,
            timestamp: 0,
            enforced,
        });
    }

    /// Handle exec transition: determine new domain when process execs a file
    pub fn handle_exec_transition(
        &mut self,
        pid: u32,
        exec_file_type: &str,
    ) -> Option<SecurityContext> {
        let current_ctx = self.process_contexts.get(&pid)?.clone();
        let current_domain = &current_ctx.stype;

        for rule in &self.transitions {
            if rule.source_domain == *current_domain && rule.exec_type == exec_file_type {
                let new_ctx = SecurityContext {
                    user: current_ctx.user.clone(),
                    role: current_ctx.role.clone(),
                    stype: rule.target_domain.clone(),
                    level: current_ctx.level,
                    categories: current_ctx.categories,
                };
                self.process_contexts.insert(pid, new_ctx.clone());
                serial_println!("  SELinux: domain transition {} -> {} (pid {})",
                    current_domain, rule.target_domain, pid);
                return Some(new_ctx);
            }
        }
        None
    }

    // ── AVC operations ────────────────────────────────────────────────

    /// Hash function for AVC lookup
    fn avc_hash(source: &str, target: &str, class: ObjectClass) -> usize {
        let mut h: u32 = 0x811C_9DC5; // FNV-1a offset basis
        for b in source.as_bytes() {
            h ^= *b as u32;
            h = h.wrapping_mul(0x0100_0193); // FNV prime
        }
        for b in target.as_bytes() {
            h ^= *b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        h ^= class as u32;
        h = h.wrapping_mul(0x0100_0193);
        (h as usize) & AVC_HASH_MASK
    }

    /// Look up in AVC
    fn avc_lookup(
        &mut self,
        source: &str,
        target: &str,
        class: ObjectClass,
        requested: AccessVector,
    ) -> Option<AccessResult> {
        self.avc_stats.lookups = self.avc_stats.lookups.saturating_add(1);
        let idx = Self::avc_hash(source, target, class);
        let entry = &mut self.avc[idx];
        if entry.valid && entry.source == source && entry.target == target && entry.class == class {
            self.avc_stats.hits = self.avc_stats.hits.saturating_add(1);
            entry.hits = entry.hits.saturating_add(1);
            // Reconstruct the result from cached vectors
            if entry.denied.intersects(requested) {
                if entry.dont_audit.contains(requested) {
                    Some(AccessResult::DontAuditDeny)
                } else {
                    Some(AccessResult::Denied)
                }
            } else if entry.allowed.contains(requested) {
                if entry.audit_allow.intersects(requested) {
                    Some(AccessResult::AuditAllow)
                } else {
                    Some(AccessResult::Allowed)
                }
            } else {
                // Requested permissions not fully covered by cached entry
                None
            }
        } else {
            self.avc_stats.misses = self.avc_stats.misses.saturating_add(1);
            None
        }
    }

    /// Insert into AVC
    fn avc_insert(&mut self, source: &str, target: &str, class: ObjectClass, result: AccessResult) {
        let idx = Self::avc_hash(source, target, class);
        if self.avc[idx].valid {
            self.avc_stats.evictions = self.avc_stats.evictions.saturating_add(1);
        }
        // Compute full allowed/denied vectors for this source/target/class
        let mut allowed = AccessVector::NONE;
        let mut audit_allow = AccessVector::NONE;
        let mut dont_audit = AccessVector::NONE;
        let mut denied = AccessVector::NONE;

        for rule in &self.te_rules {
            if !self.type_matches(&rule.source, source) { continue; }
            if !self.type_matches(&rule.target, target) { continue; }
            if rule.class != class { continue; }

            match rule.effect {
                RuleEffect::Allow => { allowed = allowed.union(rule.permissions); }
                RuleEffect::AuditAllow => {
                    allowed = allowed.union(rule.permissions);
                    audit_allow = audit_allow.union(rule.permissions);
                }
                RuleEffect::Deny => { denied = denied.union(rule.permissions); }
                RuleEffect::DontAudit => { dont_audit = dont_audit.union(rule.permissions); }
            }
        }

        self.avc[idx] = AvcEntry {
            source: String::from(source),
            target: String::from(target),
            class,
            allowed,
            denied,
            audit_allow,
            dont_audit,
            valid: true,
            hits: 0,
        };
    }

    /// Flush entire AVC (after policy change)
    fn avc_flush(&mut self) {
        for entry in self.avc.iter_mut() {
            entry.valid = false;
        }
    }

    /// Get AVC statistics
    pub fn avc_stats(&self) -> &AvcStats {
        &self.avc_stats
    }

    // ── Policy loading ────────────────────────────────────────────────

    /// Load a complete policy (replaces existing)
    pub fn load_policy(&mut self, types: Vec<TypeDecl>, rules: Vec<TERule>, transitions: Vec<TransitionRule>) {
        self.types.clear();
        for t in types {
            self.types.insert(t.name.clone(), t);
        }
        self.te_rules = rules;
        self.transitions = transitions;
        self.policy_version = self.policy_version.saturating_add(1);
        self.policy_loaded = true;
        self.avc_flush();
        self.stats.types_defined = self.types.len() as u32;
        self.stats.rules_loaded = self.te_rules.len() as u32;
        self.stats.transitions_defined = self.transitions.len() as u32;
        serial_println!("  SELinux: policy v{} loaded ({} types, {} rules, {} transitions)",
            self.policy_version, self.stats.types_defined,
            self.stats.rules_loaded, self.stats.transitions_defined);
    }

    /// Load the default boot policy
    pub fn load_default_policy(&mut self) {
        // Declare core types
        let types = vec![
            TypeDecl { name: String::from("kernel_t"), attributes: vec![String::from("domain")], is_domain: true },
            TypeDecl { name: String::from("init_t"), attributes: vec![String::from("domain")], is_domain: true },
            TypeDecl { name: String::from("unconfined_t"), attributes: vec![String::from("domain")], is_domain: true },
            TypeDecl { name: String::from("user_t"), attributes: vec![String::from("domain")], is_domain: true },
            TypeDecl { name: String::from("daemon_t"), attributes: vec![String::from("domain")], is_domain: true },
            TypeDecl { name: String::from("bin_t"), attributes: vec![String::from("file_type")], is_domain: false },
            TypeDecl { name: String::from("etc_t"), attributes: vec![String::from("file_type")], is_domain: false },
            TypeDecl { name: String::from("var_t"), attributes: vec![String::from("file_type")], is_domain: false },
            TypeDecl { name: String::from("tmp_t"), attributes: vec![String::from("file_type")], is_domain: false },
            TypeDecl { name: String::from("home_t"), attributes: vec![String::from("file_type")], is_domain: false },
            TypeDecl { name: String::from("port_t"), attributes: vec![String::from("net_type")], is_domain: false },
            TypeDecl { name: String::from("node_t"), attributes: vec![String::from("net_type")], is_domain: false },
        ];

        // Core TE rules
        let rules = vec![
            // kernel_t can do everything
            TERule { source: String::from("kernel_t"), target: String::from("file_type"),
                class: ObjectClass::File, permissions: AccessVector(0x0FFF), effect: RuleEffect::Allow },
            TERule { source: String::from("kernel_t"), target: String::from("domain"),
                class: ObjectClass::Process, permissions: AccessVector(0x003F), effect: RuleEffect::Allow },
            // init_t has broad access
            TERule { source: String::from("init_t"), target: String::from("file_type"),
                class: ObjectClass::File,
                permissions: AccessVector::FILE_READ.union(AccessVector::FILE_EXECUTE).union(AccessVector::FILE_GETATTR),
                effect: RuleEffect::Allow },
            TERule { source: String::from("init_t"), target: String::from("domain"),
                class: ObjectClass::Process, permissions: AccessVector::PROC_FORK.union(AccessVector::PROC_SIGNAL),
                effect: RuleEffect::Allow },
            // user_t can read most files, write to home and tmp
            TERule { source: String::from("user_t"), target: String::from("bin_t"),
                class: ObjectClass::File,
                permissions: AccessVector::FILE_READ.union(AccessVector::FILE_EXECUTE).union(AccessVector::FILE_GETATTR),
                effect: RuleEffect::Allow },
            TERule { source: String::from("user_t"), target: String::from("etc_t"),
                class: ObjectClass::File,
                permissions: AccessVector::FILE_READ.union(AccessVector::FILE_GETATTR),
                effect: RuleEffect::Allow },
            TERule { source: String::from("user_t"), target: String::from("home_t"),
                class: ObjectClass::File, permissions: AccessVector(0x0FFF), effect: RuleEffect::Allow },
            TERule { source: String::from("user_t"), target: String::from("tmp_t"),
                class: ObjectClass::File, permissions: AccessVector(0x0FFF), effect: RuleEffect::Allow },
            // unconfined_t has full access (for transition period)
            TERule { source: String::from("unconfined_t"), target: String::from("file_type"),
                class: ObjectClass::File, permissions: AccessVector(0x0FFF), effect: RuleEffect::Allow },
            TERule { source: String::from("unconfined_t"), target: String::from("domain"),
                class: ObjectClass::Process, permissions: AccessVector(0x003F), effect: RuleEffect::Allow },
        ];

        // Default transitions
        let transitions = vec![
            TransitionRule {
                source_domain: String::from("init_t"),
                exec_type: String::from("daemon_exec_t"),
                target_domain: String::from("daemon_t"),
                auto_transition: true,
            },
            TransitionRule {
                source_domain: String::from("init_t"),
                exec_type: String::from("user_exec_t"),
                target_domain: String::from("user_t"),
                auto_transition: true,
            },
        ];

        self.load_policy(types, rules, transitions);

        // Set PID 0 (kernel) context
        self.set_process_context(0, SecurityContext::kernel());
    }

    /// Get engine statistics
    pub fn stats(&self) -> &EngineStats {
        &self.stats
    }

    /// Get enforcement mode
    pub fn enforcement_mode(&self) -> EnforcementMode {
        self.mode
    }

    /// Set enforcement mode
    pub fn set_mode(&mut self, mode: EnforcementMode) {
        self.mode = mode;
    }

    /// Get recent denials
    pub fn recent_denials(&self, count: usize) -> &[Denial] {
        let start = if self.denials.len() > count {
            self.denials.len() - count
        } else {
            0
        };
        &self.denials[start..]
    }
}

// ── Public API ────────────────────────────────────────────────────────

/// Check access for a process against a target
pub fn check_access(
    pid: u32,
    target_type: &str,
    class: ObjectClass,
    requested: AccessVector,
) -> AccessResult {
    let mut guard = SELINUX.lock();
    if let Some(engine) = guard.as_mut() {
        let source = match engine.get_process_context(pid) {
            Some(ctx) => ctx.clone(),
            None => SecurityContext::unconfined(),
        };
        let target = SecurityContext::new("system_u", "object_r", target_type, 0);
        engine.check_access(pid, &source, &target, class, requested)
    } else {
        AccessResult::Allowed
    }
}

/// Set the security context for a process
pub fn set_context(pid: u32, ctx: SecurityContext) {
    let mut guard = SELINUX.lock();
    if let Some(engine) = guard.as_mut() {
        engine.set_process_context(pid, ctx);
    }
}

/// Handle exec transition for a process
pub fn exec_transition(pid: u32, exec_type: &str) -> Option<SecurityContext> {
    let mut guard = SELINUX.lock();
    if let Some(engine) = guard.as_mut() {
        engine.handle_exec_transition(pid, exec_type)
    } else {
        None
    }
}

/// Get the current enforcement mode
pub fn get_mode() -> EnforcementMode {
    let guard = SELINUX.lock();
    if let Some(engine) = guard.as_ref() {
        engine.enforcement_mode()
    } else {
        EnforcementMode::Disabled
    }
}

/// Initialize the SELinux engine
pub fn init() {
    let mut guard = SELINUX.lock();
    let mut engine = SELinuxEngine::new(EnforcementMode::Permissive);
    engine.load_default_policy();
    *guard = Some(engine);
    serial_println!("  SELinux: type enforcement initialized (permissive mode)");
}
