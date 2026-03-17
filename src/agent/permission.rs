use crate::sync::Mutex;
/// Agent permission system — scope-based RBAC with trust levels
///
/// Part of the AIOS agent layer. Gates every agent action behind
/// explicit permission scopes. Supports trust escalation with
/// audit trails for government compliance.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Permission scope for agent capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    ReadFiles,
    WriteFiles,
    Network,
    Execute,
    SystemConfig,
    KillProcess,
    InstallPackage,
    ModifyFirewall,
    AccessSecrets,
    EscalatePrivilege,
}

/// Trust level determines the default permission set
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    Untrusted = 0, // No permissions, everything denied
    Minimal = 1,   // Read-only file access
    Standard = 2,  // Read + limited write + limited execute
    Elevated = 3,  // Most ops allowed, destructive still prompted
    Admin = 4,     // Full access (requires explicit user grant)
}

/// A recorded permission grant for audit
#[derive(Clone, Copy)]
pub struct PermissionGrant {
    pub scope: Scope,
    pub granted_at: u64,
    pub session_only: bool,    // Revoked when session ends
    pub granted_by_user: bool, // True if user explicitly approved
}

/// A recorded permission denial for audit
#[derive(Clone, Copy)]
pub struct PermissionDenial {
    pub scope: Scope,
    pub denied_at: u64,
    pub reason_code: u32,
}

struct PermissionManagerInner {
    trust_level: TrustLevel,
    grants: Vec<PermissionGrant>,
    denials: Vec<PermissionDenial>,
    session_id: u32,
    total_checks: u64,
    total_denied: u64,
    total_granted: u64,
}

static PERMISSIONS: Mutex<Option<PermissionManagerInner>> = Mutex::new(None);

impl PermissionManagerInner {
    fn new() -> Self {
        PermissionManagerInner {
            trust_level: TrustLevel::Standard,
            grants: Vec::new(),
            denials: Vec::new(),
            session_id: 0,
            total_checks: 0,
            total_denied: 0,
            total_granted: 0,
        }
    }

    /// Check if a scope is granted based on trust level + explicit grants
    fn is_allowed(&mut self, scope: Scope, now: u64) -> bool {
        self.total_checks = self.total_checks.saturating_add(1);

        // Explicit grants always win
        for grant in &self.grants {
            if grant.scope == scope {
                self.total_granted = self.total_granted.saturating_add(1);
                return true;
            }
        }

        // Trust-level-based defaults
        let allowed = match self.trust_level {
            TrustLevel::Untrusted => false,
            TrustLevel::Minimal => matches!(scope, Scope::ReadFiles),
            TrustLevel::Standard => {
                matches!(scope, Scope::ReadFiles | Scope::WriteFiles | Scope::Execute)
            }
            TrustLevel::Elevated => !matches!(
                scope,
                Scope::AccessSecrets | Scope::EscalatePrivilege | Scope::ModifyFirewall
            ),
            TrustLevel::Admin => true,
        };

        if allowed {
            self.total_granted = self.total_granted.saturating_add(1);
        } else {
            self.denials.push(PermissionDenial {
                scope,
                denied_at: now,
                reason_code: 0x01,
            });
            self.total_denied = self.total_denied.saturating_add(1);
        }
        allowed
    }

    /// Grant a specific scope (e.g., after user approval)
    fn grant(&mut self, scope: Scope, now: u64, session_only: bool, by_user: bool) {
        // Don't duplicate grants
        if self.grants.iter().any(|g| g.scope == scope) {
            return;
        }
        self.grants.push(PermissionGrant {
            scope,
            granted_at: now,
            session_only,
            granted_by_user: by_user,
        });
    }

    /// Revoke a specific scope
    fn revoke(&mut self, scope: Scope) {
        self.grants.retain(|g| g.scope != scope);
    }

    /// Revoke all session-only grants (call at session end)
    fn clear_session_grants(&mut self) {
        self.grants.retain(|g| !g.session_only);
    }

    /// Set the trust level
    fn set_trust_level(&mut self, level: TrustLevel) {
        self.trust_level = level;
    }

    /// Check if a destructive action requires confirmation
    fn requires_confirmation(&self, scope: Scope) -> bool {
        match scope {
            Scope::WriteFiles
            | Scope::Execute
            | Scope::KillProcess
            | Scope::InstallPackage
            | Scope::ModifyFirewall
            | Scope::SystemConfig
            | Scope::AccessSecrets
            | Scope::EscalatePrivilege => self.trust_level < TrustLevel::Admin,
            Scope::ReadFiles | Scope::Network => false,
        }
    }
}

// --- Public API ---

/// Check if a permission scope is currently allowed
pub fn is_allowed(scope: Scope, now: u64) -> bool {
    let mut mgr = PERMISSIONS.lock();
    match mgr.as_mut() {
        Some(m) => m.is_allowed(scope, now),
        None => false,
    }
}

/// Grant a permission scope
pub fn grant(scope: Scope, now: u64, session_only: bool) {
    let mut mgr = PERMISSIONS.lock();
    if let Some(m) = mgr.as_mut() {
        m.grant(scope, now, session_only, true);
    }
}

/// Revoke a permission scope
pub fn revoke(scope: Scope) {
    let mut mgr = PERMISSIONS.lock();
    if let Some(m) = mgr.as_mut() {
        m.revoke(scope);
    }
}

/// Clear session-only grants
pub fn clear_session() {
    let mut mgr = PERMISSIONS.lock();
    if let Some(m) = mgr.as_mut() {
        m.clear_session_grants();
    }
}

/// Set trust level
pub fn set_trust_level(level: TrustLevel) {
    let mut mgr = PERMISSIONS.lock();
    if let Some(m) = mgr.as_mut() {
        m.set_trust_level(level);
    }
}

/// Check if a scope requires user confirmation
pub fn requires_confirmation(scope: Scope) -> bool {
    let mgr = PERMISSIONS.lock();
    match mgr.as_ref() {
        Some(m) => m.requires_confirmation(scope),
        None => true,
    }
}

pub fn init() {
    let mut mgr = PERMISSIONS.lock();
    let mut m = PermissionManagerInner::new();
    // Default grants: read files and network
    m.grant(Scope::ReadFiles, 0, false, false);
    m.grant(Scope::Network, 0, false, false);
    *mgr = Some(m);
    serial_println!("    Permissions: RBAC with trust levels, scope grants, audit trail ready");
}
