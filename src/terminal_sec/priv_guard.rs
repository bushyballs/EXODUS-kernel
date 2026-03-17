use crate::sync::Mutex;
/// Terminal privilege escalation protection
///
/// Controls sudo, su, setuid, and capability elevation requests.
/// Every privilege escalation must pass through PrivGuard, which enforces:
///   - Per-user escalation policies (max level, 2FA, timeout)
///   - Brute-force lockout after repeated failures
///   - Session-based sudo with expiry and command counting
///   - Setuid binary validation against a whitelist
///   - Full audit trail of all escalation attempts
///
/// Privilege levels (ascending): User < Operator < Admin < Root < Kernel
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// Global privilege guard instance
static PRIV_GUARD: Mutex<Option<PrivGuard>> = Mutex::new(None);

// ── Constants ──────────────────────────────────────────────────────────────

/// Maximum escalation log entries retained
const MAX_LOG_ENTRIES: usize = 2048;

/// Default sudo session timeout in seconds (5 minutes)
const DEFAULT_SUDO_TIMEOUT: u32 = 300;

/// Default max failed attempts before lockout
const DEFAULT_MAX_ATTEMPTS: u8 = 5;

/// Default lockout duration in seconds (15 minutes)
const DEFAULT_LOCKOUT_SECONDS: u32 = 900;

/// Maximum concurrent sudo sessions per user
const MAX_SESSIONS_PER_USER: usize = 4;

// ── Privilege Level ────────────────────────────────────────────────────────

/// Privilege levels in ascending order of power
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PrivilegeLevel {
    /// Standard unprivileged user
    User = 0,
    /// Elevated operator (can manage services, view logs)
    Operator = 1,
    /// System administrator (can modify configs, manage users)
    Admin = 2,
    /// Root / superuser (full system access)
    Root = 3,
    /// Kernel level (ring-0 only, no user process can hold this)
    Kernel = 4,
}

impl PrivilegeLevel {
    /// Return a numeric rank for comparisons
    pub fn rank(self) -> u8 {
        self as u8
    }

    /// Convert from raw u8, clamping unknown values to User
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => PrivilegeLevel::User,
            1 => PrivilegeLevel::Operator,
            2 => PrivilegeLevel::Admin,
            3 => PrivilegeLevel::Root,
            4 => PrivilegeLevel::Kernel,
            _ => PrivilegeLevel::User,
        }
    }
}

// ── Escalation Request ─────────────────────────────────────────────────────

/// A request to escalate privileges
#[derive(Debug, Clone)]
pub struct EscalationRequest {
    /// User ID requesting escalation
    pub uid: u32,
    /// Target privilege level
    pub target_level: PrivilegeLevel,
    /// Hash of the command to execute at elevated privilege
    pub command_hash: u64,
    /// Timestamp of the request (kernel ticks or unix time)
    pub timestamp: u64,
    /// Hash identifying the originating TTY/PTY
    pub tty_hash: u64,
    /// Whether this request has been approved
    pub approved: bool,
    /// UID of the approver (0 if self-authenticated, or an admin UID)
    pub approver: u32,
}

// ── Escalation Policy ──────────────────────────────────────────────────────

/// Per-user escalation policy controlling what elevations are allowed
#[derive(Debug, Clone)]
pub struct EscalationPolicy {
    /// Highest privilege level this user can escalate to
    pub max_level: PrivilegeLevel,
    /// Whether password authentication is required
    pub require_password: bool,
    /// Whether a second-factor challenge is required
    pub require_2fa: bool,
    /// Seconds a sudo session remains valid after grant
    pub timeout_seconds: u32,
    /// Maximum consecutive failed attempts before lockout
    pub max_attempts: u8,
    /// Seconds to lock the user out after exceeding max_attempts
    pub lockout_seconds: u32,
    /// Command hashes explicitly permitted (empty = all allowed)
    pub allowed_commands: Vec<u64>,
    /// Command hashes explicitly denied (checked before allowed)
    pub denied_commands: Vec<u64>,
}

