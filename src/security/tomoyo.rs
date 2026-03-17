/// TOMOYO LSM for Genesis — pathname-based mandatory access control
///
/// Implements pathname-based MAC with domain transition tracking:
///   - Domains are defined by process execution history (execution path chain)
///   - Each domain has explicit allow-lists for file/network/mount operations
///   - Policy modes: disabled, learning, permissive (audit), enforcing
///   - Learning mode automatically builds policy from observed behavior
///   - Domain transitions occur on execve() to build execution chains
///   - Supports wildcard path patterns (\*, \@, \$)
///   - Per-domain access statistics and audit trail
///
/// Reference: Linux TOMOYO LSM (Documentation/admin-guide/LSM/tomoyo.rst).
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static TOMOYO: Mutex<Option<TomoyoInner>> = Mutex::new(None);

/// Maximum domains
const MAX_DOMAINS: usize = 2048;

/// Maximum rules per domain
const MAX_RULES_PER_DOMAIN: usize = 512;

/// Maximum audit log entries
const MAX_AUDIT_ENTRIES: usize = 4096;

/// Permission bits
pub const PERM_READ: u8 = 1 << 0;
pub const PERM_WRITE: u8 = 1 << 1;
pub const PERM_EXEC: u8 = 1 << 2;
pub const PERM_CREATE: u8 = 1 << 3;
pub const PERM_UNLINK: u8 = 1 << 4;
pub const PERM_MKDIR: u8 = 1 << 5;
pub const PERM_RMDIR: u8 = 1 << 6;
pub const PERM_MOUNT: u8 = 1 << 7;

/// TOMOYO policy mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyMode {
    /// Security features disabled
    Disabled,
    /// Automatically learn allowed operations (build policy)
    Learning,
    /// Log violations but allow them (permissive/audit)
    Permissive,
    /// Enforce policy strictly (deny violations)
    Enforcing,
}

/// Path rule type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleType {
    /// File access rule
    File,
    /// Network access rule
    Network,
    /// Mount operation rule
    Mount,
    /// Environment variable access
    Env,
    /// Domain transition rule
    Transition,
}

/// A single access rule within a domain
#[derive(Clone)]
struct AccessRule {
    rule_type: RuleType,
    /// Path pattern (supports wildcards)
    path_pattern: String,
    /// Allowed permissions bitmask
    permissions: u8,
    /// Whether this rule was auto-learned
    learned: bool,
    /// Number of times this rule was matched
    hit_count: u64,
}

/// TOMOYO domain (execution history chain)
#[derive(Clone)]
pub struct Domain {
    /// Full domain path (e.g., "<kernel> /usr/sbin/init /usr/bin/sshd")
    pub path: String,
    /// Access rules for this domain
    rules: Vec<AccessRule>,
    /// Policy mode override for this domain (None = use global)
    mode_override: Option<PolicyMode>,
    /// Parent domain path
    parent_path: Option<String>,
    /// Process IDs in this domain
    pids: Vec<u32>,
    /// Statistics
    access_checks: u64,
    access_denials: u64,
}

/// Audit log entry
struct AuditEntry {
    domain: String,
    path: String,
    permissions: u8,
    rule_type: RuleType,
    granted: bool,
    pid: u32,
    mode: PolicyMode,
}

/// Inner TOMOYO state
struct TomoyoInner {
    /// All domains
    domains: Vec<Domain>,
    /// Global policy mode
    global_mode: PolicyMode,
    /// PID to domain index mapping
    pid_to_domain: Vec<(u32, usize)>,
    /// Audit log
    audit_log: Vec<AuditEntry>,
    /// Statistics
    total_checks: u64,
    total_denials: u64,
    total_learned: u64,
    domains_created: u64,
}

