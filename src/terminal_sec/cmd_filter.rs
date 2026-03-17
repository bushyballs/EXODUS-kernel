use crate::sync::Mutex;
/// Terminal command security filter
///
/// Prevents dangerous commands, injection attacks, and validates inputs
/// before execution in the Genesis terminal/shell. Every command passes
/// through a multi-stage pipeline:
///   1. Injection detection (backticks, pipes, path traversal, null bytes)
///   2. Pattern matching against 50+ predefined filter rules
///   3. Risk assessment (Safe -> Critical -> Blocked)
///   4. Action determination (Allow, Warn, Confirm, Block, Log, Quarantine)
///   5. User/path authorization checks
///
/// All pattern matching uses pre-computed hashes (FNV-1a) instead of
/// storing plaintext patterns, reducing memory and preventing the filter
/// rules themselves from being exfiltrated.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

static CMD_FILTER: Mutex<Option<CommandFilter>> = Mutex::new(None);

/// Maximum number of filter rules
const MAX_RULES: usize = 128;
/// Maximum blocked pattern hashes
const MAX_BLOCKED_PATTERNS: usize = 256;
/// Maximum allowed user hashes
const MAX_ALLOWED_USERS: usize = 64;
/// Maximum restricted path hashes
const MAX_RESTRICTED_PATHS: usize = 128;
/// Maximum injection pattern hashes
const MAX_INJECTION_PATTERNS: usize = 64;
/// Maximum whitelist entries
const MAX_WHITELIST: usize = 64;
/// Maximum blacklist entries
const MAX_BLACKLIST: usize = 64;
/// Maximum command length before automatic rejection
const MAX_COMMAND_LENGTH: usize = 4096;
/// Maximum nesting depth for subshells/pipes
const MAX_NESTING_DEPTH: usize = 8;

// --- FNV-1a hash constants for 64-bit ---
const FNV_OFFSET_BASIS: u64 = 0xCBF29CE484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

/// Compute FNV-1a hash of a byte slice
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Compute FNV-1a hash of a string (case-insensitive for matching)
fn fnv1a_hash_lower(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        let lower = if byte >= b'A' && byte <= b'Z' {
            byte + 32
        } else {
            byte
        };
        hash ^= lower as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Risk level for a command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRisk {
    /// No risk, fully safe command
    Safe,
    /// Minor risk, standard operations
    Low,
    /// Moderate risk, may affect system state
    Medium,
    /// High risk, can cause data loss or security issues
    High,
    /// Critical risk, can destroy the system
    Critical,
    /// Unconditionally blocked, never allowed
    Blocked,
}

/// Action to take when a rule matches
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// Allow the command to execute
    Allow,
    /// Allow but display a warning to the user
    Warn,
    /// Require explicit user confirmation before executing
    Confirm,
    /// Block execution entirely
    Block,
    /// Allow but log the command for audit
    Log,
    /// Block and quarantine the session for review
    Quarantine,
}

/// Types of injection attacks detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionType {
    /// Command chaining via ; && || etc.
    CommandInjection,
    /// Path traversal via ../ or encoded variants
    PathTraversal,
    /// Shell escape characters or control sequences
    ShellEscape,
    /// Pipe injection to redirect output
    PipeInjection,
    /// Backtick or $() command substitution
    BacktickExec,
    /// Variable expansion via $VAR or ${VAR}
    VariableExpansion,
    /// Glob pattern expansion via * ? []
    GlobExpansion,
    /// Null byte injection to truncate strings
    NullByte,
}

/// A single filter rule
#[derive(Debug, Clone, Copy)]
pub struct FilterRule {
    /// Unique rule identifier
    pub id: u32,
    /// FNV-1a hash of the pattern to match
    pub pattern_hash: u64,
    /// Risk level if matched
    pub risk: CommandRisk,
    /// Action to take on match
    pub action: FilterAction,
    /// FNV-1a hash of the human-readable description
    pub description_hash: u64,
    /// Whether this rule is active
    pub enabled: bool,
    /// Number of times this rule has matched
    pub hits: u64,
}

/// Result of filtering a command
#[derive(Debug, Clone, Copy)]
pub struct FilterResult {
    /// Whether the command is allowed to execute
    pub allowed: bool,
    /// Assessed risk level
    pub risk: CommandRisk,
    /// Action to take
    pub action: FilterAction,
    /// FNV-1a hash of the reason string
    pub reason_hash: u64,
    /// ID of the matched rule, if any
    pub matched_rule: Option<u32>,
}

/// Statistics about filter operations
#[derive(Debug, Clone, Copy)]
pub struct FilterStats {
    pub total_checked: u64,
    pub total_allowed: u64,
    pub total_warned: u64,
    pub total_blocked: u64,
    pub total_quarantined: u64,
    pub total_injections_caught: u64,
    pub total_rules: u32,
    pub active_rules: u32,
}