impl EscalationPolicy {
    /// Default restrictive policy for regular users
    pub fn default_user() -> Self {
        EscalationPolicy {
            max_level: PrivilegeLevel::User,
            require_password: true,
            require_2fa: false,
            timeout_seconds: DEFAULT_SUDO_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            lockout_seconds: DEFAULT_LOCKOUT_SECONDS,
            allowed_commands: Vec::new(),
            denied_commands: Vec::new(),
        }
    }

    /// Policy for wheel/sudoers group members
    pub fn default_sudoer() -> Self {
        EscalationPolicy {
            max_level: PrivilegeLevel::Root,
            require_password: true,
            require_2fa: false,
            timeout_seconds: DEFAULT_SUDO_TIMEOUT,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            lockout_seconds: DEFAULT_LOCKOUT_SECONDS,
            allowed_commands: Vec::new(),
            denied_commands: Vec::new(),
        }
    }

    /// Policy for system operator accounts
    pub fn default_operator() -> Self {
        EscalationPolicy {
            max_level: PrivilegeLevel::Operator,
            require_password: true,
            require_2fa: false,
            timeout_seconds: 600,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            lockout_seconds: DEFAULT_LOCKOUT_SECONDS,
            allowed_commands: Vec::new(),
            denied_commands: Vec::new(),
        }
    }
}

// ── Sudo Session ───────────────────────────────────────────────────────────

/// An active elevated-privilege session (sudo/su)
#[derive(Debug, Clone)]
pub struct SudoSession {
    /// User ID holding this session
    pub uid: u32,
    /// Privilege level granted
    pub level: PrivilegeLevel,
    /// Timestamp when the session was granted
    pub granted: u64,
    /// Timestamp when the session expires
    pub expires: u64,
    /// Number of commands executed under this session
    pub commands_run: u32,
    /// TTY hash this session is bound to
    pub tty_hash: u64,
}

impl SudoSession {
    /// Check if this session has expired
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires
    }

    /// Check if this session is bound to a specific TTY
    pub fn matches_tty(&self, tty: u64) -> bool {
        self.tty_hash == tty
    }
}

// ── Escalation Log Entry ───────────────────────────────────────────────────

/// Outcome of an escalation attempt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationOutcome {
    Granted,
    DeniedPolicy,
    DeniedLockedOut,
    DeniedBadAuth,
    DeniedCommand,
    DeniedMaxLevel,
    Revoked,
    SessionExpired,
}

/// A record of an escalation event for audit purposes
#[derive(Debug, Clone)]
pub struct EscalationLogEntry {
    pub timestamp: u64,
    pub uid: u32,
    pub target_level: PrivilegeLevel,
    pub command_hash: u64,
    pub tty_hash: u64,
    pub outcome: EscalationOutcome,
}

// ── Lockout State ──────────────────────────────────────────────────────────

/// Per-user lockout tracking
#[derive(Debug, Clone)]
struct LockoutState {
    /// Consecutive failed attempts
    failed_count: u8,
    /// Timestamp of last failure
    last_failure: u64,
    /// Timestamp when lockout expires (0 = not locked out)
    lockout_until: u64,
}

impl LockoutState {
    fn new() -> Self {
        LockoutState {
            failed_count: 0,
            last_failure: 0,
            lockout_until: 0,
        }
    }
}

// ── Setuid Entry ───────────────────────────────────────────────────────────

/// A registered setuid binary that is permitted to elevate
#[derive(Debug, Clone)]
struct SetuidEntry {
    /// Hash of the binary path
    binary_hash: u64,
    /// Target privilege level the binary runs as
    target_level: PrivilegeLevel,
    /// Whether this entry is currently enabled
    enabled: bool,
}

// ── Capability Elevation Entry ─────────────────────────────────────────────

/// A registered capability that can be checked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelCapability {
    NetAdmin,
    SysAdmin,
    DacOverride,
    Kill,
    Chown,
    SetUid,
    SetGid,
    NetBind,
    SysTime,
    SysBoot,
    Mknod,
    RawIO,
}

