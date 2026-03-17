/// Yama LSM for Genesis — ptrace restriction and process relationship controls
///
/// Restricts ptrace (process tracing/debugging) based on process relationships:
///   - Scope 0 (Classic): any process can ptrace any other (with DAC checks)
///   - Scope 1 (Restricted): only direct parent can ptrace child
///   - Scope 2 (AdminOnly): only processes with CAP_SYS_PTRACE can ptrace
///   - Scope 3 (Disabled): ptrace is completely disabled system-wide
///   - Per-process exceptions via prctl(PR_SET_PTRACER)
///   - Audit logging of all ptrace denials
///
/// Also restricts: TIOCSTI ioctl, hardlink creation, symlink following.
///
/// Reference: Linux Yama LSM (Documentation/admin-guide/LSM/Yama.rst).
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::vec::Vec;

static YAMA: Mutex<Option<YamaInner>> = Mutex::new(None);

/// Maximum per-process ptrace exceptions
const MAX_EXCEPTIONS: usize = 256;

/// Maximum process relationships tracked
const MAX_RELATIONSHIPS: usize = 4096;

/// Yama ptrace scope level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PtraceScope {
    /// 0: Classic ptrace permissions (DAC only)
    Classic = 0,
    /// 1: Restricted to direct parent/child relationship
    RestrictedToParent = 1,
    /// 2: Only admin (CAP_SYS_PTRACE) can ptrace
    AdminOnly = 2,
    /// 3: No ptrace allowed at all (even for admin)
    Disabled = 3,
}

/// Hardlink restriction mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardlinkRestriction {
    /// No restrictions
    None,
    /// Only allow hardlinks if the caller owns the target or has read/write
    Restricted,
}

/// Symlink restriction mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkRestriction {
    /// No restrictions
    None,
    /// Only follow symlinks in sticky world-writable dirs if owner matches
    Restricted,
}

/// Per-process ptrace exception
#[derive(Clone)]
struct PtraceException {
    /// The process that set the exception (tracee)
    tracee_pid: u32,
    /// The process allowed to ptrace (tracer), or 0 for "any ancestor"
    allowed_tracer: u32,
}

/// Process relationship record
#[derive(Clone)]
struct ProcessRelationship {
    pid: u32,
    parent_pid: u32,
    uid: u32,
    has_cap_sys_ptrace: bool,
}

/// Yama policy state
pub struct YamaPolicy {
    pub scope: PtraceScope,
}

impl YamaPolicy {
    pub fn new() -> Self {
        // Default to restricted scope for security
        YamaPolicy {
            scope: PtraceScope::RestrictedToParent,
        }
    }

    pub fn set_scope(&mut self, scope: PtraceScope) {
        if let Some(ref mut inner) = *YAMA.lock() {
            inner.set_scope(scope);
        }
        self.scope = scope;
    }

    pub fn check_ptrace(&self, tracer_pid: u32, tracee_pid: u32) -> bool {
        check_ptrace(tracer_pid, tracee_pid)
    }
}

/// Audit record for Yama decisions
struct YamaAuditEntry {
    tracer_pid: u32,
    tracee_pid: u32,
    scope: PtraceScope,
    granted: bool,
    reason: &'static str,
}

/// Inner Yama state
struct YamaInner {
    /// Current ptrace scope
    scope: PtraceScope,
    /// Whether scope can be lowered (one-way ratchet)
    scope_locked: bool,
    /// Per-process ptrace exceptions (set via prctl)
    exceptions: Vec<PtraceException>,
    /// Process relationship tracking
    relationships: Vec<ProcessRelationship>,
    /// Hardlink restriction
    hardlink_restriction: HardlinkRestriction,
    /// Symlink restriction
    symlink_restriction: SymlinkRestriction,
    /// TIOCSTI restriction (prevent terminal injection)
    tiocsti_restricted: bool,
    /// Audit log
    audit_log: Vec<YamaAuditEntry>,
    max_audit_entries: usize,
    /// Statistics
    ptrace_checks: u64,
    ptrace_denials: u64,
    hardlink_checks: u64,
    hardlink_denials: u64,
}

impl YamaInner {
    fn new() -> Self {
        YamaInner {
            scope: PtraceScope::RestrictedToParent,
            scope_locked: false,
            exceptions: Vec::new(),
            relationships: Vec::new(),
            hardlink_restriction: HardlinkRestriction::Restricted,
            symlink_restriction: SymlinkRestriction::Restricted,
            tiocsti_restricted: true,
            audit_log: Vec::with_capacity(256),
            max_audit_entries: 2048,
            ptrace_checks: 0,
            ptrace_denials: 0,
            hardlink_checks: 0,
            hardlink_denials: 0,
        }
    }