impl TomoyoInner {
    fn new() -> Self {
        TomoyoInner {
            domains: Vec::with_capacity(64),
            global_mode: PolicyMode::Permissive,
            pid_to_domain: Vec::new(),
            audit_log: Vec::with_capacity(256),
            total_checks: 0,
            total_denials: 0,
            total_learned: 0,
            domains_created: 0,
        }
    }

    /// Initialize with the kernel root domain
    fn init_root_domain(&mut self) {
        let root = Domain {
            path: String::from("<kernel>"),
            rules: Vec::new(),
            mode_override: None,
            parent_path: None,
            pids: Vec::new(),
            access_checks: 0,
            access_denials: 0,
        };
        self.domains.push(root);
        self.domains_created = self.domains_created.saturating_add(1);
    }

    /// Find or create a domain by path
    fn find_or_create_domain(&mut self, domain_path: &str) -> usize {
        // Search existing domains
        for (i, domain) in self.domains.iter().enumerate() {
            if domain.path == domain_path {
                return i;
            }
        }

        // Create new domain
        if self.domains.len() >= MAX_DOMAINS {
            serial_println!("    [tomoyo] WARNING: max domains reached, reusing last");
            return self.domains.len() - 1;
        }

        // Determine parent path
        let parent_path = if let Some(last_space) = domain_path.rfind(' ') {
            Some(String::from(&domain_path[..last_space]))
        } else {
            None
        };

        let domain = Domain {
            path: String::from(domain_path),
            rules: Vec::new(),
            mode_override: None,
            parent_path,
            pids: Vec::new(),
            access_checks: 0,
            access_denials: 0,
        };

        self.domains.push(domain);
        self.domains_created = self.domains_created.saturating_add(1);
        let idx = self.domains.len() - 1;

        serial_println!("    [tomoyo] New domain created: {}", domain_path);
        idx
    }

    /// Get the effective policy mode for a domain
    fn effective_mode(&self, domain_idx: usize) -> PolicyMode {
        if domain_idx < self.domains.len() {
            if let Some(mode) = self.domains[domain_idx].mode_override {
                return mode;
            }
        }
        self.global_mode
    }

    /// Handle a domain transition (execve)
    fn domain_transition(&mut self, pid: u32, exec_path: &str) {
        // Find current domain for this PID
        let current_domain_path = self.get_domain_path_for_pid(pid);

        // Build new domain path: current_domain + " " + exec_path
        let new_domain_path = if let Some(ref current) = current_domain_path {
            format!("{} {}", current, exec_path)
        } else {
            format!("<kernel> {}", exec_path)
        };

        // Find or create the new domain
        let new_idx = self.find_or_create_domain(&new_domain_path);

        // Check if domain transition is allowed in current domain
        if let Some(ref current) = current_domain_path {
            let current_idx = self.find_domain_index(current);
            if let Some(ci) = current_idx {
                let mode = self.effective_mode(ci);
                if mode == PolicyMode::Enforcing {
                    // Check for explicit transition rule
                    if !self.check_rule_exists(ci, exec_path, PERM_EXEC, RuleType::Transition) {
                        serial_println!(
                            "    [tomoyo] DENIED transition: {} -> {} (enforcing)",
                            current,
                            new_domain_path
                        );
                        crate::security::audit::log(
                            crate::security::audit::AuditEvent::MacDenied,
                            crate::security::audit::AuditResult::Deny,
                            pid,
                            0,
                            &format!("tomoyo: transition denied {} -> {}", current, exec_path),
                        );
                        return;
                    }
                } else if mode == PolicyMode::Learning {
                    // Auto-add transition rule
                    self.learn_rule(ci, exec_path, PERM_EXEC, RuleType::Transition);
                }
            }
        }

        // Remove PID from old domain
        for domain in &mut self.domains {
            domain.pids.retain(|p| *p != pid);
        }

        // Add PID to new domain
        if new_idx < self.domains.len() {
            self.domains[new_idx].pids.push(pid);
        }

        // Update PID mapping
        self.pid_to_domain.retain(|(p, _)| *p != pid);
        self.pid_to_domain.push((pid, new_idx));
    }