// ── PrivGuard ──────────────────────────────────────────────────────────────

/// Central privilege escalation guard
pub struct PrivGuard {
    /// Active sudo sessions keyed by (uid, tty_hash)
    active_sessions: BTreeMap<(u32, u64), SudoSession>,
    /// Per-user escalation policies
    policies: BTreeMap<u32, EscalationPolicy>,
    /// Per-user lockout state
    lockouts: BTreeMap<u32, LockoutState>,
    /// Registered setuid binaries
    setuid_whitelist: Vec<SetuidEntry>,
    /// Per-user capability grants: uid -> list of granted capabilities
    capability_grants: BTreeMap<u32, Vec<KernelCapability>>,
    /// Escalation history ring buffer
    log: Vec<EscalationLogEntry>,
    /// Write index into the ring buffer
    log_index: usize,
    /// Whether the log has wrapped around
    log_wrapped: bool,
    /// Monotonic session counter
    session_counter: u64,
}

impl PrivGuard {
    /// Create a new privilege guard with empty state
    pub fn new() -> Self {
        let mut log = Vec::with_capacity(MAX_LOG_ENTRIES);
        for _ in 0..MAX_LOG_ENTRIES {
            log.push(EscalationLogEntry {
                timestamp: 0,
                uid: 0,
                target_level: PrivilegeLevel::User,
                command_hash: 0,
                tty_hash: 0,
                outcome: EscalationOutcome::DeniedPolicy,
            });
        }

        PrivGuard {
            active_sessions: BTreeMap::new(),
            policies: BTreeMap::new(),
            lockouts: BTreeMap::new(),
            setuid_whitelist: Vec::new(),
            capability_grants: BTreeMap::new(),
            log,
            log_index: 0,
            log_wrapped: false,
            session_counter: 0,
        }
    }

    // ── Policy Management ──────────────────────────────────────────────

    /// Set the escalation policy for a user
    pub fn set_policy(&mut self, uid: u32, policy: EscalationPolicy) {
        self.policies.insert(uid, policy);
    }

    /// Get the escalation policy for a user (falls back to default)
    pub fn get_policy(&self, uid: u32) -> EscalationPolicy {
        self.policies
            .get(&uid)
            .cloned()
            .unwrap_or_else(EscalationPolicy::default_user)
    }

    // ── Lockout ────────────────────────────────────────────────────────

    /// Check if a user is currently locked out
    pub fn is_locked_out(&self, uid: u32, now: u64) -> bool {
        if let Some(state) = self.lockouts.get(&uid) {
            if state.lockout_until > 0 && now < state.lockout_until {
                return true;
            }
        }
        false
    }

    /// Record a failed escalation attempt and potentially trigger lockout
    pub fn record_failure(&mut self, uid: u32, now: u64) {
        let policy = self.get_policy(uid);
        let state = self.lockouts.entry(uid).or_insert_with(LockoutState::new);

        state.failed_count += 1;
        state.last_failure = now;

        if state.failed_count >= policy.max_attempts {
            state.lockout_until = now + policy.lockout_seconds as u64;
            serial_println!(
                "    [priv_guard] UID {} locked out until timestamp {}",
                uid,
                state.lockout_until
            );
        }
    }

    /// Reset the failure counter for a user (called on successful auth)
    fn reset_failures(&mut self, uid: u32) {
        if let Some(state) = self.lockouts.get_mut(&uid) {
            state.failed_count = 0;
            state.lockout_until = 0;
        }
    }

    // ── Logging ────────────────────────────────────────────────────────

    /// Append an entry to the escalation audit log
    fn append_log(&mut self, entry: EscalationLogEntry) {
        if self.log_index >= MAX_LOG_ENTRIES {
            self.log_index = 0;
            self.log_wrapped = true;
        }
        self.log[self.log_index] = entry;
        self.log_index += 1;
    }

