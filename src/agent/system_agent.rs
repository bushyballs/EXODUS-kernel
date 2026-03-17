use crate::sync::Mutex;
use alloc::string::String;
/// System administration agent with privilege control
///
/// Part of the AIOS agent layer. Provides system management actions
/// (processes, disk, network, packages, services) gated behind
/// privilege levels with full audit logging.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Actions the system agent can perform
#[derive(Debug, Clone)]
pub enum SysAction {
    ListProcesses,
    KillProcess(u32),
    CheckDisk,
    DiskUsage(String),
    NetworkStatus,
    ListInterfaces,
    InstallPackage(String),
    RemovePackage(String),
    ServiceStart(String),
    ServiceStop(String),
    ServiceStatus(String),
    SystemInfo,
    Uptime,
    MemoryUsage,
    LogQuery(String), // Search system logs
}

/// Result of a system action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysResult {
    Success,
    PermissionDenied,
    ProcessNotFound,
    ServiceNotFound,
    PackageNotFound,
    AlreadyRunning,
    AlreadyStopped,
    DiskFull,
    NetworkError,
    RateLimited,
}

/// Privilege level for system operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrivilegeLevel {
    ReadOnly = 0, // Can only query status
    Operator = 1, // Can start/stop services, kill owned processes
    Admin = 2,    // Can install packages, kill any process
    Root = 3,     // Full system access
}

/// Minimum privilege required for each action
fn required_privilege(action: &SysAction) -> PrivilegeLevel {
    match action {
        SysAction::ListProcesses
        | SysAction::CheckDisk
        | SysAction::DiskUsage(_)
        | SysAction::NetworkStatus
        | SysAction::ListInterfaces
        | SysAction::ServiceStatus(_)
        | SysAction::SystemInfo
        | SysAction::Uptime
        | SysAction::MemoryUsage
        | SysAction::LogQuery(_) => PrivilegeLevel::ReadOnly,
        SysAction::KillProcess(_) | SysAction::ServiceStart(_) | SysAction::ServiceStop(_) => {
            PrivilegeLevel::Operator
        }
        SysAction::InstallPackage(_) | SysAction::RemovePackage(_) => PrivilegeLevel::Admin,
    }
}

/// Audit log entry
#[derive(Clone, Copy)]
struct AuditEntry {
    action_hash: u64,
    result: SysResult,
    privilege_used: PrivilegeLevel,
    timestamp: u64,
    session_id: u32,
}

struct SystemAgentInner {
    privilege: PrivilegeLevel,
    audit_log: Vec<AuditEntry>,
    max_audit: usize,
    // Rate limiting
    actions_this_session: u32,
    max_actions_per_session: u32,
    // Protected processes (can't be killed)
    protected_pids: Vec<u32>,
    // Protected services (can't be stopped)
    protected_services: Vec<u64>,
    // Stats
    total_actions: u64,
    total_denied: u64,
    session_id: u32,
}

static SYS_AGENT: Mutex<Option<SystemAgentInner>> = Mutex::new(None);

/// PIDs that should never be killed
const PROTECTED_PIDS: &[u32] = &[
    1, // init/systemd
    2, // kthreadd
];

impl SystemAgentInner {
    fn new() -> Self {
        let mut protected = Vec::new();
        for &pid in PROTECTED_PIDS {
            protected.push(pid);
        }
        SystemAgentInner {
            privilege: PrivilegeLevel::Operator, // Default to operator
            audit_log: Vec::new(),
            max_audit: 500,
            actions_this_session: 0,
            max_actions_per_session: 200,
            protected_pids: protected,
            protected_services: Vec::new(),
            total_actions: 0,
            total_denied: 0,
            session_id: 0,
        }
    }

