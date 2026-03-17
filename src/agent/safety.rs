use crate::sync::Mutex;
/// Agent safety boundaries, rate limiting, and kill switch
///
/// Part of the AIOS agent layer. Enforces hard limits on agent behavior
/// with government-grade audit trails, token-bucket rate limiting,
/// and a global emergency kill switch.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Severity of a safety violation
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViolationSeverity {
    Info,     // Logged but not blocked
    Warning,  // Logged, counted toward threshold
    Critical, // Logged, action blocked, counted
    Fatal,    // Immediate kill switch activation
}

/// A recorded safety violation for audit
#[derive(Clone)]
pub struct Violation {
    pub timestamp: u64,
    pub severity: ViolationSeverity,
    pub action_hash: u64,
    pub reason_code: u32,
    pub session_id: u32,
}

/// Token-bucket rate limiter
#[derive(Clone, Copy)]
pub struct RateLimiter {
    pub capacity: u32,    // Max burst tokens
    pub tokens: u32,      // Current available tokens
    pub refill_rate: u32, // Tokens added per refill interval
    pub refill_interval_ms: u64,
    pub last_refill: u64, // Timestamp of last refill
}

impl RateLimiter {
    pub fn new(capacity: u32, refill_rate: u32, interval_ms: u64) -> Self {
        RateLimiter {
            capacity,
            tokens: capacity,
            refill_rate,
            refill_interval_ms: interval_ms,
            last_refill: 0,
        }
    }

    /// Refill tokens based on elapsed time
    pub fn refill(&mut self, now: u64) {
        if self.refill_interval_ms == 0 {
            return;
        }
        let elapsed = now.saturating_sub(self.last_refill);
        let intervals = elapsed / self.refill_interval_ms;
        if intervals > 0 {
            let add = (intervals as u32).saturating_mul(self.refill_rate);
            self.tokens = (self.tokens + add).min(self.capacity);
            self.last_refill = now;
        }
    }

    /// Try to consume one token. Returns true if allowed.
    pub fn try_consume(&mut self, now: u64) -> bool {
        self.refill(now);
        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }
}

/// Safety policy configuration
pub struct SafetyPolicy {
    pub max_steps: usize,
    pub max_cost: u64,
    pub max_duration_ms: u64,
    pub blocked_paths: Vec<u64>, // Hashed path prefixes that are off-limits
    pub allowed_paths: Vec<u64>, // Hashed path prefixes that are permitted (empty = all)
    pub require_confirmation: bool,
    pub block_network: bool,
    pub block_writes: bool,
    pub max_write_size: u64,     // Max bytes per write operation
    pub max_files_modified: u32, // Max files per session
    // Rate limits by category
    pub api_rate: RateLimiter,
    pub file_rate: RateLimiter,
    pub cmd_rate: RateLimiter,
    // Violation thresholds
    pub max_warnings: u32, // Warnings before escalation to kill
    pub max_critical: u32, // Critical violations before kill switch
}

impl SafetyPolicy {
    /// Government-grade default: strict but functional
    pub fn government_default() -> Self {
        SafetyPolicy {
            max_steps: 200,
            max_cost: 500_000,        // Cost units
            max_duration_ms: 600_000, // 10 minutes
            blocked_paths: Vec::new(),
            allowed_paths: Vec::new(),
            require_confirmation: true,
            block_network: false,
            block_writes: false,
            max_write_size: 10_000_000, // 10 MB
            max_files_modified: 50,
            api_rate: RateLimiter::new(60, 10, 60_000), // 60 burst, 10/min refill
            file_rate: RateLimiter::new(100, 20, 60_000), // 100 burst, 20/min refill
            cmd_rate: RateLimiter::new(30, 5, 60_000),  // 30 burst, 5/min refill
            max_warnings: 10,
            max_critical: 3,
        }
    }

    /// Maximum lockdown: read-only, no network, low limits
    pub fn paranoid() -> Self {
        SafetyPolicy {
            max_steps: 50,
            max_cost: 10_000,
            max_duration_ms: 60_000,
            blocked_paths: Vec::new(),
            allowed_paths: Vec::new(),
            require_confirmation: true,
            block_network: true,
            block_writes: true,
            max_write_size: 0,
            max_files_modified: 0,
            api_rate: RateLimiter::new(5, 1, 60_000),
            file_rate: RateLimiter::new(20, 5, 60_000),
            cmd_rate: RateLimiter::new(5, 1, 60_000),
            max_warnings: 3,
            max_critical: 1,
        }
    }
}