    /// Get the escalation log (oldest-first order)
    pub fn get_escalation_log(&self) -> Vec<EscalationLogEntry> {
        if self.log_wrapped {
            let mut result = Vec::with_capacity(MAX_LOG_ENTRIES);
            for i in self.log_index..MAX_LOG_ENTRIES {
                result.push(self.log[i].clone());
            }
            for i in 0..self.log_index {
                result.push(self.log[i].clone());
            }
            result
        } else {
            let mut result = Vec::with_capacity(self.log_index);
            for i in 0..self.log_index {
                result.push(self.log[i].clone());
            }
            result
        }
    }

    // ── Escalation Request / Grant / Revoke ────────────────────────────

    /// Request a privilege escalation. Returns Ok(()) if the request
    /// is structurally valid and can proceed to authentication, or
    /// Err with a reason if it is immediately denied.
    pub fn request_escalation(&mut self, request: &EscalationRequest) -> Result<(), &'static str> {
        let now = request.timestamp;

        // Kernel level is never grantable to userspace
        if request.target_level == PrivilegeLevel::Kernel {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid: request.uid,
                target_level: request.target_level,
                command_hash: request.command_hash,
                tty_hash: request.tty_hash,
                outcome: EscalationOutcome::DeniedMaxLevel,
            });
            return Err("kernel privilege cannot be granted to userspace");
        }

        // Check lockout
        if self.is_locked_out(request.uid, now) {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid: request.uid,
                target_level: request.target_level,
                command_hash: request.command_hash,
                tty_hash: request.tty_hash,
                outcome: EscalationOutcome::DeniedLockedOut,
            });
            return Err("user is locked out due to repeated failures");
        }

        // Check policy max level
        let policy = self.get_policy(request.uid);
        if request.target_level > policy.max_level {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid: request.uid,
                target_level: request.target_level,
                command_hash: request.command_hash,
                tty_hash: request.tty_hash,
                outcome: EscalationOutcome::DeniedMaxLevel,
            });
            return Err("target level exceeds policy maximum");
        }

        // Check denied commands list
        if !policy.denied_commands.is_empty()
            && policy.denied_commands.contains(&request.command_hash)
        {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid: request.uid,
                target_level: request.target_level,
                command_hash: request.command_hash,
                tty_hash: request.tty_hash,
                outcome: EscalationOutcome::DeniedCommand,
            });
            return Err("command is explicitly denied by policy");
        }

        // Check allowed commands list (if non-empty, acts as whitelist)
        if !policy.allowed_commands.is_empty()
            && !policy.allowed_commands.contains(&request.command_hash)
        {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid: request.uid,
                target_level: request.target_level,
                command_hash: request.command_hash,
                tty_hash: request.tty_hash,
                outcome: EscalationOutcome::DeniedCommand,
            });
            return Err("command is not in the allowed list");
        }

        // Request is valid — authentication still needed
        Ok(())
    }

    /// Grant an escalation after successful authentication.
    /// Creates a SudoSession bound to the user and TTY.
    pub fn grant_escalation(&mut self, request: &EscalationRequest) -> Result<(), &'static str> {
        let now = request.timestamp;
        let policy = self.get_policy(request.uid);

        // Enforce max sessions per user
        let user_session_count = self
            .active_sessions
            .keys()
            .filter(|(uid, _)| *uid == request.uid)
            .count();
        if user_session_count >= MAX_SESSIONS_PER_USER {
            return Err("maximum concurrent sudo sessions reached");
        }

        let session = SudoSession {
            uid: request.uid,
            level: request.target_level,
            granted: now,
            expires: now + policy.timeout_seconds as u64,
            commands_run: 0,
            tty_hash: request.tty_hash,
        };

        self.active_sessions
            .insert((request.uid, request.tty_hash), session);
        self.session_counter = self.session_counter.saturating_add(1);
        self.reset_failures(request.uid);

        self.append_log(EscalationLogEntry {
            timestamp: now,
            uid: request.uid,
            target_level: request.target_level,
            command_hash: request.command_hash,
            tty_hash: request.tty_hash,
            outcome: EscalationOutcome::Granted,
        });

        serial_println!(
            "    [priv_guard] Granted {:?} to UID {} on TTY 0x{:016X}",
            request.target_level,
            request.uid,
            request.tty_hash
        );

        Ok(())
    }

    /// Revoke an active escalation session for a user on a specific TTY
    pub fn revoke_escalation(&mut self, uid: u32, tty_hash: u64, now: u64) {
        if self.active_sessions.remove(&(uid, tty_hash)).is_some() {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid,
                target_level: PrivilegeLevel::User,
                command_hash: 0,
                tty_hash,
                outcome: EscalationOutcome::Revoked,
            });
            serial_println!(
                "    [priv_guard] Revoked escalation for UID {} on TTY 0x{:016X}",
                uid,
                tty_hash
            );
        }
    }

    // ── Session Management ─────────────────────────────────────────────

    /// Check if a user has an active (non-expired) sudo session on a TTY
    pub fn check_session(&mut self, uid: u32, tty_hash: u64, now: u64) -> Option<PrivilegeLevel> {
        if let Some(session) = self.active_sessions.get_mut(&(uid, tty_hash)) {
            if session.is_expired(now) {
                // Expired — remove and log
                let level = session.level;
                self.active_sessions.remove(&(uid, tty_hash));
                self.append_log(EscalationLogEntry {
                    timestamp: now,
                    uid,
                    target_level: level,
                    command_hash: 0,
                    tty_hash,
                    outcome: EscalationOutcome::SessionExpired,
                });
                return None;
            }
            session.commands_run += 1;
            return Some(session.level);
        }
        None
    }

    /// List all active sudo sessions
    pub fn list_sessions(&self) -> Vec<SudoSession> {
        self.active_sessions.values().cloned().collect()
    }

    /// Kill (forcibly revoke) a specific session by uid and tty_hash
    pub fn kill_session(&mut self, uid: u32, tty_hash: u64, now: u64) -> bool {
        if self.active_sessions.remove(&(uid, tty_hash)).is_some() {
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid,
                target_level: PrivilegeLevel::User,
                command_hash: 0,
                tty_hash,
                outcome: EscalationOutcome::Revoked,
            });
            true
        } else {
            false
        }
    }

    /// Expire and remove all stale sessions
    pub fn cleanup_sessions(&mut self, now: u64) {
        let expired: Vec<(u32, u64)> = self
            .active_sessions
            .iter()
            .filter(|(_, s)| s.is_expired(now))
            .map(|(&key, _)| key)
            .collect();

        for (uid, tty) in expired {
            self.active_sessions.remove(&(uid, tty));
            self.append_log(EscalationLogEntry {
                timestamp: now,
                uid,
                target_level: PrivilegeLevel::User,
                command_hash: 0,
                tty_hash: tty,
                outcome: EscalationOutcome::SessionExpired,
            });
        }
    }

    // ── Execution Checks ───────────────────────────────────────────────

    /// Check if a user can execute a command as a target level on a TTY.
    /// Returns true if there is an active session at or above the needed level.
    pub fn can_execute_as(
        &mut self,
        uid: u32,
        tty_hash: u64,
        needed_level: PrivilegeLevel,
        now: u64,
    ) -> bool {
        // UID 0 (root) always has Root-level access
        if uid == 0 && needed_level <= PrivilegeLevel::Root {
            return true;
        }

        if let Some(level) = self.check_session(uid, tty_hash, now) {
            return level >= needed_level;
        }

        false
    }

    // ── Setuid Validation ──────────────────────────────────────────────

    /// Register a setuid binary in the whitelist
    pub fn register_setuid(&mut self, binary_hash: u64, target_level: PrivilegeLevel) {
        // Prevent duplicates
        for entry in &mut self.setuid_whitelist {
            if entry.binary_hash == binary_hash {
                entry.target_level = target_level;
                entry.enabled = true;
                return;
            }
        }
        self.setuid_whitelist.push(SetuidEntry {
            binary_hash,
            target_level,
            enabled: true,
        });
    }

    /// Validate whether a setuid binary is permitted to elevate
    pub fn validate_setuid(&self, binary_hash: u64) -> Option<PrivilegeLevel> {
        for entry in &self.setuid_whitelist {
            if entry.binary_hash == binary_hash && entry.enabled {
                return Some(entry.target_level);
            }
        }
        None
    }

    /// Disable a setuid binary (remove it from active whitelist)
    pub fn disable_setuid(&mut self, binary_hash: u64) {
        for entry in &mut self.setuid_whitelist {
            if entry.binary_hash == binary_hash {
                entry.enabled = false;
                return;
            }
        }
    }

    // ── Capability Checks ──────────────────────────────────────────────

    /// Grant a kernel capability to a user
    pub fn grant_capability(&mut self, uid: u32, cap: KernelCapability) {
        let caps = self.capability_grants.entry(uid).or_insert_with(Vec::new);
        if !caps.contains(&cap) {
            caps.push(cap);
        }
    }

    /// Revoke a kernel capability from a user
    pub fn revoke_capability(&mut self, uid: u32, cap: KernelCapability) {
        if let Some(caps) = self.capability_grants.get_mut(&uid) {
            caps.retain(|c| *c != cap);
        }
    }

    /// Check if a user holds a specific kernel capability.
    /// Root (UID 0) implicitly holds all capabilities.
    pub fn check_capability(&self, uid: u32, cap: KernelCapability) -> bool {
        if uid == 0 {
            return true;
        }
        self.capability_grants
            .get(&uid)
            .map(|caps| caps.contains(&cap))
            .unwrap_or(false)
    }

    /// Get all capabilities granted to a user
    pub fn get_capabilities(&self, uid: u32) -> Vec<KernelCapability> {
        if uid == 0 {
            // Root has all capabilities
            return vec![
                KernelCapability::NetAdmin,
                KernelCapability::SysAdmin,
                KernelCapability::DacOverride,
                KernelCapability::Kill,
                KernelCapability::Chown,
                KernelCapability::SetUid,
                KernelCapability::SetGid,
                KernelCapability::NetBind,
                KernelCapability::SysTime,
                KernelCapability::SysBoot,
                KernelCapability::Mknod,
                KernelCapability::RawIO,
            ];
        }
        self.capability_grants
            .get(&uid)
            .cloned()
            .unwrap_or_default()
    }

    // ── Statistics ─────────────────────────────────────────────────────

    /// Count active (non-expired) sessions
    pub fn active_session_count(&self) -> usize {
        self.active_sessions.len()
    }

    /// Total escalation events processed since boot
    pub fn total_events(&self) -> usize {
        if self.log_wrapped {
            MAX_LOG_ENTRIES
        } else {
            self.log_index
        }
    }

    /// Count failed attempts for a specific user
    pub fn failed_attempt_count(&self, uid: u32) -> u8 {
        self.lockouts.get(&uid).map(|s| s.failed_count).unwrap_or(0)
    }
}