    /// Execute a system action with privilege checking
    fn do_action(&mut self, action: &SysAction, timestamp: u64) -> SysResult {
        // Rate limit
        if self.actions_this_session >= self.max_actions_per_session {
            self.total_denied = self.total_denied.saturating_add(1);
            return SysResult::RateLimited;
        }
        self.actions_this_session = self.actions_this_session.saturating_add(1);
        self.total_actions = self.total_actions.saturating_add(1);

        // Privilege check
        let required = required_privilege(action);
        if self.privilege < required {
            self.record_audit(action, SysResult::PermissionDenied, timestamp);
            self.total_denied = self.total_denied.saturating_add(1);
            return SysResult::PermissionDenied;
        }

        // Additional safety checks
        let result = match action {
            SysAction::KillProcess(pid) => {
                if self.protected_pids.contains(pid) {
                    SysResult::PermissionDenied
                } else {
                    SysResult::Success
                }
            }
            SysAction::ServiceStop(ref name) => {
                let name_hash = simple_hash(name);
                if self.protected_services.contains(&name_hash) {
                    SysResult::PermissionDenied
                } else {
                    SysResult::Success
                }
            }
            SysAction::RemovePackage(_) => {
                // Extra caution: admin-only and logged
                SysResult::Success
            }
            _ => SysResult::Success,
        };

        self.record_audit(action, result, timestamp);
        result
    }

    fn record_audit(&mut self, action: &SysAction, result: SysResult, timestamp: u64) {
        if self.audit_log.len() >= self.max_audit {
            self.audit_log.remove(0);
        }
        self.audit_log.push(AuditEntry {
            action_hash: action_to_hash(action),
            result,
            privilege_used: self.privilege,
            timestamp,
            session_id: self.session_id,
        });
    }

    fn set_privilege(&mut self, level: PrivilegeLevel) {
        self.privilege = level;
    }

    fn protect_pid(&mut self, pid: u32) {
        if !self.protected_pids.contains(&pid) {
            self.protected_pids.push(pid);
        }
    }

    fn protect_service(&mut self, name_hash: u64) {
        if !self.protected_services.contains(&name_hash) {
            self.protected_services.push(name_hash);
        }
    }

    fn reset_session(&mut self, session_id: u32) {
        self.actions_this_session = 0;
        self.session_id = session_id;
    }

    fn get_audit_count(&self) -> usize {
        self.audit_log.len()
    }
}

/// Hash an action type for audit logging
fn action_to_hash(action: &SysAction) -> u64 {
    match action {
        SysAction::ListProcesses => 0x01,
        SysAction::KillProcess(pid) => 0x02 | ((*pid as u64) << 8),
        SysAction::CheckDisk => 0x03,
        SysAction::DiskUsage(_) => 0x04,
        SysAction::NetworkStatus => 0x05,
        SysAction::ListInterfaces => 0x06,
        SysAction::InstallPackage(_) => 0x07,
        SysAction::RemovePackage(_) => 0x08,
        SysAction::ServiceStart(_) => 0x09,
        SysAction::ServiceStop(_) => 0x0A,
        SysAction::ServiceStatus(_) => 0x0B,
        SysAction::SystemInfo => 0x0C,
        SysAction::Uptime => 0x0D,
        SysAction::MemoryUsage => 0x0E,
        SysAction::LogQuery(_) => 0x0F,
    }
}

/// Simple hash helper
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// --- Public API ---

/// Execute a system action
pub fn do_action(action: &SysAction, timestamp: u64) -> SysResult {
    let mut agent = SYS_AGENT.lock();
    match agent.as_mut() {
        Some(a) => a.do_action(action, timestamp),
        None => SysResult::PermissionDenied,
    }
}

/// Set privilege level
pub fn set_privilege(level: PrivilegeLevel) {
    let mut agent = SYS_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.set_privilege(level);
    }
}

/// Protect a PID from being killed
pub fn protect_pid(pid: u32) {
    let mut agent = SYS_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.protect_pid(pid);
    }
}

/// Protect a service from being stopped
pub fn protect_service(name_hash: u64) {
    let mut agent = SYS_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.protect_service(name_hash);
    }
}

/// Reset for new session
pub fn reset_session(session_id: u32) {
    let mut agent = SYS_AGENT.lock();
    if let Some(a) = agent.as_mut() {
        a.reset_session(session_id);
    }
}

/// Get audit log size
pub fn audit_count() -> usize {
    let agent = SYS_AGENT.lock();
    match agent.as_ref() {
        Some(a) => a.get_audit_count(),
        None => 0,
    }
}

pub fn init() {
    let mut agent = SYS_AGENT.lock();
    *agent = Some(SystemAgentInner::new());
    serial_println!("    System agent: privilege-gated ops, protected PIDs, audit logging ready");
}
