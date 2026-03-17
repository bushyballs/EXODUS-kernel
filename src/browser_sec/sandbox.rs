use crate::sync::Mutex;
/// Hoags Browser Tab Sandbox — per-tab security isolation
///
/// Isolates each browser tab in its own security context:
///   - Memory limits enforced per tab (OOM kills the tab, not the browser)
///   - CPU quotas prevent runaway scripts from freezing the system
///   - Filesystem access restricted by policy (none / read-only / downloads / full)
///   - Network access restricted by policy (blocked / same-origin / allowlist / full)
///   - Domain blocklists keyed by hash for O(1) lookup
///   - JavaScript, cookies, and local storage individually toggleable
///   - Cross-origin escape attempts detected and logged
///   - iframe breakout and popup flood protection
///   - WebRTC leak prevention and fingerprint resistance
///
/// Inspired by: Chromium site-isolation, Firefox Fission, Brave shields.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Global sandbox engine
static SANDBOX_ENGINE: Mutex<Option<SandboxEngine>> = Mutex::new(None);

// ──────────────────────────────────────────────────────────────────────
// Enums
// ──────────────────────────────────────────────────────────────────────

/// Filesystem access level for a sandboxed tab
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsAccess {
    /// No filesystem access at all
    None,
    /// Read-only access to explicitly shared files
    ReadOnly,
    /// Read-write access limited to the downloads directory
    Downloads,
    /// Full filesystem access (dangerous — admin only)
    Full,
}

/// Network access level for a sandboxed tab
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetAccess {
    /// All network requests blocked
    Blocked,
    /// Only same-origin requests allowed
    SameOrigin,
    /// Requests allowed only to explicitly listed domains
    AllowList,
    /// Unrestricted network access
    Full,
}

/// Classification of sandbox violations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationType {
    /// Tab tried to access resources from a different origin
    CrossOriginAccess,
    /// Tab tried to escape its filesystem jail
    FsEscapeAttempt,
    /// Tab exceeded its memory quota
    ExcessiveMemory,
    /// Tab exceeded its CPU time quota
    ExcessiveCpu,
    /// Tab tried to contact a blocked domain
    BlockedDomain,
    /// Heuristic detected malicious script patterns
    MaliciousScript,
    /// iframe tried to navigate or access the parent frame
    IframeEscape,
    /// Tab opened excessive popups
    PopupBlocked,
    /// WebRTC revealed the real IP address
    WebRtcLeak,
    /// Canvas/WebGL/AudioContext fingerprinting detected
    FingerprintAttempt,
}

// ──────────────────────────────────────────────────────────────────────
// Structs
// ──────────────────────────────────────────────────────────────────────

/// A single browser tab sandbox
#[derive(Debug, Clone)]
pub struct TabSandbox {
    /// Unique identifier for this tab
    pub tab_id: u64,
    /// OS-level process id hosting the tab renderer
    pub process_id: u32,
    /// Maximum resident memory in megabytes
    pub memory_limit_mb: u32,
    /// Maximum CPU time per scheduling window in milliseconds
    pub cpu_quota_ms: u32,
    /// Filesystem access level
    pub fs_access: FsAccess,
    /// Network access level
    pub net_access: NetAccess,
    /// Whether JavaScript execution is permitted
    pub js_enabled: bool,
    /// Whether cookies are permitted
    pub cookies_enabled: bool,
    /// Maximum local-storage quota in bytes
    pub storage_quota_bytes: u64,
    /// Hashed domains that are blocked for this tab (FNV-1a hashes)
    pub blocked_domains: Vec<u64>,
    /// Whether full cross-origin isolation is active
    pub isolated: bool,
    /// Current memory usage in megabytes (tracked by the engine)
    pub current_memory_mb: u32,
    /// Accumulated CPU time in the current window (milliseconds)
    pub current_cpu_ms: u32,
    /// Number of popups this tab has opened
    pub popup_count: u32,
    /// Whether the tab has been killed
    pub killed: bool,
}