    /// Get domain path for a PID
    fn get_domain_path_for_pid(&self, pid: u32) -> Option<String> {
        self.pid_to_domain
            .iter()
            .find(|(p, _)| *p == pid)
            .and_then(|(_, idx)| self.domains.get(*idx))
            .map(|d| d.path.clone())
    }

    /// Find domain index by path
    fn find_domain_index(&self, path: &str) -> Option<usize> {
        self.domains.iter().position(|d| d.path == path)
    }

    /// Check if a specific rule exists in a domain
    fn check_rule_exists(
        &self,
        domain_idx: usize,
        path: &str,
        perm: u8,
        rule_type: RuleType,
    ) -> bool {
        if domain_idx >= self.domains.len() {
            return false;
        }
        let domain = &self.domains[domain_idx];
        domain.rules.iter().any(|r| {
            r.rule_type == rule_type
                && (r.permissions & perm) == perm
                && path_matches_pattern(path, &r.path_pattern)
        })
    }

    /// Learn (auto-add) a rule
    fn learn_rule(&mut self, domain_idx: usize, path: &str, perm: u8, rule_type: RuleType) {
        if domain_idx >= self.domains.len() {
            return;
        }

        let domain = &mut self.domains[domain_idx];

        // Check if rule already exists
        for rule in &mut domain.rules {
            if rule.rule_type == rule_type && rule.path_pattern == path {
                rule.permissions |= perm;
                return;
            }
        }

        // Add new rule
        if domain.rules.len() >= MAX_RULES_PER_DOMAIN {
            return;
        }

        domain.rules.push(AccessRule {
            rule_type,
            path_pattern: String::from(path),
            permissions: perm,
            learned: true,
            hit_count: 0,
        });

        self.total_learned = self.total_learned.saturating_add(1);
        serial_println!(
            "    [tomoyo] LEARNED: {} {:?} perm=0x{:02X} in {}",
            path,
            rule_type,
            perm,
            domain.path
        );
    }

    /// Add an explicit policy rule
    fn add_rule(&mut self, domain_path: &str, path: &str, perm: u8, rule_type: RuleType) {
        let idx = self.find_or_create_domain(domain_path);
        if idx >= self.domains.len() {
            return;
        }

        let domain = &mut self.domains[idx];

        // Update existing rule if present
        for rule in &mut domain.rules {
            if rule.rule_type == rule_type && rule.path_pattern == path {
                rule.permissions |= perm;
                rule.learned = false;
                return;
            }
        }

        if domain.rules.len() >= MAX_RULES_PER_DOMAIN {
            return;
        }

        domain.rules.push(AccessRule {
            rule_type,
            path_pattern: String::from(path),
            permissions: perm,
            learned: false,
            hit_count: 0,
        });
    }