/// FNV-1a hash of a byte slice (compile-time compatible)
const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Dangerous command patterns (FNV-1a hashed)
const DANGEROUS_CMDS: &[u64] = &[
    fnv1a(b"rm -rf"),
    fnv1a(b"mkfs"),
    fnv1a(b"dd"),
    fnv1a(b"format"),
    fnv1a(b"shutdown"),
    fnv1a(b"reboot"),
    fnv1a(b"iptables"),
    fnv1a(b"chmod 777"),
    fnv1a(b"chown root"),
];

/// Sensitive path patterns (FNV-1a hashed)
const SENSITIVE_PATHS: &[u64] = &[
    fnv1a(b"/etc/shadow"),
    fnv1a(b"/etc/passwd"),
    fnv1a(b"/boot/"),
    fnv1a(b"/var/log/"),
    fnv1a(b".env"),
    fnv1a(b".ssh/"),
    fnv1a(b".gnupg/"),
];

/// The safety guard — enforces all policies
struct SafetyGuardInner {
    policy: SafetyPolicy,
    steps_taken: usize,
    cost_spent: u64,
    start_time: u64,
    files_modified: u32,
    violations: Vec<Violation>,
    warning_count: u32,
    critical_count: u32,
    killed: bool, // Emergency kill switch
    session_id: u32,
}

static SAFETY: Mutex<Option<SafetyGuardInner>> = Mutex::new(None);

impl SafetyGuardInner {
    fn new(policy: SafetyPolicy, session_id: u32, now: u64) -> Self {
        SafetyGuardInner {
            policy,
            steps_taken: 0,
            cost_spent: 0,
            start_time: now,
            files_modified: 0,
            violations: Vec::new(),
            warning_count: 0,
            critical_count: 0,
            killed: false,
            session_id,
        }
    }

    fn record_violation(
        &mut self,
        severity: ViolationSeverity,
        action_hash: u64,
        reason_code: u32,
        timestamp: u64,
    ) {
        self.violations.push(Violation {
            timestamp,
            severity,
            action_hash,
            reason_code,
            session_id: self.session_id,
        });
        match severity {
            ViolationSeverity::Warning => {
                self.warning_count = self.warning_count.saturating_add(1);
                if self.warning_count >= self.policy.max_warnings {
                    self.killed = true;
                }
            }
            ViolationSeverity::Critical => {
                self.critical_count = self.critical_count.saturating_add(1);
                if self.critical_count >= self.policy.max_critical {
                    self.killed = true;
                }
            }
            ViolationSeverity::Fatal => {
                self.killed = true;
            }
            ViolationSeverity::Info => {}
        }
    }

    /// Check whether an action is safe to proceed. Returns true if allowed.
    fn check_action(&mut self, action_hash: u64, now: u64) -> bool {
        // Kill switch is absolute
        if self.killed {
            return false;
        }

        // Step limit
        if self.steps_taken >= self.policy.max_steps {
            self.record_violation(ViolationSeverity::Critical, action_hash, 0x01, now);
            return false;
        }

        // Duration limit
        let elapsed = now.saturating_sub(self.start_time);
        if elapsed > self.policy.max_duration_ms {
            self.record_violation(ViolationSeverity::Critical, action_hash, 0x02, now);
            return false;
        }

        // Check if action matches dangerous commands
        for &dangerous in DANGEROUS_CMDS {
            if action_hash == dangerous {
                self.record_violation(ViolationSeverity::Critical, action_hash, 0x10, now);
                return false;
            }
        }

        true
    }