/// Blocked command log entry
#[derive(Debug, Clone, Copy)]
pub struct BlockedEntry {
    pub tick: u64,
    pub command_hash: u64,
    pub rule_id: u32,
    pub risk: CommandRisk,
    pub user_hash: u64,
}

/// The main command filter engine
pub struct CommandFilter {
    /// Filter rules
    rules: Vec<FilterRule>,
    /// Hashes of explicitly blocked command patterns
    blocked_patterns: Vec<u64>,
    /// Hashes of allowed user identifiers
    allowed_users: Vec<u64>,
    /// Hashes of restricted filesystem paths
    restricted_paths: Vec<u64>,
    /// Hashes of injection patterns
    injection_patterns: Vec<(u64, InjectionType)>,
    /// Whitelisted command hashes (bypass filtering)
    whitelist: Vec<u64>,
    /// Blacklisted command hashes (always block)
    blacklist: Vec<u64>,
    /// Blocked command log (ring buffer)
    blocked_log: Vec<BlockedEntry>,
    blocked_log_head: usize,
    /// Counters
    total_checked: u64,
    total_allowed: u64,
    total_warned: u64,
    total_blocked: u64,
    total_quarantined: u64,
    total_injections_caught: u64,
    /// Monotonic tick counter for log entries
    tick: u64,
    /// Whether filtering is enabled globally
    enabled: bool,
    /// Whether strict mode is active (block on any suspicion)
    strict_mode: bool,
}

impl CommandFilter {
    /// Create a new empty command filter
    pub fn new() -> Self {
        CommandFilter {
            rules: Vec::new(),
            blocked_patterns: Vec::new(),
            allowed_users: Vec::new(),
            restricted_paths: Vec::new(),
            injection_patterns: Vec::new(),
            whitelist: Vec::new(),
            blacklist: Vec::new(),
            blocked_log: vec![
                BlockedEntry {
                    tick: 0,
                    command_hash: 0,
                    rule_id: 0,
                    risk: CommandRisk::Safe,
                    user_hash: 0,
                };
                256
            ],
            blocked_log_head: 0,
            total_checked: 0,
            total_allowed: 0,
            total_warned: 0,
            total_blocked: 0,
            total_quarantined: 0,
            total_injections_caught: 0,
            tick: 0,
            enabled: true,
            strict_mode: false,
        }
    }

