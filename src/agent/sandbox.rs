use crate::sync::Mutex;
/// Sandboxed execution for Genesis agent
///
/// Safe command execution with permission boundaries,
/// resource limits, filesystem isolation, network controls.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SandboxPolicy {
    Unrestricted, // Full access (root/admin)
    Standard,     // Normal user permissions
    Restricted,   // Read-only + whitelisted commands
    Paranoid,     // No execution, read-only, no network
}

#[derive(Clone, Copy, PartialEq)]
pub enum CommandVerdict {
    Allow,
    Deny,
    AskUser,
    AllowOnce,
}

#[derive(Clone, Copy)]
struct AllowRule {
    command_hash: u64,     // Hash of command name (e.g., "git", "cargo")
    allow_args: bool,      // Allow any arguments?
    arg_pattern_hash: u64, // 0 = any, else match pattern
}

#[derive(Clone, Copy)]
struct DenyRule {
    pattern_hash: u64,
    reason_hash: u64,
}

#[derive(Clone, Copy)]
struct ExecutionRecord {
    command_hash: u64,
    exit_code: i32,
    duration_ms: u32,
    stdout_size: u32,
    stderr_size: u32,
    timestamp: u64,
    was_killed: bool,
}

struct SandboxManager {
    policy: SandboxPolicy,
    allow_list: Vec<AllowRule>,
    deny_list: Vec<DenyRule>,
    exec_history: Vec<ExecutionRecord>,
    // Resource limits
    max_exec_time_ms: u32,
    max_output_bytes: u32,
    max_concurrent: u8,
    active_count: u8,
    // Filesystem boundaries
    allowed_paths: Vec<u64>, // Hashes of allowed directory prefixes
    denied_paths: Vec<u64>,
    // Network
    network_allowed: bool,
    allowed_hosts: Vec<u64>,
    // Stats
    total_executed: u64,
    total_denied: u32,
    total_killed: u32,
}

static SANDBOX: Mutex<Option<SandboxManager>> = Mutex::new(None);

impl SandboxManager {
    fn new() -> Self {
        SandboxManager {
            policy: SandboxPolicy::Standard,
            allow_list: Vec::new(),
            deny_list: Vec::new(),
            exec_history: Vec::new(),
            max_exec_time_ms: 120_000, // 2 min
            max_output_bytes: 100_000,
            max_concurrent: 4,
            active_count: 0,
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            network_allowed: true,
            allowed_hosts: Vec::new(),
            total_executed: 0,
            total_denied: 0,
            total_killed: 0,
        }
    }

    fn setup_defaults(&mut self) {
        // Safe commands always allowed
        let safe_cmds = [
            0x676974,
            0x636172676F,
            0x6E706D,
            0x707974686F6E, // git, cargo, npm, python
            0x6C73,
            0x707764,
            0x636174,
            0x67726570, // ls, pwd, cat, grep
            0x66696E64,
            0x776869636820,
            0x656E76, // find, which, env
            0x72757374632C,
            0x72757374757020, // rustc, rustup
        ];
        for &cmd in &safe_cmds {
            self.allow_list.push(AllowRule {
                command_hash: cmd,
                allow_args: true,
                arg_pattern_hash: 0,
            });
        }
        // Dangerous commands always denied
        let dangerous = [
            0x726D202D7266,
            0x6D6B6673, // rm -rf, mkfs
            0x6464,
            0x666F726D6174,     // dd, format
            0x73687574646F776E, // shutdown
        ];
        for &cmd in &dangerous {
            self.deny_list.push(DenyRule {
                pattern_hash: cmd,
                reason_hash: 0xDA_E0_0005,
            });
        }
    }

    fn check_command(&self, command_hash: u64) -> CommandVerdict {
        // Deny list takes priority
        for rule in &self.deny_list {
            if rule.pattern_hash == command_hash {
                return CommandVerdict::Deny;
            }
        }
        // Check allow list
        for rule in &self.allow_list {
            if rule.command_hash == command_hash {
                return CommandVerdict::Allow;
            }
        }
        // Policy-based default
        match self.policy {
            SandboxPolicy::Unrestricted => CommandVerdict::Allow,
            SandboxPolicy::Standard => CommandVerdict::AskUser,
            SandboxPolicy::Restricted => CommandVerdict::Deny,
            SandboxPolicy::Paranoid => CommandVerdict::Deny,
        }
    }

    fn check_path(&self, path_hash: u64) -> bool {
        // If denied paths list has this path, deny
        if self.denied_paths.contains(&path_hash) {
            return false;
        }
        // If allowed paths list is empty, allow all (standard mode)
        if self.allowed_paths.is_empty() {
            return true;
        }
        self.allowed_paths.contains(&path_hash)
    }

    fn record_execution(
        &mut self,
        command_hash: u64,
        exit_code: i32,
        duration_ms: u32,
        stdout_size: u32,
        stderr_size: u32,
        timestamp: u64,
        killed: bool,
    ) {
        self.exec_history.push(ExecutionRecord {
            command_hash,
            exit_code,
            duration_ms,
            stdout_size,
            stderr_size,
            timestamp,
            was_killed: killed,
        });
        self.total_executed = self.total_executed.saturating_add(1);
        if killed {
            self.total_killed = self.total_killed.saturating_add(1);
        }
    }

    fn can_execute(&self) -> bool {
        self.active_count < self.max_concurrent && self.policy != SandboxPolicy::Paranoid
    }

    fn add_allow_rule(&mut self, command_hash: u64) {
        self.allow_list.push(AllowRule {
            command_hash,
            allow_args: true,
            arg_pattern_hash: 0,
        });
    }

    fn set_policy(&mut self, policy: SandboxPolicy) {
        self.policy = policy;
    }
}

pub fn init() {
    let mut sb = SANDBOX.lock();
    let mut mgr = SandboxManager::new();
    mgr.setup_defaults();
    *sb = Some(mgr);
    serial_println!("    Sandbox: policy-based execution, allow/deny lists, resource limits ready");
}