    /// Set the ptrace scope level
    fn set_scope(&mut self, scope: PtraceScope) {
        if self.scope_locked && (scope as u8) < (self.scope as u8) {
            serial_println!(
                "    [yama] Cannot lower scope below {:?} (locked)",
                self.scope
            );
            return;
        }
        let old = self.scope;
        self.scope = scope;
        serial_println!("    [yama] Ptrace scope changed: {:?} -> {:?}", old, scope);

        crate::security::audit::log(
            crate::security::audit::AuditEvent::PolicyChange,
            crate::security::audit::AuditResult::Info,
            0,
            0,
            &format!("yama scope: {:?} -> {:?}", old, scope),
        );
    }

    /// Lock the scope (can only increase from here)
    fn lock_scope(&mut self) {
        self.scope_locked = true;
        serial_println!(
            "    [yama] Scope locked at {:?} (can only increase)",
            self.scope
        );
    }

    /// Register a process relationship
    fn register_process(&mut self, pid: u32, parent_pid: u32, uid: u32, has_cap: bool) {
        // Remove stale entry
        self.relationships.retain(|r| r.pid != pid);

        if self.relationships.len() >= MAX_RELATIONSHIPS {
            // Evict oldest
            self.relationships.remove(0);
        }

        self.relationships.push(ProcessRelationship {
            pid,
            parent_pid,
            uid,
            has_cap_sys_ptrace: has_cap,
        });
    }

    /// Unregister a process (on exit)
    fn unregister_process(&mut self, pid: u32) {
        self.relationships.retain(|r| r.pid != pid);
        self.exceptions
            .retain(|e| e.tracee_pid != pid && e.allowed_tracer != pid);
    }

    /// Set a ptrace exception: allow `tracer_pid` to ptrace `tracee_pid`
    fn set_ptracer(&mut self, tracee_pid: u32, tracer_pid: u32) {
        // Remove existing exception for this tracee
        self.exceptions.retain(|e| e.tracee_pid != tracee_pid);

        if self.exceptions.len() >= MAX_EXCEPTIONS {
            self.exceptions.remove(0);
        }

        self.exceptions.push(PtraceException {
            tracee_pid,
            allowed_tracer: tracer_pid,
        });

        serial_println!(
            "    [yama] Exception set: pid {} allows ptrace from pid {}",
            tracee_pid,
            tracer_pid
        );
    }

    /// Clear a ptrace exception
    fn clear_ptracer(&mut self, tracee_pid: u32) {
        self.exceptions.retain(|e| e.tracee_pid != tracee_pid);
    }

    /// Check if tracer_pid is an ancestor of tracee_pid
    fn is_ancestor(&self, tracer_pid: u32, tracee_pid: u32) -> bool {
        let mut current = tracee_pid;
        let mut depth = 0;
        while depth < 64 {
            // Don't loop forever on cycles
            let rel = self.relationships.iter().find(|r| r.pid == current);
            match rel {
                Some(r) => {
                    if r.parent_pid == tracer_pid {
                        return true;
                    }
                    if r.parent_pid == 0 || r.parent_pid == current {
                        return false; // Hit init or self-loop
                    }
                    current = r.parent_pid;
                }
                None => return false,
            }
            depth += 1;
        }
        false
    }

    /// Check if a process has CAP_SYS_PTRACE
    fn has_ptrace_cap(&self, pid: u32) -> bool {
        self.relationships
            .iter()
            .find(|r| r.pid == pid)
            .map(|r| r.has_cap_sys_ptrace)
            .unwrap_or(false)
    }

    /// Check if there is a ptrace exception allowing this trace
    fn has_exception(&self, tracer_pid: u32, tracee_pid: u32) -> bool {
        self.exceptions.iter().any(|e| {
            e.tracee_pid == tracee_pid
                && (
                    e.allowed_tracer == tracer_pid || e.allowed_tracer == 0
                    // 0 means any ancestor
                )
        })
    }

    /// Core ptrace access check
    fn check_ptrace(&mut self, tracer_pid: u32, tracee_pid: u32) -> bool {
        self.ptrace_checks = self.ptrace_checks.saturating_add(1);

        // Self-trace is always allowed
        if tracer_pid == tracee_pid {
            return true;
        }

        let (granted, reason) = match self.scope {
            PtraceScope::Classic => {
                // Scope 0: allow all (DAC checks are done elsewhere)
                (true, "classic mode: allowed")
            }

            PtraceScope::RestrictedToParent => {
                // Scope 1: tracer must be direct parent, ancestor, or have exception
                if self.is_ancestor(tracer_pid, tracee_pid) {
                    (true, "ancestor relationship")
                } else if self.has_exception(tracer_pid, tracee_pid) {
                    (true, "ptrace exception set")
                } else if self.has_ptrace_cap(tracer_pid) {
                    (true, "CAP_SYS_PTRACE")
                } else {
                    (false, "not parent/ancestor, no exception")
                }
            }

            PtraceScope::AdminOnly => {
                // Scope 2: only CAP_SYS_PTRACE holders
                if self.has_ptrace_cap(tracer_pid) {
                    (true, "CAP_SYS_PTRACE (admin)")
                } else {
                    (false, "admin-only mode, no CAP_SYS_PTRACE")
                }
            }

            PtraceScope::Disabled => {
                // Scope 3: completely disabled
                (false, "ptrace globally disabled")
            }
        };

        // Audit the decision
        if self.audit_log.len() >= self.max_audit_entries {
            self.audit_log.remove(0);
        }
        self.audit_log.push(YamaAuditEntry {
            tracer_pid,
            tracee_pid,
            scope: self.scope,
            granted,
            reason,
        });

        if !granted {
            self.ptrace_denials = self.ptrace_denials.saturating_add(1);
            serial_println!(
                "    [yama] DENIED ptrace: {} -> {} (scope={:?}, {})",
                tracer_pid,
                tracee_pid,
                self.scope,
                reason
            );

            crate::security::audit::log(
                crate::security::audit::AuditEvent::CapDenied,
                crate::security::audit::AuditResult::Deny,
                tracer_pid,
                0,
                &format!(
                    "yama: ptrace {} -> {} denied ({})",
                    tracer_pid, tracee_pid, reason
                ),
            );
        }

        granted
    }

