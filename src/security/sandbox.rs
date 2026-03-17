/// Process sandboxing for Genesis
///
/// Provides seccomp-like syscall filtering:
///   - Each process has a syscall filter (allowlist/denylist)
///   - Filters are inherited by child processes
///   - Filters can only become more restrictive (no privilege escalation)
///   - Violations are logged and the process is killed
///
/// Also provides namespace isolation:
///   - PID namespace (process can't see others)
///   - Mount namespace (private filesystem view)
///   - Network namespace (isolated network stack)
///   - IPC namespace (isolated message queues)
///
/// Inspired by: Linux seccomp-bpf, FreeBSD Capsicum, Chromium sandbox.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Global sandbox registry
static SANDBOXES: Mutex<BTreeMap<u32, Sandbox>> = Mutex::new(BTreeMap::new());

/// Syscall filter action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// Allow the syscall
    Allow,
    /// Kill the process immediately
    Kill,
    /// Return an error (EPERM) without executing
    Errno,
    /// Log and allow (audit mode)
    Log,
    /// Trap — deliver SIGSYS to the process
    Trap,
}

/// A syscall filter rule
#[derive(Debug, Clone)]
pub struct SyscallRule {
    /// Syscall number (or ANY for wildcard)
    pub syscall_nr: SyscallMatch,
    /// Action to take
    pub action: FilterAction,
    /// Optional argument constraints
    pub arg_filters: Vec<ArgFilter>,
}

/// Syscall number matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallMatch {
    /// Match any syscall
    Any,
    /// Match a specific syscall number
    Exact(u32),
    /// Match a range of syscall numbers
    Range(u32, u32),
}

/// Argument filter — constrain syscall arguments
#[derive(Debug, Clone, Copy)]
pub struct ArgFilter {
    /// Which argument (0-5)
    pub arg_index: u8,
    /// Comparison operation
    pub op: ArgOp,
    /// Value to compare against
    pub value: u64,
}

/// Argument comparison operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgOp {
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    MaskedEqual(u64), // (arg & mask) == value
}

impl ArgFilter {
    pub fn matches(&self, arg_value: u64) -> bool {
        match self.op {
            ArgOp::Equal => arg_value == self.value,
            ArgOp::NotEqual => arg_value != self.value,
            ArgOp::LessThan => arg_value < self.value,
            ArgOp::GreaterThan => arg_value > self.value,
            ArgOp::MaskedEqual(mask) => (arg_value & mask) == self.value,
        }
    }
}

/// Namespace isolation flags
#[derive(Debug, Clone, Copy)]
pub struct NamespaceFlags {
    pub pid: bool,     // PID namespace isolation
    pub mount: bool,   // Mount namespace (private fs view)
    pub network: bool, // Network namespace
    pub ipc: bool,     // IPC namespace
    pub user: bool,    // User namespace (UID mapping)
}

impl NamespaceFlags {
    pub const NONE: Self = NamespaceFlags {
        pid: false,
        mount: false,
        network: false,
        ipc: false,
        user: false,
    };

    pub const FULL: Self = NamespaceFlags {
        pid: true,
        mount: true,
        network: true,
        ipc: true,
        user: true,
    };
}

/// Resource limits for sandboxed processes
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    /// Max memory (bytes, 0 = unlimited)
    pub max_memory: u64,
    /// Max CPU time (milliseconds, 0 = unlimited)
    pub max_cpu_time: u64,
    /// Max open file descriptors
    pub max_fds: u32,
    /// Max child processes
    pub max_processes: u32,
    /// Max file size (bytes)
    pub max_file_size: u64,
    /// Max network connections
    pub max_connections: u32,
}

impl ResourceLimits {
    pub const UNLIMITED: Self = ResourceLimits {
        max_memory: 0,
        max_cpu_time: 0,
        max_fds: 0,
        max_processes: 0,
        max_file_size: 0,
        max_connections: 0,
    };

    pub const RESTRICTED: Self = ResourceLimits {
        max_memory: 256 * 1024 * 1024, // 256 MB
        max_cpu_time: 60_000,          // 60 seconds
        max_fds: 64,
        max_processes: 4,
        max_file_size: 16 * 1024 * 1024, // 16 MB
        max_connections: 8,
    };

    pub const MINIMAL: Self = ResourceLimits {
        max_memory: 32 * 1024 * 1024, // 32 MB
        max_cpu_time: 10_000,         // 10 seconds
        max_fds: 8,
        max_processes: 0,           // No child processes
        max_file_size: 1024 * 1024, // 1 MB
        max_connections: 2,
    };
}

/// A process sandbox
#[derive(Debug, Clone)]
pub struct Sandbox {
    pub pid: u32,
    pub rules: Vec<SyscallRule>,
    pub default_action: FilterAction,
    pub namespaces: NamespaceFlags,
    pub limits: ResourceLimits,
    pub locked: bool, // Once locked, can't be relaxed
    pub violations: u32,
    pub max_violations: u32, // Kill after this many violations (0 = kill on first)
}

impl Sandbox {
    /// Create a new sandbox for a process
    pub fn new(pid: u32) -> Self {
        Sandbox {
            pid,
            rules: Vec::new(),
            default_action: FilterAction::Allow, // Start permissive, tighten
            namespaces: NamespaceFlags::NONE,
            limits: ResourceLimits::UNLIMITED,
            locked: false,
            violations: 0,
            max_violations: 0,
        }
    }

