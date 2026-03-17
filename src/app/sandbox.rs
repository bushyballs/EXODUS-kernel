use crate::sync::Mutex;
use alloc::vec::Vec;

/// Sandbox policy controlling what an app can access
pub struct SandboxPolicy {
    pub allow_network: bool,
    pub allow_filesystem: bool,
    pub memory_limit_kb: usize,
}

impl SandboxPolicy {
    /// Create a default restrictive policy
    pub fn default_restrictive() -> Self {
        Self {
            allow_network: false,
            allow_filesystem: false,
            memory_limit_kb: 16384, // 16MB default limit
        }
    }

    /// Create a permissive policy (for system apps)
    pub fn permissive() -> Self {
        Self {
            allow_network: true,
            allow_filesystem: true,
            memory_limit_kb: 256 * 1024, // 256MB
        }
    }
}

/// Known sandbox operations for permission checking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxOp {
    NetworkConnect,
    NetworkListen,
    FileRead,
    FileWrite,
    FileDelete,
    ProcessSpawn,
    MemoryAllocate,
    DeviceAccess,
    ClipboardRead,
    ClipboardWrite,
    CameraAccess,
    MicrophoneAccess,
    LocationAccess,
    Unknown,
}

impl SandboxOp {
    fn from_str(s: &str) -> Self {
        match s {
            "network.connect" | "net_connect" => SandboxOp::NetworkConnect,
            "network.listen" | "net_listen" => SandboxOp::NetworkListen,
            "file.read" | "fs_read" => SandboxOp::FileRead,
            "file.write" | "fs_write" => SandboxOp::FileWrite,
            "file.delete" | "fs_delete" => SandboxOp::FileDelete,
            "process.spawn" => SandboxOp::ProcessSpawn,
            "memory.allocate" | "mem_alloc" => SandboxOp::MemoryAllocate,
            "device.access" => SandboxOp::DeviceAccess,
            "clipboard.read" => SandboxOp::ClipboardRead,
            "clipboard.write" => SandboxOp::ClipboardWrite,
            "camera" => SandboxOp::CameraAccess,
            "microphone" => SandboxOp::MicrophoneAccess,
            "location" => SandboxOp::LocationAccess,
            _ => SandboxOp::Unknown,
        }
    }
}

/// Audit log entry for sandbox violations
struct AuditEntry {
    app_id: u64,
    operation: SandboxOp,
    allowed: bool,
    timestamp: u64,
}

/// Resource accounting for a sandboxed app
struct ResourceAccount {
    memory_used_kb: usize,
    file_handles_open: u32,
    network_connections: u32,
    max_file_handles: u32,
    max_connections: u32,
}

impl ResourceAccount {
    fn new() -> Self {
        Self {
            memory_used_kb: 0,
            file_handles_open: 0,
            network_connections: 0,
            max_file_handles: 64,
            max_connections: 16,
        }
    }
}

static SANDBOX_TICK: Mutex<u64> = Mutex::new(0);

fn sb_tick() -> u64 {
    let mut t = SANDBOX_TICK.lock();
    *t = t.saturating_add(1);
    *t
}

pub struct Sandbox {
    pub app_id: u64,
    pub policy: SandboxPolicy,
    pub active: bool,
    resources: ResourceAccount,
    audit_log: Vec<AuditEntry>,
    max_audit_entries: usize,
    violation_count: u64,
    check_count: u64,
    /// Additional granted permissions beyond the base policy
    extra_permissions: Vec<SandboxOp>,
}

impl Sandbox {
    pub fn new(app_id: u64) -> Self {
        crate::serial_println!("[app::sandbox] created sandbox for app {}", app_id);
        Self {
            app_id,
            policy: SandboxPolicy::default_restrictive(),
            active: true,
            resources: ResourceAccount::new(),
            audit_log: Vec::new(),
            max_audit_entries: 256,
            violation_count: 0,
            check_count: 0,
            extra_permissions: Vec::new(),
        }
    }

    /// Create a sandbox with a specific policy
    pub fn with_policy(app_id: u64, policy: SandboxPolicy) -> Self {
        let mut sb = Self::new(app_id);
        sb.policy = policy;
        sb
    }

    /// Grant an additional permission
    pub fn grant(&mut self, op: SandboxOp) {
        for existing in &self.extra_permissions {
            if *existing == op {
                return;
            }
        }
        self.extra_permissions.push(op);
        crate::serial_println!("[app::sandbox] app {}: granted {:?}", self.app_id, op);
    }

    /// Revoke a previously granted permission
    pub fn revoke(&mut self, op: SandboxOp) {
        let mut i = 0;
        while i < self.extra_permissions.len() {
            if self.extra_permissions[i] == op {
                self.extra_permissions.remove(i);
                crate::serial_println!("[app::sandbox] app {}: revoked {:?}", self.app_id, op);
            } else {
                i += 1;
            }
        }
    }