    /// Load the default set of 50+ filter rules
    fn load_default_rules(&mut self) {
        let mut id: u32 = 1;

        // --- CRITICAL: Destructive filesystem commands ---
        // Rule 1: rm -rf /
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"rm -rf /"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"recursive force delete root"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 2: rm -rf /*
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"rm -rf /*"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"recursive force delete root glob"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 3: rm -rf .
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"rm -rf ."),
            risk: CommandRisk::Critical,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"recursive force delete current dir"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 4: dd to disk device
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"dd if=/dev/zero of=/dev/sda"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"overwrite disk with zeros"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 5: dd to disk device sdb
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"dd if=/dev/zero of=/dev/sdb"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"overwrite second disk with zeros"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 6: dd random to disk
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"dd if=/dev/urandom of=/dev/sda"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"overwrite disk with random data"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 7: chmod 777 /
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"chmod 777 /"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"world writable root filesystem"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 8: chmod -R 777 /
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"chmod -R 777 /"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"recursive world writable root"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 9: fork bomb
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b":(){ :|:& };:"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"fork bomb denial of service"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 10: another fork bomb variant
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b".() { .|.& }; ."),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"fork bomb variant"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 11: /dev/mem access
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/dev/mem"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"direct physical memory access"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 12: /dev/kmem access
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/dev/kmem"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"kernel memory access"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 13: mkfs on sda
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"mkfs /dev/sda"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"format primary disk"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 14: mkfs.ext4 on sda
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"mkfs.ext4 /dev/sda"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"format primary disk ext4"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 15: overwrite MBR
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"dd of=/dev/sda bs=512 count=1"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"overwrite master boot record"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // --- HIGH RISK: Privilege and system manipulation ---
        // Rule 16: sudo
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"sudo"),
            risk: CommandRisk::High,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"privilege escalation via sudo"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 17: su -
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"su -"),
            risk: CommandRisk::High,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"switch to root user"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 18: curl pipe to sh
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"curl | sh"),
            risk: CommandRisk::Critical,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"remote code execution via curl pipe"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 19: curl pipe to bash
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"curl | bash"),
            risk: CommandRisk::Critical,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"remote code execution via curl pipe bash"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 20: wget pipe to sh
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"wget | sh"),
            risk: CommandRisk::Critical,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"remote code execution via wget pipe"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 21: wget to system dirs
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"wget -O /usr/"),
            risk: CommandRisk::High,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"wget download to system directory"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 22: wget to /etc/
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"wget -O /etc/"),
            risk: CommandRisk::High,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"wget download to etc directory"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 23: kill init
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"kill -9 1"),
            risk: CommandRisk::Critical,
            action: FilterAction::Warn,
            description_hash: fnv1a_hash(b"kill init process"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 24: kill all
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"kill -9 -1"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"kill all processes"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 25: modify passwd
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/etc/passwd"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"access password file"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 26: modify shadow
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/etc/shadow"),
            risk: CommandRisk::Critical,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"access shadow password file"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 27: modify sudoers
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/etc/sudoers"),
            risk: CommandRisk::Critical,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"modify sudoers file"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 28: chown root
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"chown root"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"change ownership to root"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 29: insmod (kernel module loading)
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"insmod"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"load kernel module"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 30: modprobe
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"modprobe"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"load kernel module via modprobe"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // --- MEDIUM RISK: Network and data exfiltration ---
        // Rule 31: nc (netcat) listener
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"nc -l"),
            risk: CommandRisk::Medium,
            action: FilterAction::Log,
            description_hash: fnv1a_hash(b"netcat listener"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 32: ncat listener
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"ncat -l"),
            risk: CommandRisk::Medium,
            action: FilterAction::Log,
            description_hash: fnv1a_hash(b"ncat listener"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 33: reverse shell
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/dev/tcp/"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"reverse shell via dev tcp"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 34: iptables flush
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"iptables -F"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"flush all firewall rules"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 35: disable firewall
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"ufw disable"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"disable firewall"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 36: sshd config modification
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/etc/ssh/sshd_config"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"modify ssh daemon config"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // --- MEDIUM RISK: Data exfiltration tools ---
        // Rule 37: base64 encode and pipe
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"base64 | curl"),
            risk: CommandRisk::High,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"data exfiltration via base64 curl"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 38: xxd hex dump and send
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"xxd | nc"),
            risk: CommandRisk::High,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"data exfiltration via hex dump"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 39: tar to remote
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"tar czf - | ssh"),
            risk: CommandRisk::Medium,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"archive and send via ssh"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // --- CRITICAL: System boot and partition ---
        // Rule 40: fdisk
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"fdisk /dev/"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"partition disk"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 41: parted
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"parted /dev/"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"partition disk with parted"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 42: grub modification
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"grub-install"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"modify bootloader"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 43: systemctl disable
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"systemctl disable"),
            risk: CommandRisk::Medium,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"disable system service"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 44: init 0 (shutdown)
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"init 0"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"system halt"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 45: init 6 (reboot)
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"init 6"),
            risk: CommandRisk::Medium,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"system reboot"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 46: echo to /proc/sysrq
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"/proc/sysrq-trigger"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"sysrq trigger access"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 47: crontab overwrite
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"crontab -r"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"remove all cron jobs"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 48: history clear
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"history -c"),
            risk: CommandRisk::Medium,
            action: FilterAction::Log,
            description_hash: fnv1a_hash(b"clear command history"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 49: shred on device
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"shred /dev/"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"secure erase device"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 50: wipefs
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"wipefs /dev/"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"wipe filesystem signatures"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 51: truncate system logs
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"> /var/log/"),
            risk: CommandRisk::High,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"truncate system log"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 52: python reverse shell
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"python -c 'import socket"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"python reverse shell"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 53: perl reverse shell
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"perl -e 'use Socket"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"perl reverse shell"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 54: mount bind root
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"mount --bind / "),
            risk: CommandRisk::Critical,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"bind mount root filesystem"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 55: swapoff all
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"swapoff -a"),
            risk: CommandRisk::High,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"disable all swap"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 56: echo to disk device
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"echo > /dev/sda"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"write to raw disk device"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 57: cat /dev/zero overwrite
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"cat /dev/zero > /dev/"),
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"cat zeros to device"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 58: chattr immutable removal on system files
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"chattr -i /"),
            risk: CommandRisk::Critical,
            action: FilterAction::Confirm,
            description_hash: fnv1a_hash(b"remove immutable flag from root"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 59: sysctl write
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"sysctl -w"),
            risk: CommandRisk::Medium,
            action: FilterAction::Log,
            description_hash: fnv1a_hash(b"modify kernel parameters"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        // Rule 60: xargs rm
        self.rules.push(FilterRule {
            id,
            pattern_hash: fnv1a_hash(b"xargs rm -rf"),
            risk: CommandRisk::Critical,
            action: FilterAction::Block,
            description_hash: fnv1a_hash(b"piped recursive force delete"),
            enabled: true,
            hits: 0,
        });
        id += 1;

        serial_println!("    cmd_filter: loaded {} default rules", id - 1);
    }

    /// Load default injection patterns
    fn load_injection_patterns(&mut self) {
        // Null byte injection
        self.injection_patterns
            .push((fnv1a_hash(b"\x00"), InjectionType::NullByte));

        // Path traversal patterns
        self.injection_patterns
            .push((fnv1a_hash(b"../"), InjectionType::PathTraversal));
        self.injection_patterns
            .push((fnv1a_hash(b"..\\"), InjectionType::PathTraversal));
        self.injection_patterns
            .push((fnv1a_hash(b"%2e%2e%2f"), InjectionType::PathTraversal));
        self.injection_patterns
            .push((fnv1a_hash(b"%2e%2e/"), InjectionType::PathTraversal));
        self.injection_patterns
            .push((fnv1a_hash(b"..%2f"), InjectionType::PathTraversal));
        self.injection_patterns
            .push((fnv1a_hash(b"%2e%2e%5c"), InjectionType::PathTraversal));

        // Backtick execution
        self.injection_patterns
            .push((fnv1a_hash(b"`"), InjectionType::BacktickExec));

        // Command substitution $()
        self.injection_patterns
            .push((fnv1a_hash(b"$("), InjectionType::BacktickExec));

        // Shell escape sequences
        self.injection_patterns
            .push((fnv1a_hash(b"\\x"), InjectionType::ShellEscape));
        self.injection_patterns
            .push((fnv1a_hash(b"\\u"), InjectionType::ShellEscape));
        self.injection_patterns
            .push((fnv1a_hash(b"$'\\"), InjectionType::ShellEscape));

        // Variable expansion
        self.injection_patterns
            .push((fnv1a_hash(b"${"), InjectionType::VariableExpansion));
        self.injection_patterns
            .push((fnv1a_hash(b"$IFS"), InjectionType::VariableExpansion));
        self.injection_patterns
            .push((fnv1a_hash(b"$PATH"), InjectionType::VariableExpansion));

        serial_println!(
            "    cmd_filter: loaded {} injection patterns",
            self.injection_patterns.len()
        );
    }

    /// Load default restricted paths
    fn load_restricted_paths(&mut self) {
        self.restricted_paths.push(fnv1a_hash(b"/dev/sda"));
        self.restricted_paths.push(fnv1a_hash(b"/dev/sdb"));
        self.restricted_paths.push(fnv1a_hash(b"/dev/nvme0"));
        self.restricted_paths.push(fnv1a_hash(b"/dev/mem"));
        self.restricted_paths.push(fnv1a_hash(b"/dev/kmem"));
        self.restricted_paths.push(fnv1a_hash(b"/dev/port"));
        self.restricted_paths.push(fnv1a_hash(b"/boot/"));
        self.restricted_paths.push(fnv1a_hash(b"/etc/shadow"));
        self.restricted_paths.push(fnv1a_hash(b"/etc/sudoers"));
        self.restricted_paths
            .push(fnv1a_hash(b"/proc/sysrq-trigger"));
        self.restricted_paths.push(fnv1a_hash(b"/proc/kcore"));
        self.restricted_paths.push(fnv1a_hash(b"/sys/firmware/"));
        serial_println!(
            "    cmd_filter: loaded {} restricted paths",
            self.restricted_paths.len()
        );
    }

    /// Check a command against all filter rules and injection patterns
    pub fn check_command(&mut self, command: &[u8], user_hash: u64) -> FilterResult {
        self.total_checked = self.total_checked.saturating_add(1);
        self.tick = self.tick.saturating_add(1);

        // Check if filtering is enabled
        if !self.enabled {
            self.total_allowed = self.total_allowed.saturating_add(1);
            return FilterResult {
                allowed: true,
                risk: CommandRisk::Safe,
                action: FilterAction::Allow,
                reason_hash: fnv1a_hash(b"filtering disabled"),
                matched_rule: None,
            };
        }

        // Reject excessively long commands
        if command.len() > MAX_COMMAND_LENGTH {
            self.total_blocked = self.total_blocked.saturating_add(1);
            self.log_blocked_entry(fnv1a_hash(command), 0, CommandRisk::High, user_hash);
            return FilterResult {
                allowed: false,
                risk: CommandRisk::High,
                action: FilterAction::Block,
                reason_hash: fnv1a_hash(b"command exceeds maximum length"),
                matched_rule: None,
            };
        }

        // Check whitelist first (bypass all other checks)
        let cmd_hash = fnv1a_hash(command);
        for &wl_hash in &self.whitelist {
            if wl_hash == cmd_hash {
                self.total_allowed = self.total_allowed.saturating_add(1);
                return FilterResult {
                    allowed: true,
                    risk: CommandRisk::Safe,
                    action: FilterAction::Allow,
                    reason_hash: fnv1a_hash(b"whitelisted command"),
                    matched_rule: None,
                };
            }
        }

        // Check blacklist (always block)
        for &bl_hash in &self.blacklist {
            if bl_hash == cmd_hash {
                self.total_blocked = self.total_blocked.saturating_add(1);
                self.log_blocked_entry(cmd_hash, 0, CommandRisk::Blocked, user_hash);
                return FilterResult {
                    allowed: false,
                    risk: CommandRisk::Blocked,
                    action: FilterAction::Block,
                    reason_hash: fnv1a_hash(b"blacklisted command"),
                    matched_rule: None,
                };
            }
        }

        // Check user authorization
        if !self.is_user_allowed(user_hash) {
            self.total_blocked = self.total_blocked.saturating_add(1);
            self.log_blocked_entry(cmd_hash, 0, CommandRisk::High, user_hash);
            return FilterResult {
                allowed: false,
                risk: CommandRisk::High,
                action: FilterAction::Block,
                reason_hash: fnv1a_hash(b"user not authorized"),
                matched_rule: None,
            };
        }

        // Check for injection attacks
        if let Some(injection_result) = self.check_injection(command) {
            self.total_injections_caught = self.total_injections_caught.saturating_add(1);
            if self.strict_mode {
                self.total_blocked = self.total_blocked.saturating_add(1);
                self.log_blocked_entry(cmd_hash, 0, CommandRisk::Critical, user_hash);
                return FilterResult {
                    allowed: false,
                    risk: CommandRisk::Critical,
                    action: FilterAction::Block,
                    reason_hash: injection_result,
                    matched_rule: None,
                };
            }
            // Non-strict: warn but allow for some injection types
        }

        // Check nesting depth (detect deeply nested subshells)
        let nesting = self.count_nesting_depth(command);
        if nesting > MAX_NESTING_DEPTH {
            self.total_blocked = self.total_blocked.saturating_add(1);
            self.log_blocked_entry(cmd_hash, 0, CommandRisk::High, user_hash);
            return FilterResult {
                allowed: false,
                risk: CommandRisk::High,
                action: FilterAction::Block,
                reason_hash: fnv1a_hash(b"excessive nesting depth"),
                matched_rule: None,
            };
        }

        // Check against all filter rules (substring matching via hash windows)
        let _cmd_lower_hash = fnv1a_hash_lower(command);
        let mut worst_risk = CommandRisk::Safe;
        let mut worst_action = FilterAction::Allow;
        let mut worst_reason = fnv1a_hash(b"no rule matched");
        let mut worst_rule_id: Option<u32> = None;

        for rule_idx in 0..self.rules.len() {
            if !self.rules[rule_idx].enabled {
                continue;
            }

            // Check if the command contains the pattern as a substring
            // We compare the pattern hash against sliding windows of the command
            let pattern_hash = self.rules[rule_idx].pattern_hash;
            if self.contains_pattern_hash(command, pattern_hash) {
                self.rules[rule_idx].hits = self.rules[rule_idx].hits.saturating_add(1);
                let rule_risk = self.rules[rule_idx].risk;
                let rule_action = self.rules[rule_idx].action;
                let rule_id = self.rules[rule_idx].id;

                if Self::risk_level(rule_risk) > Self::risk_level(worst_risk) {
                    worst_risk = rule_risk;
                    worst_action = rule_action;
                    worst_reason = self.rules[rule_idx].description_hash;
                    worst_rule_id = Some(rule_id);
                }
            }
        }

        // Check restricted paths
        if self.check_restricted_paths(command) {
            if Self::risk_level(CommandRisk::High) > Self::risk_level(worst_risk) {
                worst_risk = CommandRisk::High;
                worst_action = FilterAction::Block;
                worst_reason = fnv1a_hash(b"restricted path access");
                worst_rule_id = None;
            }
        }

        // Also check blocked_patterns
        for &bp_hash in &self.blocked_patterns {
            if self.contains_pattern_hash(command, bp_hash) {
                if Self::risk_level(CommandRisk::Blocked) > Self::risk_level(worst_risk) {
                    worst_risk = CommandRisk::Blocked;
                    worst_action = FilterAction::Block;
                    worst_reason = fnv1a_hash(b"matched blocked pattern");
                    worst_rule_id = None;
                }
            }
        }

        // Determine outcome
        let allowed = match worst_action {
            FilterAction::Allow => true,
            FilterAction::Warn => true,
            FilterAction::Log => true,
            FilterAction::Confirm => false, // requires confirmation, treat as blocked until confirmed
            FilterAction::Block => false,
            FilterAction::Quarantine => false,
        };

        // Update counters
        match worst_action {
            FilterAction::Allow | FilterAction::Log => {
                self.total_allowed = self.total_allowed.saturating_add(1);
            }
            FilterAction::Warn => {
                self.total_warned = self.total_warned.saturating_add(1);
                self.total_allowed = self.total_allowed.saturating_add(1);
            }
            FilterAction::Block | FilterAction::Confirm => {
                self.total_blocked = self.total_blocked.saturating_add(1);
                self.log_blocked_entry(cmd_hash, worst_rule_id.unwrap_or(0), worst_risk, user_hash);
            }
            FilterAction::Quarantine => {
                self.total_quarantined = self.total_quarantined.saturating_add(1);
                self.log_blocked_entry(cmd_hash, worst_rule_id.unwrap_or(0), worst_risk, user_hash);
            }
        }

        FilterResult {
            allowed,
            risk: worst_risk,
            action: worst_action,
            reason_hash: worst_reason,
            matched_rule: worst_rule_id,
        }
    }

    /// Check a command for injection attacks. Returns reason hash if detected.
    pub fn check_injection(&self, command: &[u8]) -> Option<u64> {
        // Check for null bytes
        for &byte in command {
            if byte == 0 {
                return Some(fnv1a_hash(b"null byte injection detected"));
            }
        }

        // Check for path traversal (direct byte scan, not hash-based)
        if self.scan_path_traversal(command) {
            return Some(fnv1a_hash(b"path traversal detected"));
        }

        // Check for backtick execution
        for &byte in command {
            if byte == b'`' {
                return Some(fnv1a_hash(b"backtick command execution detected"));
            }
        }

        // Check for $() command substitution
        for i in 0..command.len().saturating_sub(1) {
            if command[i] == b'$' && command[i + 1] == b'(' {
                return Some(fnv1a_hash(b"command substitution detected"));
            }
        }

        // Check for ${} variable expansion
        for i in 0..command.len().saturating_sub(1) {
            if command[i] == b'$' && command[i + 1] == b'{' {
                return Some(fnv1a_hash(b"variable expansion detected"));
            }
        }

        // Check for pipe injection (multiple pipes suggest chaining)
        let mut pipe_count: u32 = 0;
        for &byte in command {
            if byte == b'|' {
                pipe_count += 1;
            }
        }
        if pipe_count > 3 {
            return Some(fnv1a_hash(b"excessive pipe chaining detected"));
        }

        // Check for command chaining via ; outside of quotes
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut semicolons: u32 = 0;
        for &byte in command {
            match byte {
                b'\'' if !in_double_quote => in_single_quote = !in_single_quote,
                b'"' if !in_single_quote => in_double_quote = !in_double_quote,
                b';' if !in_single_quote && !in_double_quote => semicolons += 1,
                _ => {}
            }
        }
        if semicolons > 2 {
            return Some(fnv1a_hash(b"excessive command chaining detected"));
        }

        // Check for glob expansion in dangerous positions
        if self.strict_mode {
            for &byte in command {
                if byte == b'*' || byte == b'?' {
                    return Some(fnv1a_hash(b"glob expansion in strict mode"));
                }
            }
        }

        None
    }

    /// Sanitize input by stripping dangerous characters
    pub fn sanitize_input(&self, input: &[u8], output: &mut [u8]) -> usize {
        let mut out_idx = 0;
        let max_out = output.len();

        for &byte in input {
            if out_idx >= max_out {
                break;
            }
            match byte {
                // Strip null bytes
                0 => {}
                // Strip backticks
                b'`' => {}
                // Strip control characters except tab, newline, carriage return
                0x01..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => {}
                // Allow everything else
                _ => {
                    output[out_idx] = byte;
                    out_idx += 1;
                }
            }
        }
        out_idx
    }

    /// Check if a path is allowed for access
    pub fn is_path_allowed(&self, path: &[u8]) -> bool {
        let path_hash = fnv1a_hash(path);
        for &restricted in &self.restricted_paths {
            if restricted == path_hash {
                return false;
            }
        }
        // Also check for path traversal in the path itself
        if self.scan_path_traversal(path) {
            return false;
        }
        true
    }

    /// Check if a user is allowed to execute commands
    pub fn is_user_allowed(&self, user_hash: u64) -> bool {
        // If no allowed users configured, allow all
        if self.allowed_users.is_empty() {
            return true;
        }
        for &allowed in &self.allowed_users {
            if allowed == user_hash {
                return true;
            }
        }
        false
    }

    /// Add a new filter rule
    pub fn add_rule(&mut self, rule: FilterRule) -> bool {
        if self.rules.len() >= MAX_RULES {
            return false;
        }
        // Check for duplicate ID
        for existing in &self.rules {
            if existing.id == rule.id {
                return false;
            }
        }
        self.rules.push(rule);
        true
    }

    /// Remove a filter rule by ID
    pub fn remove_rule(&mut self, rule_id: u32) -> bool {
        let initial_len = self.rules.len();
        self.rules.retain(|r| r.id != rule_id);
        self.rules.len() < initial_len
    }

    /// Get the numeric risk level for comparison
    fn risk_level(risk: CommandRisk) -> u32 {
        match risk {
            CommandRisk::Safe => 0,
            CommandRisk::Low => 1,
            CommandRisk::Medium => 2,
            CommandRisk::High => 3,
            CommandRisk::Critical => 4,
            CommandRisk::Blocked => 5,
        }
    }

    /// Get the risk level of a command (without executing the full check pipeline)
    pub fn get_risk_level(&self, command: &[u8]) -> CommandRisk {
        let mut worst = CommandRisk::Safe;
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            if self.contains_pattern_hash(command, rule.pattern_hash) {
                if Self::risk_level(rule.risk) > Self::risk_level(worst) {
                    worst = rule.risk;
                }
            }
        }
        worst
    }

    /// Log a blocked command to the ring buffer
    pub fn log_blocked(
        &mut self,
        command_hash: u64,
        rule_id: u32,
        risk: CommandRisk,
        user_hash: u64,
    ) {
        self.log_blocked_entry(command_hash, rule_id, risk, user_hash);
    }

    /// Internal: write a blocked entry to the ring buffer
    fn log_blocked_entry(
        &mut self,
        command_hash: u64,
        rule_id: u32,
        risk: CommandRisk,
        user_hash: u64,
    ) {
        let entry = BlockedEntry {
            tick: self.tick,
            command_hash,
            rule_id,
            risk,
            user_hash,
        };
        if !self.blocked_log.is_empty() {
            let idx = self.blocked_log_head % self.blocked_log.len();
            self.blocked_log[idx] = entry;
            self.blocked_log_head += 1;
        }
    }

    /// Get filter statistics
    pub fn get_stats(&self) -> FilterStats {
        let active_rules = self.rules.iter().filter(|r| r.enabled).count() as u32;
        FilterStats {
            total_checked: self.total_checked,
            total_allowed: self.total_allowed,
            total_warned: self.total_warned,
            total_blocked: self.total_blocked,
            total_quarantined: self.total_quarantined,
            total_injections_caught: self.total_injections_caught,
            total_rules: self.rules.len() as u32,
            active_rules,
        }
    }

    /// Add a command to the whitelist (bypass all filtering)
    pub fn whitelist_command(&mut self, command_hash: u64) -> bool {
        if self.whitelist.len() >= MAX_WHITELIST {
            return false;
        }
        // Prevent duplicates
        for &existing in &self.whitelist {
            if existing == command_hash {
                return true;
            }
        }
        self.whitelist.push(command_hash);
        true
    }

    /// Add a command to the blacklist (always block)
    pub fn blacklist_command(&mut self, command_hash: u64) -> bool {
        if self.blacklist.len() >= MAX_BLACKLIST {
            return false;
        }
        for &existing in &self.blacklist {
            if existing == command_hash {
                return true;
            }
        }
        self.blacklist.push(command_hash);
        true
    }

    /// Add an allowed user
    pub fn add_allowed_user(&mut self, user_hash: u64) -> bool {
        if self.allowed_users.len() >= MAX_ALLOWED_USERS {
            return false;
        }
        self.allowed_users.push(user_hash);
        true
    }

    /// Add a restricted path
    pub fn add_restricted_path(&mut self, path_hash: u64) -> bool {
        if self.restricted_paths.len() >= MAX_RESTRICTED_PATHS {
            return false;
        }
        self.restricted_paths.push(path_hash);
        true
    }

    /// Add a blocked pattern
    pub fn add_blocked_pattern(&mut self, pattern_hash: u64) -> bool {
        if self.blocked_patterns.len() >= MAX_BLOCKED_PATTERNS {
            return false;
        }
        self.blocked_patterns.push(pattern_hash);
        true
    }

    /// Enable or disable strict mode
    pub fn set_strict_mode(&mut self, strict: bool) {
        self.strict_mode = strict;
    }

    /// Enable or disable the filter globally
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Enable or disable a rule by ID
    pub fn set_rule_enabled(&mut self, rule_id: u32, enabled: bool) -> bool {
        for rule in &mut self.rules {
            if rule.id == rule_id {
                rule.enabled = enabled;
                return true;
            }
        }
        false
    }

    /// Get a rule by ID
    pub fn get_rule(&self, rule_id: u32) -> Option<&FilterRule> {
        self.rules.iter().find(|r| r.id == rule_id)
    }

    /// Get the number of hits for a rule
    pub fn get_rule_hits(&self, rule_id: u32) -> Option<u64> {
        self.rules.iter().find(|r| r.id == rule_id).map(|r| r.hits)
    }

    /// Reset all hit counters
    pub fn reset_stats(&mut self) {
        self.total_checked = 0;
        self.total_allowed = 0;
        self.total_warned = 0;
        self.total_blocked = 0;
        self.total_quarantined = 0;
        self.total_injections_caught = 0;
        for rule in &mut self.rules {
            rule.hits = 0;
        }
    }

    // --- Internal helper methods ---

    /// Check if command bytes contain the pattern by comparing substring hashes.
    /// This uses a rolling window check: for each possible substring length, hash
    /// the substring and compare to the pattern hash. Because we don't know the
    /// pattern length, we do a multi-pass approach for common lengths.
    /// For efficiency we also do a direct full-command hash comparison.
    fn contains_pattern_hash(&self, command: &[u8], pattern_hash: u64) -> bool {
        // Quick check: exact match of the entire command
        if fnv1a_hash(command) == pattern_hash {
            return true;
        }

        // Sliding window for common pattern lengths (3 to 40 bytes)
        for window_size in 3..=40usize {
            if window_size > command.len() {
                break;
            }
            for start in 0..=(command.len() - window_size) {
                let window = &command[start..start + window_size];
                if fnv1a_hash(window) == pattern_hash {
                    return true;
                }
            }
        }
        false
    }

    /// Scan for path traversal patterns in raw bytes
    fn scan_path_traversal(&self, data: &[u8]) -> bool {
        for i in 0..data.len().saturating_sub(2) {
            if data[i] == b'.' && data[i + 1] == b'.' {
                if i + 2 < data.len() && (data[i + 2] == b'/' || data[i + 2] == b'\\') {
                    return true;
                }
            }
        }
        // Check for URL-encoded path traversal: %2e%2e
        for i in 0..data.len().saturating_sub(5) {
            if data[i] == b'%' && data[i + 1] == b'2' {
                let c = data[i + 2];
                if c == b'e' || c == b'E' {
                    // Could be %2e - check for another %2e or ..
                    if i + 5 < data.len() && data[i + 3] == b'%' && data[i + 4] == b'2' {
                        let c2 = data[i + 5];
                        if c2 == b'e' || c2 == b'E' {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Count nesting depth of subshells and command groups
    fn count_nesting_depth(&self, command: &[u8]) -> usize {
        let mut max_depth: usize = 0;
        let mut current_depth: usize = 0;
        for &byte in command {
            match byte {
                b'(' | b'{' => {
                    current_depth += 1;
                    if current_depth > max_depth {
                        max_depth = current_depth;
                    }
                }
                b')' | b'}' => {
                    if current_depth > 0 {
                        current_depth -= 1;
                    }
                }
                _ => {}
            }
        }
        max_depth
    }

    /// Check if the command references any restricted paths
    fn check_restricted_paths(&self, command: &[u8]) -> bool {
        for &path_hash in &self.restricted_paths {
            if self.contains_pattern_hash(command, path_hash) {
                return true;
            }
        }
        false
    }
}

// --- Public API (accessed through the global Mutex) ---

/// Initialize the command filter subsystem with default rules
pub fn init() {
    let mut filter = CommandFilter::new();
    filter.load_default_rules();
    filter.load_injection_patterns();
    filter.load_restricted_paths();

    let mut global = CMD_FILTER.lock();
    *global = Some(filter);

    serial_println!("    cmd_filter: initialized with default security rules");
}

/// Check a command for safety. Returns the filter result.
pub fn check(command: &[u8], user_hash: u64) -> FilterResult {
    let mut guard = CMD_FILTER.lock();
    match guard.as_mut() {
        Some(filter) => filter.check_command(command, user_hash),
        None => FilterResult {
            allowed: false,
            risk: CommandRisk::Blocked,
            action: FilterAction::Block,
            reason_hash: fnv1a_hash(b"filter not initialized"),
            matched_rule: None,
        },
    }
}

/// Check a command for injection attacks only
pub fn check_injection(command: &[u8]) -> Option<u64> {
    let guard = CMD_FILTER.lock();
    match guard.as_ref() {
        Some(filter) => filter.check_injection(command),
        None => Some(fnv1a_hash(b"filter not initialized")),
    }
}

/// Sanitize command input, writing result to output buffer. Returns bytes written.
pub fn sanitize(input: &[u8], output: &mut [u8]) -> usize {
    let guard = CMD_FILTER.lock();
    match guard.as_ref() {
        Some(filter) => filter.sanitize_input(input, output),
        None => 0,
    }
}

/// Add a command hash to the whitelist
pub fn whitelist(command_hash: u64) -> bool {
    let mut guard = CMD_FILTER.lock();
    match guard.as_mut() {
        Some(filter) => filter.whitelist_command(command_hash),
        None => false,
    }
}

/// Add a command hash to the blacklist
pub fn blacklist(command_hash: u64) -> bool {
    let mut guard = CMD_FILTER.lock();
    match guard.as_mut() {
        Some(filter) => filter.blacklist_command(command_hash),
        None => false,
    }
}

/// Get filter statistics
pub fn stats() -> Option<FilterStats> {
    let guard = CMD_FILTER.lock();
    guard.as_ref().map(|f| f.get_stats())
}

/// Get risk level for a command without full check
pub fn risk_level(command: &[u8]) -> CommandRisk {
    let guard = CMD_FILTER.lock();
    match guard.as_ref() {
        Some(filter) => filter.get_risk_level(command),
        None => CommandRisk::Blocked,
    }
}

/// Check if a path is allowed
pub fn path_allowed(path: &[u8]) -> bool {
    let guard = CMD_FILTER.lock();
    match guard.as_ref() {
        Some(filter) => filter.is_path_allowed(path),
        None => false,
    }
}

/// Enable or disable strict mode
pub fn set_strict(strict: bool) {
    let mut guard = CMD_FILTER.lock();
    if let Some(filter) = guard.as_mut() {
        filter.set_strict_mode(strict);
    }
}

/// Add a custom filter rule
pub fn add_rule(rule: FilterRule) -> bool {
    let mut guard = CMD_FILTER.lock();
    match guard.as_mut() {
        Some(filter) => filter.add_rule(rule),
        None => false,
    }
}

/// Remove a filter rule by ID
pub fn remove_rule(rule_id: u32) -> bool {
    let mut guard = CMD_FILTER.lock();
    match guard.as_mut() {
        Some(filter) => filter.remove_rule(rule_id),
        None => false,
    }
}

/// Compute a hash for use with the filter APIs
pub fn hash(data: &[u8]) -> u64 {
    fnv1a_hash(data)
}