// ── Module-level Public API ────────────────────────────────────────────────

/// Initialize the privilege guard subsystem
pub fn init() {
    let mut guard = PrivGuard::new();

    // Root (UID 0) gets an unrestricted policy
    guard.set_policy(
        0,
        EscalationPolicy {
            max_level: PrivilegeLevel::Root,
            require_password: false,
            require_2fa: false,
            timeout_seconds: 0xFFFFFFFF,
            max_attempts: 255,
            lockout_seconds: 0,
            allowed_commands: Vec::new(),
            denied_commands: Vec::new(),
        },
    );

    // Register common setuid binaries by conventional hash
    // 0xABCDE00001 = /bin/su, 0xABCDE00002 = /usr/bin/sudo
    guard.register_setuid(0xABCDE00001, PrivilegeLevel::Root);
    guard.register_setuid(0xABCDE00002, PrivilegeLevel::Root);
    guard.register_setuid(0xABCDE00003, PrivilegeLevel::Operator);

    *PRIV_GUARD.lock() = Some(guard);
    serial_println!("    [priv_guard] Privilege escalation guard initialized");
}

/// Request privilege escalation (pre-auth validation)
pub fn request_escalation(request: &EscalationRequest) -> Result<(), &'static str> {
    PRIV_GUARD
        .lock()
        .as_mut()
        .ok_or("priv_guard not initialized")?
        .request_escalation(request)
}