    /// Check file access for a process
    fn check_access(&mut self, pid: u32, path: &str, perm: u8) -> bool {
        self.total_checks = self.total_checks.saturating_add(1);

        // Find domain for this PID
        let domain_idx = self
            .pid_to_domain
            .iter()
            .find(|(p, _)| *p == pid)
            .map(|(_, idx)| *idx);

        let domain_idx = match domain_idx {
            Some(i) => i,
            None => return true, // No domain = unrestricted
        };

        let mode = self.effective_mode(domain_idx);

        if mode == PolicyMode::Disabled {
            return true;
        }

        // Check rules in the domain
        let mut allowed = false;
        if domain_idx < self.domains.len() {
            let domain = &mut self.domains[domain_idx];
            domain.access_checks = domain.access_checks.saturating_add(1);

            for rule in &mut domain.rules {
                if rule.rule_type == RuleType::File
                    && (rule.permissions & perm) == perm
                    && path_matches_pattern(path, &rule.path_pattern)
                {
                    rule.hit_count = rule.hit_count.saturating_add(1);
                    allowed = true;
                    break;
                }
            }
        }

        if !allowed {
            match mode {
                PolicyMode::Learning => {
                    // Auto-learn the rule
                    self.learn_rule(domain_idx, path, perm, RuleType::File);
                    allowed = true;
                }
                PolicyMode::Permissive => {
                    // Log but allow
                    self.total_denials = self.total_denials.saturating_add(1);
                    if domain_idx < self.domains.len() {
                        self.domains[domain_idx].access_denials =
                            self.domains[domain_idx].access_denials.saturating_add(1);
                    }
                    self.log_audit(domain_idx, path, perm, RuleType::File, false, pid, mode);
                    // Permissive: allow anyway
                    allowed = true;
                }
                PolicyMode::Enforcing => {
                    // Deny
                    self.total_denials = self.total_denials.saturating_add(1);
                    if domain_idx < self.domains.len() {
                        self.domains[domain_idx].access_denials =
                            self.domains[domain_idx].access_denials.saturating_add(1);
                    }
                    self.log_audit(domain_idx, path, perm, RuleType::File, false, pid, mode);
                    allowed = false;
                }
                PolicyMode::Disabled => {
                    allowed = true;
                }
            }
        } else {
            self.log_audit(domain_idx, path, perm, RuleType::File, true, pid, mode);
        }

        allowed
    }

    /// Log an audit entry
    fn log_audit(
        &mut self,
        domain_idx: usize,
        path: &str,
        perm: u8,
        rule_type: RuleType,
        granted: bool,
        pid: u32,
        mode: PolicyMode,
    ) {
        if self.audit_log.len() >= MAX_AUDIT_ENTRIES {
            self.audit_log.remove(0);
        }

        let domain_path = if domain_idx < self.domains.len() {
            self.domains[domain_idx].path.clone()
        } else {
            String::from("<unknown>")
        };

        self.audit_log.push(AuditEntry {
            domain: domain_path.clone(),
            path: String::from(path),
            permissions: perm,
            rule_type,
            granted,
            pid,
            mode,
        });

        if !granted {
            serial_println!(
                "    [tomoyo] {} access to {} perm=0x{:02X} in {} (pid={}, mode={:?})",
                if mode == PolicyMode::Enforcing {
                    "DENIED"
                } else {
                    "WOULD_DENY"
                },
                path,
                perm,
                domain_path,
                pid,
                mode
            );

            crate::security::audit::log(
                crate::security::audit::AuditEvent::MacCheck,
                if mode == PolicyMode::Enforcing {
                    crate::security::audit::AuditResult::Deny
                } else {
                    crate::security::audit::AuditResult::Info
                },
                pid,
                0,
                &format!("tomoyo: {} perm=0x{:02X} in {}", path, perm, domain_path),
            );
        }
    }

    /// Unregister a process
    fn unregister_process(&mut self, pid: u32) {
        if let Some((_, idx)) = self.pid_to_domain.iter().find(|(p, _)| *p == pid) {
            let idx = *idx;
            if idx < self.domains.len() {
                self.domains[idx].pids.retain(|p| *p != pid);
            }
        }
        self.pid_to_domain.retain(|(p, _)| *p != pid);
    }
}

/// Check if a path matches a TOMOYO pattern
/// Supports: * (any filename component), @ (any single char), exact match
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    // Exact match
    if path == pattern {
        return true;
    }

    // Wildcard matching
    let path_parts: Vec<&str> = path.split('/').collect();
    let pattern_parts: Vec<&str> = pattern.split('/').collect();

    if path_parts.len() != pattern_parts.len() {
        // Check for trailing wildcard
        if pattern.ends_with("/*") {
            let prefix = &pattern[..pattern.len() - 2];
            return path.starts_with(prefix);
        }
        return false;
    }

    for (pp, pat) in path_parts.iter().zip(pattern_parts.iter()) {
        if *pat == "*" {
            continue; // Wildcard matches any single component
        }
        if *pat == "\\*" {
            continue; // Escaped wildcard also matches
        }
        if *pp != *pat {
            // Check per-character wildcard
            if pat.contains('@') {
                if pp.len() != pat.len() {
                    return false;
                }
                for (c, p) in pp.chars().zip(pat.chars()) {
                    if p != '@' && c != p {
                        return false;
                    }
                }
                continue;
            }
            return false;
        }
    }

    true
}