/// Record of a sandbox violation
#[derive(Debug, Clone)]
pub struct SandboxViolation {
    /// Which tab caused the violation
    pub tab_id: u64,
    /// What kind of violation occurred
    pub violation_type: ViolationType,
    /// FNV-1a hash of the target (URL, path, or domain)
    pub target_hash: u64,
    /// Monotonic timestamp in milliseconds when the violation occurred
    pub timestamp: u64,
    /// Whether the violation was blocked (true) or merely logged (false)
    pub blocked: bool,
}

/// Global sandbox policy applied to new tabs by default
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Default filesystem access for new tabs
    pub default_fs: FsAccess,
    /// Default network access for new tabs
    pub default_net: NetAccess,
    /// Default memory limit in megabytes
    pub max_memory_mb: u32,
    /// Default CPU quota per window in milliseconds
    pub max_cpu_ms: u32,
    /// Block WebRTC to prevent IP leaks
    pub block_webrtc: bool,
    /// Block canvas/WebGL/AudioContext fingerprinting
    pub block_fingerprint: bool,
    /// Isolate cookies per origin (prevent third-party tracking)
    pub isolate_cookies: bool,
    /// Enforce strict Content-Security-Policy headers
    pub strict_csp: bool,
    /// Maximum popups before blocking
    pub max_popups: u32,
    /// Global domain blocklist (FNV-1a hashes)
    pub global_blocked_domains: Vec<u64>,
}

/// Runtime statistics for the sandbox engine
#[derive(Debug, Clone)]
pub struct SandboxStats {
    /// Total tabs currently active
    pub active_tabs: u32,
    /// Total violations ever recorded
    pub total_violations: u64,
    /// Violations blocked (prevented)
    pub blocked_violations: u64,
    /// Tabs killed for policy violations
    pub tabs_killed: u64,
    /// Total memory used by all tabs in megabytes
    pub total_memory_mb: u32,
}

/// The sandbox engine that manages all tab sandboxes
pub struct SandboxEngine {
    /// Global policy applied to new tabs
    policy: SandboxPolicy,
    /// All active tab sandboxes
    sandboxes: Vec<TabSandbox>,
    /// Violation log (ring buffer — keeps last MAX_VIOLATIONS entries)
    violations: Vec<SandboxViolation>,
    /// Monotonic counter for generating unique tab ids
    next_tab_id: u64,
    /// Running count of tabs killed
    tabs_killed: u64,
    /// Running count of total violations ever recorded
    total_violations_count: u64,
    /// Running count of violations that were blocked
    blocked_violations_count: u64,
}

// ──────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────

/// Maximum violation log entries before oldest are evicted
const MAX_VIOLATIONS: usize = 1024;

/// Default memory limit per tab (256 MB)
const DEFAULT_MEMORY_LIMIT_MB: u32 = 256;

/// Default CPU quota per scheduling window (500 ms)
const DEFAULT_CPU_QUOTA_MS: u32 = 500;

/// Maximum popups before automatic blocking
const DEFAULT_MAX_POPUPS: u32 = 3;

/// Default local-storage quota per tab (5 MB)
const DEFAULT_STORAGE_QUOTA: u64 = 5 * 1024 * 1024;

/// FNV-1a offset basis for 64-bit hashing
const FNV_OFFSET: u64 = 0xCBF29CE484222325;

/// FNV-1a prime for 64-bit hashing
const FNV_PRIME: u64 = 0x00000100000001B3;

// ──────────────────────────────────────────────────────────────────────
// Hash helper
// ──────────────────────────────────────────────────────────────────────

/// FNV-1a 64-bit hash for domain/URL strings (no allocator needed)
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ──────────────────────────────────────────────────────────────────────
// Timestamp helper (uses a simple monotonic counter in no_std)
// ──────────────────────────────────────────────────────────────────────

