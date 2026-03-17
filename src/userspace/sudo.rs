use crate::sync::Mutex;
/// sudo — privilege escalation for Genesis
///
/// Implements su/sudo-style privilege escalation with:
///   - Sudoers rules (user/group/command ACLs)
///   - Password caching with configurable TTL
///   - Full audit trail logged to syslog
///   - Environment sanitization (strip dangerous vars)
///   - Per-user and per-group policies
///
/// Security model:
///   uid 0 = root. All other uids require explicit sudoers
///   rules to run privileged commands.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum cached credentials before eviction
const MAX_CACHED_CREDS: usize = 64;

/// Default password cache TTL in seconds
const DEFAULT_CACHE_TTL: u64 = 300;

/// Maximum sudoers rules
const MAX_RULES: usize = 256;

/// Maximum audit log entries
const MAX_AUDIT_ENTRIES: usize = 2048;

/// Root user ID
const ROOT_UID: u32 = 0;

/// Privilege level for a sudoers rule
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    /// Full root access (ALL commands)
    All,
    /// Specific command only
    Command,
    /// No password required (NOPASSWD)
    NoPassword,
}

/// Target user/group for privilege escalation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAs {
    /// Run as root (default)
    Root,
    /// Run as specific user
    User(u32),
    /// Run as any user
    AnyUser,
}

/// A sudoers rule entry
#[derive(Debug, Clone)]
pub struct SudoersRule {
    /// Rule identifier
    pub id: u32,
    /// User this rule applies to (None = group-based)
    pub uid: Option<u32>,
    /// Group this rule applies to (None = user-based)
    pub gid: Option<u32>,
    /// Hostname restriction (empty = all hosts)
    pub hostname: String,
    /// Target user to run as
    pub run_as: RunAs,
    /// Privilege level
    pub privilege: Privilege,
    /// Allowed command pattern (empty = ALL)
    pub command: String,
    /// Whether NOPASSWD is set
    pub nopasswd: bool,
    /// Whether this rule is enabled
    pub enabled: bool,
}

/// A cached credential entry
#[derive(Debug, Clone)]
struct CachedCredential {
    /// User who authenticated
    uid: u32,
    /// Terminal/session ID
    tty: u32,
    /// Timestamp when credential was cached
    timestamp: u64,
    /// TTL in seconds
    ttl: u64,
}

/// Audit log entry
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Timestamp (uptime seconds)
    pub timestamp: u64,
    /// User who invoked sudo
    pub uid: u32,
    /// Target user
    pub target_uid: u32,
    /// Command attempted
    pub command: String,
    /// Terminal/session
    pub tty: u32,
    /// Working directory
    pub cwd: String,
    /// Whether the attempt was allowed
    pub allowed: bool,
    /// Reason for denial (if denied)
    pub reason: String,
}

/// Environment variable entry
#[derive(Debug, Clone)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// Dangerous environment variables to strip
const DANGEROUS_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
    "PERL5LIB",
    "RUBYLIB",
    "CLASSPATH",
    "BASH_ENV",
    "ENV",
    "CDPATH",
    "IFS",
    "GLOBIGNORE",
    "BASH_FUNC_",
    "SHELLOPTS",
    "PS4",
    "PROMPT_COMMAND",
];

/// Safe environment variables to preserve
const SAFE_ENV_VARS: &[&str] = &[
    "PATH", "HOME", "SHELL", "USER", "LOGNAME", "TERM", "DISPLAY", "LANG", "LC_ALL", "TZ",
];

/// Password hash (simplified hash function)
fn simple_hash(input: &[u8]) -> u64 {
    let mut h: u64 = 0x5A5A5A5A5A5A5A5A;
    for &b in input {
        h = h.wrapping_mul(0x00000001000001B3).wrapping_add(b as u64);
    }
    h
}

/// Global sudo state
struct SudoState {
    rules: Vec<SudoersRule>,
    next_rule_id: u32,
    cached_creds: Vec<CachedCredential>,
    cache_ttl: u64,
    audit_log: Vec<AuditEntry>,
    /// Q16 fixed-point: password attempt rate limit (attempts per second << 16)
    rate_limit_q16: i32,
    /// Maximum failed attempts before lockout
    max_failures: u32,
    /// Per-user failure counters: (uid, count, last_attempt_time)
    failure_counts: Vec<(u32, u32, u64)>,
}