/// Grant escalation after authentication succeeds
pub fn grant_escalation(request: &EscalationRequest) -> Result<(), &'static str> {
    PRIV_GUARD
        .lock()
        .as_mut()
        .ok_or("priv_guard not initialized")?
        .grant_escalation(request)
}

/// Revoke an active escalation
pub fn revoke_escalation(uid: u32, tty_hash: u64, now: u64) {
    if let Some(g) = PRIV_GUARD.lock().as_mut() {
        g.revoke_escalation(uid, tty_hash, now);
    }
}

/// Check if a user has an active sudo session
pub fn check_session(uid: u32, tty_hash: u64, now: u64) -> Option<PrivilegeLevel> {
    PRIV_GUARD
        .lock()
        .as_mut()?
        .check_session(uid, tty_hash, now)
}

/// Check if a user is locked out
pub fn is_locked_out(uid: u32, now: u64) -> bool {
    PRIV_GUARD
        .lock()
        .as_ref()
        .map(|g| g.is_locked_out(uid, now))
        .unwrap_or(false)
}

/// Record a failed authentication attempt
pub fn record_failure(uid: u32, now: u64) {
    if let Some(g) = PRIV_GUARD.lock().as_mut() {
        g.record_failure(uid, now);
    }
}

/// Set the escalation policy for a user
pub fn set_policy(uid: u32, policy: EscalationPolicy) {
    if let Some(g) = PRIV_GUARD.lock().as_mut() {
        g.set_policy(uid, policy);
    }
}