    /// Enforce the sandbox policy before an operation
    pub fn check(&self, operation: &str) -> bool {
        if !self.active {
            return false;
        }

        let op = SandboxOp::from_str(operation);
        let allowed = self.check_op(op);

        // We'd log the audit entry but can't mutate self through &self
        // In practice, the caller would use check_mut
        if !allowed {
            crate::serial_println!("[app::sandbox] app {}: DENIED '{}'", self.app_id, operation);
        }

        allowed
    }

    /// Check and audit an operation (mutable version)
    pub fn check_mut(&mut self, operation: &str) -> bool {
        self.check_count = self.check_count.saturating_add(1);

        let op = SandboxOp::from_str(operation);
        let allowed = self.check_op(op);

        // Audit log
        if self.audit_log.len() < self.max_audit_entries {
            self.audit_log.push(AuditEntry {
                app_id: self.app_id,
                operation: op,
                allowed,
                timestamp: sb_tick(),
            });
        }

        if !allowed {
            self.violation_count = self.violation_count.saturating_add(1);
            crate::serial_println!(
                "[app::sandbox] app {}: DENIED '{}' (violation #{})",
                self.app_id,
                operation,
                self.violation_count
            );

            // Auto-terminate if too many violations
            if self.violation_count > 100 {
                crate::serial_println!(
                    "[app::sandbox] app {}: exceeded violation limit, deactivating",
                    self.app_id
                );
                self.active = false;
            }
        }

        allowed
    }

    /// Internal permission check logic
    fn check_op(&self, op: SandboxOp) -> bool {
        // Check extra permissions first
        for granted in &self.extra_permissions {
            if *granted == op {
                return true;
            }
        }

        // Check against base policy
        match op {
            SandboxOp::NetworkConnect | SandboxOp::NetworkListen => {
                if !self.policy.allow_network {
                    return false;
                }
                // Check connection limit
                if self.resources.network_connections >= self.resources.max_connections {
                    return false;
                }
                true
            }
            SandboxOp::FileRead | SandboxOp::FileWrite | SandboxOp::FileDelete => {
                if !self.policy.allow_filesystem {
                    return false;
                }
                if self.resources.file_handles_open >= self.resources.max_file_handles {
                    return false;
                }
                true
            }
            SandboxOp::MemoryAllocate => {
                self.resources.memory_used_kb < self.policy.memory_limit_kb
            }
            SandboxOp::ProcessSpawn => false, // never allowed by default
            SandboxOp::DeviceAccess => false, // never allowed by default
            SandboxOp::ClipboardRead | SandboxOp::ClipboardWrite => true, // generally safe
            SandboxOp::CameraAccess | SandboxOp::MicrophoneAccess | SandboxOp::LocationAccess => {
                false
            }
            SandboxOp::Unknown => {
                crate::serial_println!(
                    "[app::sandbox] app {}: unknown operation denied",
                    self.app_id
                );
                false
            }
        }
    }

    /// Record a resource allocation
    pub fn record_alloc(&mut self, memory_kb: usize) -> bool {
        let new_total = self.resources.memory_used_kb + memory_kb;
        if new_total > self.policy.memory_limit_kb {
            crate::serial_println!(
                "[app::sandbox] app {}: memory limit exceeded ({} + {} > {} KB)",
                self.app_id,
                self.resources.memory_used_kb,
                memory_kb,
                self.policy.memory_limit_kb
            );
            return false;
        }
        self.resources.memory_used_kb = new_total;
        true
    }

    /// Record a resource deallocation
    pub fn record_free(&mut self, memory_kb: usize) {
        self.resources.memory_used_kb = self.resources.memory_used_kb.saturating_sub(memory_kb);
    }

    /// Tear down the sandbox and release resources
    pub fn destroy(&mut self) {
        crate::serial_println!(
            "[app::sandbox] destroying sandbox for app {}: {} checks, {} violations",
            self.app_id,
            self.check_count,
            self.violation_count
        );
        self.active = false;
        self.resources = ResourceAccount::new();
        self.audit_log.clear();
        self.extra_permissions.clear();
    }

    /// Get violation count
    pub fn violations(&self) -> u64 {
        self.violation_count
    }

    /// Get memory usage
    pub fn memory_used_kb(&self) -> usize {
        self.resources.memory_used_kb
    }
}

static SANDBOX_MGR: Mutex<Option<Vec<Sandbox>>> = Mutex::new(None);

pub fn init() {
    let mut mgr = SANDBOX_MGR.lock();
    *mgr = Some(Vec::new());
    crate::serial_println!("[app::sandbox] sandbox subsystem initialized");
}

/// Create a sandbox for an app
pub fn create_sandbox(app_id: u64) -> bool {
    let mut mgr = SANDBOX_MGR.lock();
    if let Some(ref mut sandboxes) = *mgr {
        let sb = Sandbox::new(app_id);
        sandboxes.push(sb);
        true
    } else {
        false
    }
}