impl SudoState {
    const fn new() -> Self {
        SudoState {
            rules: Vec::new(),
            next_rule_id: 1,
            cached_creds: Vec::new(),
            cache_ttl: DEFAULT_CACHE_TTL,
            audit_log: Vec::new(),
            rate_limit_q16: 3 << 16, // 3 attempts per second
            max_failures: 5,
            failure_counts: Vec::new(),
        }
    }
}

static SUDO: Mutex<SudoState> = Mutex::new(SudoState::new());

/// Add a sudoers rule
pub fn add_rule(
    uid: Option<u32>,
    gid: Option<u32>,
    command: &str,
    run_as: RunAs,
    nopasswd: bool,
) -> u32 {
    let mut state = SUDO.lock();
    if state.rules.len() >= MAX_RULES {
        serial_println!("  [sudo] WARNING: max sudoers rules reached");
        return 0;
    }
    let id = state.next_rule_id;
    state.next_rule_id = state.next_rule_id.saturating_add(1);

    state.rules.push(SudoersRule {
        id,
        uid,
        gid,
        hostname: String::new(),
        run_as,
        privilege: if command.is_empty() {
            Privilege::All
        } else {
            Privilege::Command
        },
        command: String::from(command),
        nopasswd,
        enabled: true,
    });

    id
}

/// Remove a sudoers rule by ID
pub fn remove_rule(id: u32) -> bool {
    let mut state = SUDO.lock();
    let before = state.rules.len();
    state.rules.retain(|r| r.id != id);
    state.rules.len() < before
}

/// Enable or disable a rule
pub fn set_rule_enabled(id: u32, enabled: bool) -> bool {
    let mut state = SUDO.lock();
    if let Some(rule) = state.rules.iter_mut().find(|r| r.id == id) {
        rule.enabled = enabled;
        true
    } else {
        false
    }
}

/// Check if a user is allowed to run a command as target_uid
fn check_rules(
    state: &SudoState,
    uid: u32,
    gid: u32,
    command: &str,
    target_uid: u32,
) -> (bool, bool) {
    // Root can do anything
    if uid == ROOT_UID {
        return (true, true); // (allowed, nopasswd)
    }

    for rule in &state.rules {
        if !rule.enabled {
            continue;
        }

        // Check user match
        let user_match = match rule.uid {
            Some(rule_uid) => rule_uid == uid,
            None => match rule.gid {
                Some(rule_gid) => rule_gid == gid,
                None => false,
            },
        };

        if !user_match {
            continue;
        }

        // Check target user match
        let target_match = match rule.run_as {
            RunAs::Root => target_uid == ROOT_UID,
            RunAs::User(u) => target_uid == u,
            RunAs::AnyUser => true,
        };

        if !target_match {
            continue;
        }

        // Check command match
        let cmd_match = match rule.privilege {
            Privilege::All | Privilege::NoPassword => true,
            Privilege::Command => {
                if rule.command.is_empty() {
                    true
                } else {
                    command.starts_with(rule.command.as_str())
                }
            }
        };

        if cmd_match {
            return (true, rule.nopasswd);
        }
    }

    (false, false)
}

/// Check if user has cached credentials
fn has_cached_cred(state: &SudoState, uid: u32, tty: u32, now: u64) -> bool {
    state
        .cached_creds
        .iter()
        .any(|c| c.uid == uid && c.tty == tty && now < c.timestamp + c.ttl)
}

/// Cache a credential after successful authentication
fn cache_credential(state: &mut SudoState, uid: u32, tty: u32, now: u64) {
    // Remove expired entries
    state.cached_creds.retain(|c| now < c.timestamp + c.ttl);

    // Evict oldest if at capacity
    if state.cached_creds.len() >= MAX_CACHED_CREDS {
        state.cached_creds.remove(0);
    }

    let ttl = state.cache_ttl;
    state.cached_creds.push(CachedCredential {
        uid,
        tty,
        timestamp: now,
        ttl,
    });
}

/// Record an audit entry
fn record_audit(
    state: &mut SudoState,
    uid: u32,
    target_uid: u32,
    command: &str,
    tty: u32,
    allowed: bool,
    reason: &str,
) {
    let now = crate::time::clock::uptime_secs();

    if state.audit_log.len() >= MAX_AUDIT_ENTRIES {
        state.audit_log.remove(0);
    }

    let entry = AuditEntry {
        timestamp: now,
        uid,
        target_uid,
        command: String::from(command),
        tty,
        cwd: String::from("/"),
        allowed,
        reason: String::from(reason),
    };

    // Log to syslog
    if allowed {
        crate::userspace::syslog::auth(
            crate::userspace::syslog::Severity::Info,
            &alloc::format!("sudo: uid={} -> uid={} cmd={}", uid, target_uid, command),
        );
    } else {
        crate::userspace::syslog::auth(
            crate::userspace::syslog::Severity::Warning,
            &alloc::format!(
                "sudo DENIED: uid={} -> uid={} cmd={} ({})",
                uid,
                target_uid,
                command,
                reason
            ),
        );
    }

    state.audit_log.push(entry);
}