/// Get the escalation policy for a user
pub fn get_policy(uid: u32) -> EscalationPolicy {
    PRIV_GUARD
        .lock()
        .as_ref()
        .map(|g| g.get_policy(uid))
        .unwrap_or_else(EscalationPolicy::default_user)
}

/// List all active sudo sessions
pub fn list_sessions() -> Vec<SudoSession> {
    PRIV_GUARD
        .lock()
        .as_ref()
        .map(|g| g.list_sessions())
        .unwrap_or_default()
}

/// Kill a specific sudo session
pub fn kill_session(uid: u32, tty_hash: u64, now: u64) -> bool {
    PRIV_GUARD
        .lock()
        .as_mut()
        .map(|g| g.kill_session(uid, tty_hash, now))
        .unwrap_or(false)
}

/// Check if a user can execute at a given privilege level
pub fn can_execute_as(uid: u32, tty_hash: u64, level: PrivilegeLevel, now: u64) -> bool {
    PRIV_GUARD
        .lock()
        .as_mut()
        .map(|g| g.can_execute_as(uid, tty_hash, level, now))
        .unwrap_or(false)
}

/// Validate a setuid binary
pub fn validate_setuid(binary_hash: u64) -> Option<PrivilegeLevel> {
    PRIV_GUARD.lock().as_ref()?.validate_setuid(binary_hash)
}

/// Check if a user holds a kernel capability
pub fn check_capability(uid: u32, cap: KernelCapability) -> bool {
    PRIV_GUARD
        .lock()
        .as_ref()
        .map(|g| g.check_capability(uid, cap))
        .unwrap_or(false)
}

/// Get the full escalation audit log
pub fn get_escalation_log() -> Vec<EscalationLogEntry> {
    PRIV_GUARD
        .lock()
        .as_ref()
        .map(|g| g.get_escalation_log())
        .unwrap_or_default()
}