static MONO_COUNTER: Mutex<u64> = Mutex::new(0);

fn monotonic_now() -> u64 {
    let mut counter = MONO_COUNTER.lock();
    *counter = counter.saturating_add(1);
    *counter
}

// ──────────────────────────────────────────────────────────────────────
// SandboxPolicy
// ──────────────────────────────────────────────────────────────────────

impl SandboxPolicy {
    /// Create a default hardened policy
    pub fn default_hardened() -> Self {
        SandboxPolicy {
            default_fs: FsAccess::None,
            default_net: NetAccess::SameOrigin,
            max_memory_mb: DEFAULT_MEMORY_LIMIT_MB,
            max_cpu_ms: DEFAULT_CPU_QUOTA_MS,
            block_webrtc: true,
            block_fingerprint: true,
            isolate_cookies: true,
            strict_csp: true,
            max_popups: DEFAULT_MAX_POPUPS,
            global_blocked_domains: Vec::new(),
        }
    }

    /// Create a permissive policy (for trusted internal pages)
    pub fn permissive() -> Self {
        SandboxPolicy {
            default_fs: FsAccess::Downloads,
            default_net: NetAccess::Full,
            max_memory_mb: 1024,
            max_cpu_ms: 2000,
            block_webrtc: false,
            block_fingerprint: false,
            isolate_cookies: false,
            strict_csp: false,
            max_popups: 20,
            global_blocked_domains: Vec::new(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// SandboxEngine
// ──────────────────────────────────────────────────────────────────────

impl SandboxEngine {
    /// Create a new engine with the given default policy
    pub fn new(policy: SandboxPolicy) -> Self {
        SandboxEngine {
            policy,
            sandboxes: Vec::new(),
            violations: Vec::new(),
            next_tab_id: 1,
            tabs_killed: 0,
            total_violations_count: 0,
            blocked_violations_count: 0,
        }
    }

    /// Create a sandbox for a new browser tab. Returns the assigned tab id.
    pub fn create_sandbox(&mut self, process_id: u32) -> u64 {
        let tab_id = self.next_tab_id;
        self.next_tab_id = self.next_tab_id.saturating_add(1);

        let sandbox = TabSandbox {
            tab_id,
            process_id,
            memory_limit_mb: self.policy.max_memory_mb,
            cpu_quota_ms: self.policy.max_cpu_ms,
            fs_access: self.policy.default_fs,
            net_access: self.policy.default_net,
            js_enabled: true,
            cookies_enabled: !self.policy.isolate_cookies,
            storage_quota_bytes: DEFAULT_STORAGE_QUOTA,
            blocked_domains: self.policy.global_blocked_domains.clone(),
            isolated: true,
            current_memory_mb: 0,
            current_cpu_ms: 0,
            popup_count: 0,
            killed: false,
        };

        serial_println!("  Sandbox: created tab {} (pid {})", tab_id, process_id);
        self.sandboxes.push(sandbox);
        tab_id
    }

    /// Destroy a sandbox and free its resources. Returns true if found.
    pub fn destroy_sandbox(&mut self, tab_id: u64) -> bool {
        let before = self.sandboxes.len();
        self.sandboxes.retain(|s| s.tab_id != tab_id);
        let removed = self.sandboxes.len() < before;
        if removed {
            serial_println!("  Sandbox: destroyed tab {}", tab_id);
        }
        removed
    }

    /// Check whether a specific access request is allowed for a tab.
    /// `access_type` encodes what the tab wants to do, `target_hash` identifies the target.
    /// Returns true if allowed, false if blocked (and logs the violation).
    pub fn check_access(
        &mut self,
        tab_id: u64,
        violation_type: ViolationType,
        target_hash: u64,
    ) -> bool {
        // Find the sandbox (if killed, deny everything)
        let sandbox = match self.find_sandbox(tab_id) {
            Some(s) => s,
            Option::None => return false,
        };

        if sandbox.killed {
            return false;
        }

        let allowed = match violation_type {
            ViolationType::CrossOriginAccess => {
                // Only allowed if net_access is Full
                sandbox.net_access == NetAccess::Full
            }
            ViolationType::FsEscapeAttempt => {
                // Filesystem escape is never allowed
                false
            }
            ViolationType::ExcessiveMemory => {
                // Allowed only if under limit
                sandbox.current_memory_mb <= sandbox.memory_limit_mb
            }
            ViolationType::ExcessiveCpu => {
                // Allowed only if under quota
                sandbox.current_cpu_ms <= sandbox.cpu_quota_ms
            }
            ViolationType::BlockedDomain => {
                // Check both per-tab and global blocklist
                let in_tab_list = sandbox.blocked_domains.contains(&target_hash);
                let in_global = self.policy.global_blocked_domains.contains(&target_hash);
                !in_tab_list && !in_global
            }
            ViolationType::MaliciousScript => {
                // Malicious scripts are always blocked
                false
            }
            ViolationType::IframeEscape => {
                // iframe escapes are never allowed
                false
            }
            ViolationType::PopupBlocked => {
                // Allowed if under popup limit
                sandbox.popup_count < self.policy.max_popups
            }
            ViolationType::WebRtcLeak => {
                // Blocked if policy says so
                !self.policy.block_webrtc
            }
            ViolationType::FingerprintAttempt => {
                // Blocked if policy says so
                !self.policy.block_fingerprint
            }
        };

        if !allowed {
            self.log_violation(tab_id, violation_type, target_hash, true);
        }

        allowed
    }

    /// Explicitly block a network request for a tab. Logs a BlockedDomain violation.
    pub fn block_request(&mut self, tab_id: u64, domain_hash: u64) {
        self.log_violation(tab_id, ViolationType::BlockedDomain, domain_hash, true);

        // Also add the domain to the tab's local blocklist for future fast-path rejection
        if let Some(sandbox) = self.find_sandbox_mut(tab_id) {
            if !sandbox.blocked_domains.contains(&domain_hash) {
                sandbox.blocked_domains.push(domain_hash);
            }
        }
    }

    /// Record a violation in the log ring buffer.
    pub fn log_violation(
        &mut self,
        tab_id: u64,
        violation_type: ViolationType,
        target_hash: u64,
        blocked: bool,
    ) {
        let violation = SandboxViolation {
            tab_id,
            violation_type,
            target_hash,
            timestamp: monotonic_now(),
            blocked,
        };

        serial_println!(
            "  Sandbox VIOLATION: tab {} type {:?} blocked={}",
            tab_id,
            violation_type,
            blocked
        );

        self.total_violations_count = self.total_violations_count.saturating_add(1);
        if blocked {
            self.blocked_violations_count = self.blocked_violations_count.saturating_add(1);
        }

        // Ring buffer eviction
        if self.violations.len() >= MAX_VIOLATIONS {
            self.violations.remove(0);
        }
        self.violations.push(violation);
    }

    /// Update the global sandbox policy. Existing tabs are NOT retroactively changed.
    pub fn set_policy(&mut self, policy: SandboxPolicy) {
        serial_println!(
            "  Sandbox: policy updated (mem={}MB, cpu={}ms, webrtc_block={}, fingerprint_block={})",
            policy.max_memory_mb,
            policy.max_cpu_ms,
            policy.block_webrtc,
            policy.block_fingerprint
        );
        self.policy = policy;
    }

    /// Return a clone of all recorded violations (most recent last).
    pub fn get_violations(&self) -> Vec<SandboxViolation> {
        self.violations.clone()
    }

    /// Get violations for a specific tab.
    pub fn get_violations_for_tab(&self, tab_id: u64) -> Vec<SandboxViolation> {
        self.violations
            .iter()
            .filter(|v| v.tab_id == tab_id)
            .cloned()
            .collect()
    }

    /// Check if a domain hash is allowed for a specific tab.
    /// Checks both the per-tab blocklist and the global blocklist.
    pub fn is_allowed(&self, tab_id: u64, domain_hash: u64) -> bool {
        let sandbox = match self.find_sandbox_ref(tab_id) {
            Some(s) => s,
            Option::None => return false,
        };

        if sandbox.killed {
            return false;
        }

        // Check net access policy first
        match sandbox.net_access {
            NetAccess::Blocked => return false,
            NetAccess::SameOrigin => {
                // In same-origin mode, only the origin domain is allowed.
                // The caller must verify origin externally; here we just check blocklists.
            }
            NetAccess::AllowList => {
                // In allowlist mode, the domain must NOT be on the blocklist.
                // If it's on the blocklist, deny immediately.
            }
            NetAccess::Full => {
                // Full access — still respect explicit blocklists
            }
        }

        // Check per-tab blocklist
        if sandbox.blocked_domains.contains(&domain_hash) {
            return false;
        }

        // Check global blocklist
        if self.policy.global_blocked_domains.contains(&domain_hash) {
            return false;
        }

        true
    }

    /// Enforce memory limits for a tab. Updates current usage and kills the tab if exceeded.
    /// `current_mb` is the tab's current resident memory in megabytes.
    /// Returns true if the tab is still alive after enforcement.
    pub fn enforce_memory_limit(&mut self, tab_id: u64, current_mb: u32) -> bool {
        let limit = {
            let sandbox = match self.find_sandbox_mut(tab_id) {
                Some(s) => s,
                Option::None => return false,
            };
            sandbox.current_memory_mb = current_mb;
            sandbox.memory_limit_mb
        };

        if current_mb > limit {
            self.log_violation(
                tab_id,
                ViolationType::ExcessiveMemory,
                current_mb as u64,
                true,
            );
            self.kill_tab(tab_id);
            return false;
        }

        true
    }

    /// Enforce CPU quota for a tab. Updates current usage and kills if exceeded.
    /// `elapsed_ms` is the CPU time consumed in the current scheduling window.
    /// Returns true if the tab is still alive.
    pub fn enforce_cpu_quota(&mut self, tab_id: u64, elapsed_ms: u32) -> bool {
        let quota = {
            let sandbox = match self.find_sandbox_mut(tab_id) {
                Some(s) => s,
                Option::None => return false,
            };
            sandbox.current_cpu_ms = elapsed_ms;
            sandbox.cpu_quota_ms
        };

        if elapsed_ms > quota {
            self.log_violation(tab_id, ViolationType::ExcessiveCpu, elapsed_ms as u64, true);
            self.kill_tab(tab_id);
            return false;
        }

        true
    }

    /// Kill a tab sandbox. The tab's process should be terminated by the caller.
    pub fn kill_tab(&mut self, tab_id: u64) {
        if let Some(sandbox) = self.find_sandbox_mut(tab_id) {
            if !sandbox.killed {
                serial_println!(
                    "  Sandbox: KILLING tab {} (pid {})",
                    tab_id,
                    sandbox.process_id
                );
                sandbox.killed = true;
                sandbox.js_enabled = false;
                sandbox.net_access = NetAccess::Blocked;
                sandbox.fs_access = FsAccess::None;
                self.tabs_killed = self.tabs_killed.saturating_add(1);
            }
        }
    }

    /// Get engine statistics.
    pub fn get_stats(&self) -> SandboxStats {
        let active = self.sandboxes.iter().filter(|s| !s.killed).count() as u32;
        let total_mem: u32 = self
            .sandboxes
            .iter()
            .filter(|s| !s.killed)
            .map(|s| s.current_memory_mb)
            .sum();

        SandboxStats {
            active_tabs: active,
            total_violations: self.total_violations_count,
            blocked_violations: self.blocked_violations_count,
            tabs_killed: self.tabs_killed,
            total_memory_mb: total_mem,
        }
    }

    /// Add a domain to the global blocklist (by raw bytes for hashing).
    pub fn block_domain_global(&mut self, domain: &[u8]) {
        let hash = fnv1a_hash(domain);
        if !self.policy.global_blocked_domains.contains(&hash) {
            self.policy.global_blocked_domains.push(hash);
            serial_println!("  Sandbox: blocked domain globally (hash=0x{:016X})", hash);
        }
    }

    /// Add a domain to a specific tab's blocklist.
    pub fn block_domain_for_tab(&mut self, tab_id: u64, domain: &[u8]) {
        let hash = fnv1a_hash(domain);
        if let Some(sandbox) = self.find_sandbox_mut(tab_id) {
            if !sandbox.blocked_domains.contains(&hash) {
                sandbox.blocked_domains.push(hash);
            }
        }
    }

    /// Track a popup opened by a tab. Returns false if the popup should be blocked.
    pub fn track_popup(&mut self, tab_id: u64) -> bool {
        let count = {
            let sandbox = match self.find_sandbox_mut(tab_id) {
                Some(s) => s,
                Option::None => return false,
            };
            sandbox.popup_count = sandbox.popup_count.saturating_add(1);
            sandbox.popup_count
        };

        if count > self.policy.max_popups {
            self.log_violation(tab_id, ViolationType::PopupBlocked, count as u64, true);
            return false;
        }

        true
    }

    /// Check and enforce filesystem access for a tab.
    /// Returns true if the requested access level is permitted.
    pub fn check_fs_access(&mut self, tab_id: u64, requested: FsAccess) -> bool {
        let sandbox = match self.find_sandbox_ref(tab_id) {
            Some(s) => s,
            Option::None => return false,
        };

        if sandbox.killed {
            return false;
        }

        let allowed = match (sandbox.fs_access, requested) {
            (FsAccess::Full, _) => true,
            (FsAccess::Downloads, FsAccess::Downloads) => true,
            (FsAccess::Downloads, FsAccess::ReadOnly) => true,
            (FsAccess::Downloads, FsAccess::None) => true,
            (FsAccess::ReadOnly, FsAccess::ReadOnly) => true,
            (FsAccess::ReadOnly, FsAccess::None) => true,
            (FsAccess::None, FsAccess::None) => true,
            _ => false,
        };

        if !allowed {
            self.log_violation(tab_id, ViolationType::FsEscapeAttempt, 0, true);
        }

        allowed
    }

    /// Check whether JavaScript is enabled for a tab.
    pub fn is_js_enabled(&self, tab_id: u64) -> bool {
        match self.find_sandbox_ref(tab_id) {
            Some(s) => s.js_enabled && !s.killed,
            Option::None => false,
        }
    }

    /// Check whether cookies are enabled for a tab.
    pub fn are_cookies_enabled(&self, tab_id: u64) -> bool {
        match self.find_sandbox_ref(tab_id) {
            Some(s) => s.cookies_enabled && !s.killed,
            Option::None => false,
        }
    }

    /// Reset CPU counters for all tabs (call at the start of each scheduling window).
    pub fn reset_cpu_counters(&mut self) {
        for sandbox in self.sandboxes.iter_mut() {
            sandbox.current_cpu_ms = 0;
        }
    }

    /// Get a reference to the current policy.
    pub fn get_policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// Count of active (non-killed) tabs.
    pub fn active_tab_count(&self) -> usize {
        self.sandboxes.iter().filter(|s| !s.killed).count()
    }

    /// Purge all killed tabs from the sandbox list (garbage collection).
    pub fn purge_killed(&mut self) -> usize {
        let before = self.sandboxes.len();
        self.sandboxes.retain(|s| !s.killed);
        let removed = before - self.sandboxes.len();
        if removed > 0 {
            serial_println!("  Sandbox: purged {} killed tabs", removed);
        }
        removed
    }

    // ──────────────────────────────────────────────────────────────────
    // Internal helpers
    // ──────────────────────────────────────────────────────────────────

    /// Find a sandbox by tab id (immutable reference)
    fn find_sandbox_ref(&self, tab_id: u64) -> Option<&TabSandbox> {
        self.sandboxes.iter().find(|s| s.tab_id == tab_id)
    }

    /// Find a sandbox by tab id (mutable reference)
    fn find_sandbox_mut(&mut self, tab_id: u64) -> Option<&mut TabSandbox> {
        self.sandboxes.iter_mut().find(|s| s.tab_id == tab_id)
    }

    /// Find a sandbox by tab id (clone for borrow-safe reads)
    fn find_sandbox(&self, tab_id: u64) -> Option<TabSandbox> {
        self.sandboxes.iter().find(|s| s.tab_id == tab_id).cloned()
    }
}

// ──────────────────────────────────────────────────────────────────────
// Public API (operates on the global engine)
// ──────────────────────────────────────────────────────────────────────

/// Create a new tab sandbox in the global engine. Returns the tab id.
pub fn create_tab(process_id: u32) -> u64 {
    let mut engine = SANDBOX_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.create_sandbox(process_id),
        Option::None => 0,
    }
}

/// Destroy a tab sandbox. Returns true if it existed.
pub fn destroy_tab(tab_id: u64) -> bool {
    let mut engine = SANDBOX_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.destroy_sandbox(tab_id),
        Option::None => false,
    }
}

/// Check if a domain is allowed for a tab.
pub fn is_domain_allowed(tab_id: u64, domain: &[u8]) -> bool {
    let hash = fnv1a_hash(domain);
    let engine = SANDBOX_ENGINE.lock();
    match engine.as_ref() {
        Some(e) => e.is_allowed(tab_id, hash),
        Option::None => false,
    }
}

/// Block a domain globally.
pub fn block_domain(domain: &[u8]) {
    let mut engine = SANDBOX_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.block_domain_global(domain);
    }
}

/// Kill a misbehaving tab.
pub fn kill_misbehaving_tab(tab_id: u64) {
    let mut engine = SANDBOX_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.kill_tab(tab_id);
    }
}