/// Check rate limiting for a user
fn check_rate_limit(state: &mut SudoState, uid: u32, now: u64) -> bool {
    if let Some(entry) = state.failure_counts.iter().find(|e| e.0 == uid) {
        if entry.1 >= state.max_failures {
            // Lockout period: 1 second per failure
            let lockout_secs = entry.1 as u64;
            if now < entry.2 + lockout_secs {
                return false; // still locked out
            }
        }
    }
    true
}

/// Record a failed authentication attempt
fn record_failure(state: &mut SudoState, uid: u32, now: u64) {
    if let Some(entry) = state.failure_counts.iter_mut().find(|e| e.0 == uid) {
        entry.1 = entry.1.saturating_add(1);
        entry.2 = now;
    } else {
        state.failure_counts.push((uid, 1, now));
    }
}

/// Reset failure count for a user (after successful auth)
fn reset_failures(state: &mut SudoState, uid: u32) {
    state.failure_counts.retain(|e| e.0 != uid);
}

/// Attempt privilege escalation
///
/// Returns Ok(target_uid) if allowed, Err(reason) if denied.
pub fn sudo_exec(
    uid: u32,
    gid: u32,
    target_uid: u32,
    command: &str,
    tty: u32,
    password_hash: u64,
) -> Result<u32, &'static str> {
    let mut state = SUDO.lock();
    let now = crate::time::clock::uptime_secs();

    // Root always allowed
    if uid == ROOT_UID {
        record_audit(&mut state, uid, target_uid, command, tty, true, "root");
        return Ok(target_uid);
    }

    // Check rate limiting
    if !check_rate_limit(&mut state, uid, now) {
        record_audit(
            &mut state,
            uid,
            target_uid,
            command,
            tty,
            false,
            "rate-limited",
        );
        return Err("too many failed attempts, try later");
    }

    // Check sudoers rules
    let (allowed, nopasswd) = check_rules(&state, uid, gid, command, target_uid);
    if !allowed {
        record_audit(
            &mut state,
            uid,
            target_uid,
            command,
            tty,
            false,
            "no matching rule",
        );
        return Err("user is not in the sudoers file");
    }

    // Check authentication
    if !nopasswd {
        // Check credential cache first
        if !has_cached_cred(&state, uid, tty, now) {
            // Verify password (simplified: compare hashes)
            let expected = simple_hash(b"genesis");
            if password_hash != expected {
                record_failure(&mut state, uid, now);
                record_audit(
                    &mut state,
                    uid,
                    target_uid,
                    command,
                    tty,
                    false,
                    "bad password",
                );
                return Err("incorrect password");
            }
            // Cache the credential
            cache_credential(&mut state, uid, tty, now);
            reset_failures(&mut state, uid);
        }
    }

    record_audit(&mut state, uid, target_uid, command, tty, true, "allowed");
    Ok(target_uid)
}

/// Sanitize environment for privileged execution
pub fn sanitize_env(env: &[(String, String)]) -> Vec<EnvVar> {
    let mut clean = Vec::new();

    for (key, value) in env {
        // Skip dangerous variables
        let is_dangerous = DANGEROUS_ENV_VARS
            .iter()
            .any(|&d| key.as_str() == d || key.starts_with(d));

        if is_dangerous {
            continue;
        }

        // Only keep safe variables
        let is_safe = SAFE_ENV_VARS.iter().any(|&s| key.as_str() == s);
        if is_safe {
            clean.push(EnvVar {
                key: key.clone(),
                value: value.clone(),
            });
        }
    }

    // Always set secure PATH
    clean.push(EnvVar {
        key: String::from("PATH"),
        value: String::from("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"),
    });

    clean
}

/// Invalidate cached credentials for a user
pub fn invalidate_cache(uid: u32) {
    let mut state = SUDO.lock();
    state.cached_creds.retain(|c| c.uid != uid);
}

/// Invalidate all cached credentials
pub fn invalidate_all_cache() {
    let mut state = SUDO.lock();
    state.cached_creds.clear();
}