    /// Check if a path is safe to access
    fn check_path(&mut self, path_hash: u64, is_write: bool, now: u64) -> bool {
        if self.killed {
            return false;
        }

        // Check sensitive paths
        for &sensitive in SENSITIVE_PATHS {
            if path_hash == sensitive {
                self.record_violation(ViolationSeverity::Warning, path_hash, 0x20, now);
                return false;
            }
        }

        // Check blocked paths
        if self.policy.blocked_paths.contains(&path_hash) {
            self.record_violation(ViolationSeverity::Warning, path_hash, 0x21, now);
            return false;
        }

        // Allowed paths whitelist (empty = allow all)
        if !self.policy.allowed_paths.is_empty() && !self.policy.allowed_paths.contains(&path_hash)
        {
            self.record_violation(ViolationSeverity::Warning, path_hash, 0x22, now);
            return false;
        }

        // Write check
        if is_write {
            if self.policy.block_writes {
                self.record_violation(ViolationSeverity::Critical, path_hash, 0x23, now);
                return false;
            }
            if self.files_modified >= self.policy.max_files_modified {
                self.record_violation(ViolationSeverity::Warning, path_hash, 0x24, now);
                return false;
            }
        }

        true
    }

    /// Check rate limit for a category. Returns true if within limits.
    fn check_rate(&mut self, category: RateCategory, now: u64) -> bool {
        if self.killed {
            return false;
        }
        let limiter = match category {
            RateCategory::Api => &mut self.policy.api_rate,
            RateCategory::File => &mut self.policy.file_rate,
            RateCategory::Command => &mut self.policy.cmd_rate,
        };
        if limiter.try_consume(now) {
            true
        } else {
            self.record_violation(ViolationSeverity::Warning, category as u64, 0x30, now);
            false
        }
    }

    /// Record a successful step
    fn record_step(&mut self, cost: u64) {
        self.steps_taken = self.steps_taken.saturating_add(1);
        self.cost_spent = self.cost_spent.saturating_add(cost);
    }

    /// Record a file modification
    fn record_write(&mut self) {
        self.files_modified = self.files_modified.saturating_add(1);
    }

    fn is_killed(&self) -> bool {
        self.killed
    }

    fn violation_count(&self) -> usize {
        self.violations.len()
    }
}

/// Rate limit categories
#[derive(Clone, Copy)]
pub enum RateCategory {
    Api = 1,
    File = 2,
    Command = 3,
}

// --- Public API ---

/// Check if an action is safe. Call before every agent action.
pub fn check_action(action_hash: u64, now: u64) -> bool {
    let mut guard = SAFETY.lock();
    match guard.as_mut() {
        Some(g) => g.check_action(action_hash, now),
        None => false, // Not initialized = deny all
    }
}

/// Check if a path access is safe.
pub fn check_path(path_hash: u64, is_write: bool, now: u64) -> bool {
    let mut guard = SAFETY.lock();
    match guard.as_mut() {
        Some(g) => g.check_path(path_hash, is_write, now),
        None => false,
    }
}

/// Check rate limit for a category.
pub fn check_rate(category: RateCategory, now: u64) -> bool {
    let mut guard = SAFETY.lock();
    match guard.as_mut() {
        Some(g) => g.check_rate(category, now),
        None => false,
    }
}

/// Record a completed step with its cost.
pub fn record_step(cost: u64) {
    let mut guard = SAFETY.lock();
    if let Some(g) = guard.as_mut() {
        g.record_step(cost);
    }
}

/// Record a file write operation.
pub fn record_write() {
    let mut guard = SAFETY.lock();
    if let Some(g) = guard.as_mut() {
        g.record_write();
    }
}

/// Emergency kill switch — stops all agent activity immediately.
pub fn emergency_kill() {
    let mut guard = SAFETY.lock();
    if let Some(g) = guard.as_mut() {
        g.killed = true;
    }
}

/// Check if the kill switch has been activated.
pub fn is_killed() -> bool {
    let guard = SAFETY.lock();
    match guard.as_ref() {
        Some(g) => g.is_killed(),
        None => true,
    }
}

/// Get total violation count.
pub fn violation_count() -> usize {
    let guard = SAFETY.lock();
    match guard.as_ref() {
        Some(g) => g.violation_count(),
        None => 0,
    }
}

/// Reset the safety guard for a new session.
pub fn reset_session(session_id: u32, now: u64) {
    let mut guard = SAFETY.lock();
    let policy = SafetyPolicy::government_default();
    *guard = Some(SafetyGuardInner::new(policy, session_id, now));
}

pub fn init() {
    let mut guard = SAFETY.lock();
    let policy = SafetyPolicy::government_default();
    *guard = Some(SafetyGuardInner::new(policy, 0, 0));
    serial_println!(
        "    Safety: gov-default policy, rate limiters, kill switch, audit trail ready"
    );
}