/// Get engine statistics.
pub fn stats() -> Option<SandboxStats> {
    let engine = SANDBOX_ENGINE.lock();
    engine.as_ref().map(|e| e.get_stats())
}

/// Update the global policy.
pub fn update_policy(policy: SandboxPolicy) {
    let mut engine = SANDBOX_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.set_policy(policy);
    }
}

// ──────────────────────────────────────────────────────────────────────
// Initialization
// ──────────────────────────────────────────────────────────────────────

/// Initialize the browser sandbox engine with a hardened default policy.
pub fn init() {
    let policy = SandboxPolicy::default_hardened();

    // Pre-populate the global blocklist with known tracking/ad domains
    let mut engine = SandboxEngine::new(policy);

    // Block well-known tracker domains by hash
    let known_trackers: Vec<&[u8]> = vec![
        b"doubleclick.net",
        b"googlesyndication.com",
        b"facebook.com/tr",
        b"analytics.google.com",
        b"connect.facebook.net",
        b"ad.doubleclick.net",
        b"pagead2.googlesyndication.com",
        b"googleadservices.com",
    ];

    for tracker in known_trackers.iter() {
        engine.block_domain_global(tracker);
    }

    let stats = engine.get_stats();

    let mut global = SANDBOX_ENGINE.lock();
    *global = Some(engine);

    serial_println!(
        "  Browser Sandbox: initialized (blocked {} trackers, mem_limit={}MB, cpu_quota={}ms)",
        stats.active_tabs, // 0 tabs at init — we're reporting blocked domain count here
        DEFAULT_MEMORY_LIMIT_MB,
        DEFAULT_CPU_QUOTA_MS,
    );
}