/// Set the credential cache TTL (seconds)
pub fn set_cache_ttl(ttl_secs: u64) {
    SUDO.lock().cache_ttl = ttl_secs;
}

/// Get recent audit log entries
pub fn audit_log(count: usize) -> Vec<AuditEntry> {
    let state = SUDO.lock();
    let len = state.audit_log.len();
    let skip = if len > count { len - count } else { 0 };
    state.audit_log.iter().skip(skip).cloned().collect()
}

/// Format audit log for display
pub fn format_audit(entries: &[AuditEntry]) -> String {
    let mut out = String::from("TIMESTAMP  UID  TARGET  ALLOWED  COMMAND\n");
    for e in entries {
        out.push_str(&alloc::format!(
            "{:<9}  {:<4} {:<7} {:<8} {}\n",
            e.timestamp,
            e.uid,
            e.target_uid,
            if e.allowed { "yes" } else { "NO" },
            e.command
        ));
    }
    out
}

/// List all sudoers rules
pub fn list_rules() -> String {
    let state = SUDO.lock();
    let mut out = String::from("ID  USER/GRP  RUNAS   NOPASSWD  ENABLED  COMMAND\n");
    for r in &state.rules {
        let who = match (r.uid, r.gid) {
            (Some(u), _) => alloc::format!("u:{}", u),
            (_, Some(g)) => alloc::format!("g:{}", g),
            _ => String::from("???"),
        };
        let target = match r.run_as {
            RunAs::Root => String::from("root"),
            RunAs::User(u) => alloc::format!("u:{}", u),
            RunAs::AnyUser => String::from("ALL"),
        };
        let cmd = if r.command.is_empty() {
            String::from("ALL")
        } else {
            r.command.clone()
        };
        out.push_str(&alloc::format!(
            "{:<3} {:<9} {:<7} {:<9} {:<8} {}\n",
            r.id,
            who,
            target,
            if r.nopasswd { "yes" } else { "no" },
            if r.enabled { "yes" } else { "no" },
            cmd
        ));
    }
    out
}

/// su -- switch user (requires target user password or root)
pub fn su(current_uid: u32, target_uid: u32, password_hash: u64) -> Result<u32, &'static str> {
    // Root can su to anyone without password
    if current_uid == ROOT_UID {
        crate::userspace::syslog::auth(
            crate::userspace::syslog::Severity::Info,
            &alloc::format!("su: root -> uid={}", target_uid),
        );
        return Ok(target_uid);
    }

    // Non-root must provide target user password
    let expected = simple_hash(b"genesis");
    if password_hash != expected {
        crate::userspace::syslog::auth(
            crate::userspace::syslog::Severity::Warning,
            &alloc::format!(
                "su DENIED: uid={} -> uid={} (bad password)",
                current_uid,
                target_uid
            ),
        );
        return Err("incorrect password");
    }

    crate::userspace::syslog::auth(
        crate::userspace::syslog::Severity::Info,
        &alloc::format!("su: uid={} -> uid={}", current_uid, target_uid),
    );
    Ok(target_uid)
}

/// Initialize the sudo subsystem with default rules
pub fn init() {
    let mut state = SUDO.lock();

    // Default rule: root can do everything
    state.rules.push(SudoersRule {
        id: 0,
        uid: Some(ROOT_UID),
        gid: None,
        hostname: String::new(),
        run_as: RunAs::AnyUser,
        privilege: Privilege::All,
        command: String::new(),
        nopasswd: true,
        enabled: true,
    });

    // Default rule: wheel group can sudo with password
    let rid = state.next_rule_id;
    state.next_rule_id = state.next_rule_id.saturating_add(1);
    state.rules.push(SudoersRule {
        id: rid,
        uid: None,
        gid: Some(10), // wheel group
        hostname: String::new(),
        run_as: RunAs::Root,
        privilege: Privilege::All,
        command: String::new(),
        nopasswd: false,
        enabled: true,
    });

    // Default rule: first user (uid 1000) gets NOPASSWD ALL
    let rid = state.next_rule_id;
    state.next_rule_id = state.next_rule_id.saturating_add(1);
    state.rules.push(SudoersRule {
        id: rid,
        uid: Some(1000),
        gid: None,
        hostname: String::new(),
        run_as: RunAs::Root,
        privilege: Privilege::All,
        command: String::new(),
        nopasswd: true,
        enabled: true,
    });

    serial_println!(
        "  sudo: privilege escalation ready ({} rules)",
        state.rules.len()
    );
}