    /// Create a strict sandbox (default deny)
    pub fn strict(pid: u32) -> Self {
        let mut sb = Sandbox::new(pid);
        sb.default_action = FilterAction::Kill;
        sb.namespaces = NamespaceFlags::FULL;
        sb.limits = ResourceLimits::RESTRICTED;
        sb
    }

    /// Add a syscall filter rule
    pub fn add_rule(&mut self, rule: SyscallRule) -> Result<(), &'static str> {
        if self.locked {
            return Err("sandbox is locked");
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Allow a specific syscall
    pub fn allow_syscall(&mut self, nr: u32) -> Result<(), &'static str> {
        self.add_rule(SyscallRule {
            syscall_nr: SyscallMatch::Exact(nr),
            action: FilterAction::Allow,
            arg_filters: Vec::new(),
        })
    }

    /// Block a specific syscall
    pub fn deny_syscall(&mut self, nr: u32) -> Result<(), &'static str> {
        self.add_rule(SyscallRule {
            syscall_nr: SyscallMatch::Exact(nr),
            action: FilterAction::Kill,
            arg_filters: Vec::new(),
        })
    }

    /// Lock the sandbox — no more rule changes allowed
    pub fn lock(&mut self) {
        self.locked = true;
    }

    /// Evaluate a syscall against the filter
    pub fn check_syscall(&mut self, nr: u32, args: &[u64; 6]) -> FilterAction {
        // Check rules in order (first match wins)
        for rule in &self.rules {
            let nr_match = match rule.syscall_nr {
                SyscallMatch::Any => true,
                SyscallMatch::Exact(n) => n == nr,
                SyscallMatch::Range(lo, hi) => nr >= lo && nr <= hi,
            };

            if !nr_match {
                continue;
            }

            // Check argument filters
            let args_match = rule.arg_filters.iter().all(|f| {
                let idx = f.arg_index as usize;
                idx < 6 && f.matches(args[idx])
            });

            if args_match {
                if rule.action == FilterAction::Kill || rule.action == FilterAction::Errno {
                    self.violations = self.violations.saturating_add(1);
                    crate::security::audit::log(
                        crate::security::audit::AuditEvent::CapDenied,
                        crate::security::audit::AuditResult::Deny,
                        self.pid,
                        0,
                        &alloc::format!("sandbox: syscall {} denied", nr),
                    );
                }
                return rule.action;
            }
        }

        // Default action
        if self.default_action == FilterAction::Kill {
            self.violations = self.violations.saturating_add(1);
        }
        self.default_action
    }
}

/// Pre-built sandbox profiles
pub mod profiles {
    use super::*;

    /// Profile for a GUI application (display client)
    pub fn gui_app(pid: u32) -> Sandbox {
        let mut sb = Sandbox::strict(pid);
        sb.limits = ResourceLimits::RESTRICTED;
        // Allow basic syscalls
        for nr in &[0, 1, 2, 3, 5, 9, 10, 11, 12, 60] {
            // read, write, open, close, mmap, etc.
            let _ = sb.allow_syscall(*nr);
        }
        sb
    }

    /// Profile for a network service
    pub fn network_service(pid: u32) -> Sandbox {
        let mut sb = Sandbox::strict(pid);
        sb.namespaces.network = false; // Needs real network access
        sb.limits = ResourceLimits {
            max_connections: 1024,
            ..ResourceLimits::RESTRICTED
        };
        // Allow networking syscalls
        for nr in &[0, 1, 2, 3, 41, 42, 43, 44, 45, 46, 49, 50, 51] {
            let _ = sb.allow_syscall(*nr);
        }
        sb
    }

    /// Profile for an untrusted/downloaded program
    pub fn untrusted(pid: u32) -> Sandbox {
        let mut sb = Sandbox::strict(pid);
        sb.namespaces = NamespaceFlags::FULL;
        sb.limits = ResourceLimits::MINIMAL;
        sb.max_violations = 0; // Kill on first violation
                               // Allow only the most basic syscalls
        for nr in &[0, 1, 3, 60] {
            // read, write, close, exit
            let _ = sb.allow_syscall(*nr);
        }
        sb.lock();
        sb
    }
}

/// Create a sandbox for a process
pub fn create(pid: u32) -> Sandbox {
    Sandbox::new(pid)
}

/// Register a sandbox
pub fn register(sandbox: Sandbox) {
    let pid = sandbox.pid;
    SANDBOXES.lock().insert(pid, sandbox);
    serial_println!("  [sandbox] Process {} sandboxed", pid);
}

/// Check a syscall for a sandboxed process
pub fn check_syscall(pid: u32, nr: u32, args: &[u64; 6]) -> FilterAction {
    SANDBOXES
        .lock()
        .get_mut(&pid)
        .map(|sb| sb.check_syscall(nr, args))
        .unwrap_or(FilterAction::Allow) // Non-sandboxed processes pass
}

/// Remove sandbox when process exits
pub fn remove(pid: u32) {
    SANDBOXES.lock().remove(&pid);
}

/// Initialize the sandbox subsystem
pub fn init() {
    serial_println!("  [sandbox] Process sandbox framework initialized");
}