/// TOMOYO policy (public API for backward compatibility)
pub struct TomoyoPolicy;

impl TomoyoPolicy {
    pub fn new() -> Self {
        TomoyoPolicy
    }

    pub fn allow_path(&mut self, domain: &str, path: &str, perm: u8) {
        if let Some(ref mut inner) = *TOMOYO.lock() {
            inner.add_rule(domain, path, perm, RuleType::File);
        }
    }

    pub fn check_access(&self, domain: &str, path: &str, perm: u8) -> bool {
        // Find a PID in this domain and check via it
        if let Some(ref mut inner) = *TOMOYO.lock() {
            let domain_idx = inner.find_domain_index(domain);
            if let Some(idx) = domain_idx {
                if idx < inner.domains.len() {
                    if let Some(&pid) = inner.domains[idx].pids.first() {
                        return inner.check_access(pid, path, perm);
                    }
                }
            }
            // If no PID found, check rules directly
            if let Some(idx) = domain_idx {
                return inner.check_rule_exists(idx, path, perm, RuleType::File);
            }
        }
        false
    }
}

/// Check file access
pub fn check_access(pid: u32, path: &str, perm: u8) -> bool {
    if let Some(ref mut inner) = *TOMOYO.lock() {
        return inner.check_access(pid, path, perm);
    }
    true
}

/// Add a policy rule
pub fn allow_path(domain: &str, path: &str, perm: u8) {
    if let Some(ref mut inner) = *TOMOYO.lock() {
        inner.add_rule(domain, path, perm, RuleType::File);
    }
}

/// Handle domain transition on exec
pub fn domain_transition(pid: u32, exec_path: &str) {
    if let Some(ref mut inner) = *TOMOYO.lock() {
        inner.domain_transition(pid, exec_path);
    }
}

/// Set global policy mode
pub fn set_mode(mode: PolicyMode) {
    if let Some(ref mut inner) = *TOMOYO.lock() {
        let old = inner.global_mode;
        inner.global_mode = mode;
        serial_println!("    [tomoyo] Policy mode changed: {:?} -> {:?}", old, mode);
    }
}

/// Get global policy mode
pub fn get_mode() -> PolicyMode {
    if let Some(ref inner) = *TOMOYO.lock() {
        return inner.global_mode;
    }
    PolicyMode::Disabled
}

/// Handle process exit
pub fn process_exit(pid: u32) {
    if let Some(ref mut inner) = *TOMOYO.lock() {
        inner.unregister_process(pid);
    }
}

/// Get statistics
pub fn stats() -> (u64, u64, u64, u64) {
    if let Some(ref inner) = *TOMOYO.lock() {
        return (
            inner.total_checks,
            inner.total_denials,
            inner.total_learned,
            inner.domains_created,
        );
    }
    (0, 0, 0, 0)
}

/// Initialize the TOMOYO subsystem
pub fn init() {
    let mut inner = TomoyoInner::new();
    inner.init_root_domain();

    let mode = inner.global_mode;
    *TOMOYO.lock() = Some(inner);

    serial_println!(
        "    [tomoyo] Pathname-based MAC initialized (mode: {:?})",
        mode
    );
    serial_println!(
        "    [tomoyo] Max domains: {}, max rules/domain: {}",
        MAX_DOMAINS,
        MAX_RULES_PER_DOMAIN
    );
}