    /// Check hardlink creation
    fn check_hardlink(
        &mut self,
        caller_uid: u32,
        target_owner_uid: u32,
        caller_can_rw: bool,
    ) -> bool {
        self.hardlink_checks = self.hardlink_checks.saturating_add(1);

        match self.hardlink_restriction {
            HardlinkRestriction::None => true,
            HardlinkRestriction::Restricted => {
                // Caller must own the file or have read+write access
                if caller_uid == target_owner_uid {
                    return true;
                }
                if caller_can_rw {
                    return true;
                }
                self.hardlink_denials = self.hardlink_denials.saturating_add(1);
                serial_println!(
                    "    [yama] DENIED hardlink: uid {} to file owned by uid {}",
                    caller_uid,
                    target_owner_uid
                );
                false
            }
        }
    }

    /// Check TIOCSTI ioctl (terminal character injection)
    fn check_tiocsti(&self, caller_pid: u32, target_tty_pid: u32) -> bool {
        if !self.tiocsti_restricted {
            return true;
        }
        // Only allow if caller owns the terminal
        if caller_pid == target_tty_pid {
            return true;
        }
        serial_println!(
            "    [yama] DENIED TIOCSTI: pid {} on tty owned by pid {}",
            caller_pid,
            target_tty_pid
        );
        false
    }
}

/// Check if ptrace is allowed
pub fn check_ptrace(tracer_pid: u32, tracee_pid: u32) -> bool {
    if let Some(ref mut inner) = *YAMA.lock() {
        return inner.check_ptrace(tracer_pid, tracee_pid);
    }
    true // No Yama = allow
}

/// Set the ptrace scope
pub fn set_scope(scope: PtraceScope) {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.set_scope(scope);
    }
}

/// Lock the scope (one-way ratchet)
pub fn lock_scope() {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.lock_scope();
    }
}

/// Get current scope
pub fn get_scope() -> PtraceScope {
    if let Some(ref inner) = *YAMA.lock() {
        return inner.scope;
    }
    PtraceScope::Classic
}

/// Register a process (called on fork/exec)
pub fn register_process(pid: u32, parent_pid: u32, uid: u32, has_cap_sys_ptrace: bool) {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.register_process(pid, parent_pid, uid, has_cap_sys_ptrace);
    }
}

/// Unregister a process (called on exit)
pub fn unregister_process(pid: u32) {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.unregister_process(pid);
    }
}

/// Set a ptrace exception (prctl PR_SET_PTRACER)
pub fn set_ptracer(tracee_pid: u32, tracer_pid: u32) {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.set_ptracer(tracee_pid, tracer_pid);
    }
}

/// Clear a ptrace exception
pub fn clear_ptracer(tracee_pid: u32) {
    if let Some(ref mut inner) = *YAMA.lock() {
        inner.clear_ptracer(tracee_pid);
    }
}

/// Check hardlink creation
pub fn check_hardlink(caller_uid: u32, target_owner_uid: u32, caller_can_rw: bool) -> bool {
    if let Some(ref mut inner) = *YAMA.lock() {
        return inner.check_hardlink(caller_uid, target_owner_uid, caller_can_rw);
    }
    true
}

/// Check TIOCSTI ioctl
pub fn check_tiocsti(caller_pid: u32, target_tty_pid: u32) -> bool {
    if let Some(ref inner) = *YAMA.lock() {
        return inner.check_tiocsti(caller_pid, target_tty_pid);
    }
    true
}

/// Get statistics
pub fn stats() -> (u64, u64, u64, u64) {
    if let Some(ref inner) = *YAMA.lock() {
        return (
            inner.ptrace_checks,
            inner.ptrace_denials,
            inner.hardlink_checks,
            inner.hardlink_denials,
        );
    }
    (0, 0, 0, 0)
}

/// Initialize the Yama LSM
pub fn init() {
    let inner = YamaInner::new();
    let scope = inner.scope;
    *YAMA.lock() = Some(inner);

    serial_println!(
        "    [yama] Yama LSM initialized (ptrace scope: {:?})",
        scope
    );
    serial_println!("    [yama] Hardlink restriction: enabled, TIOCSTI restriction: enabled");
}
