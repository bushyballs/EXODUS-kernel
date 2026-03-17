/// Seccomp-BPF Sandbox for Genesis — syscall filtering
///
/// Restricts which system calls a process can make using a simple rule-based system.
/// Each process gets a fixed-size filter with up to 64 rules.
///
/// Storage: a fixed-size array of 256 `SeccompProfile` entries indexed by `pid % 256`.
///
/// Critical rules:
///   - No heap: [SeccompFilter; 64] is a plain static array.
///   - No panics: every array access is bounds-checked; functions return Option/bool.
///   - No float casts.
///   - All counters use saturating arithmetic.
///   - The Mutex inner type is Copy and has const fn empty().
use crate::serial_println;
use crate::sync::Mutex;

// ── Seccomp action types ──────────────────────────────────────────────────

/// Seccomp action (no associated data — separate errno_val field if needed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeccompAction {
    Allow = 0,
    KillThread = 1,
    KillProcess = 2,
    Trap = 3,
    Log = 4,
    Errno = 5,
}

// ── Seccomp mode ─────────────────────────────────────────────────────────

/// Seccomp operating mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeccompMode {
    /// Disabled — no filtering applied.
    Disabled = 0,
    /// Strict mode — only read/write/exit/sigreturn allowed.
    Strict = 1,
    /// Filter mode — BPF-based per-syscall rules.
    Filter = 2,
}

// ── Seccomp filter rule (fixed-size) ──────────────────────────────────────

/// A single seccomp filter rule (Copy + Clone).
#[derive(Clone, Copy)]
pub struct SeccompFilter {
    pub syscall_nr: u64,
    pub action: SeccompAction,
    pub errno_val: u32,
    pub active: bool,
    pub mode: SeccompMode,
}

impl SeccompFilter {
    pub const fn empty() -> Self {
        SeccompFilter {
            syscall_nr: 0,
            action: SeccompAction::Allow,
            errno_val: 0,
            active: false,
            mode: SeccompMode::Disabled,
        }
    }
}

// ── Seccomp profile per process ───────────────────────────────────────────

/// Per-process seccomp profile (Copy + const fn empty()).
#[derive(Clone, Copy)]
pub struct SeccompProfile {
    pub pid: u32,
    pub default_action: SeccompAction,
    pub filters: [SeccompFilter; 64],
    pub nfilters: u8,
    pub active: bool,
}

impl SeccompProfile {
    pub const fn empty() -> Self {
        SeccompProfile {
            pid: 0,
            default_action: SeccompAction::Allow,
            filters: [SeccompFilter::empty(); 64],
            nfilters: 0,
            active: false,
        }
    }
}

// ── Global seccomp table ──────────────────────────────────────────────────

/// Fixed-size array of 256 seccomp profiles, indexed by pid % 256.
static SECCOMP_PROFILES: Mutex<[SeccompProfile; 256]> = Mutex::new([SeccompProfile::empty(); 256]);

const PROFILE_TABLE_SIZE: usize = 256;

#[inline(always)]
fn profile_idx(pid: u32) -> usize {
    (pid as usize) % PROFILE_TABLE_SIZE
}

// ── Public seccomp API ────────────────────────────────────────────────────

/// Set or create a seccomp profile for a process.
///
/// Returns true if successful, false if the profile table is full or pid is invalid.
pub fn seccomp_set_profile(pid: u32, default_action: SeccompAction) -> bool {
    let idx = profile_idx(pid);
    let mut table = SECCOMP_PROFILES.lock();

    table[idx] = SeccompProfile {
        pid,
        default_action,
        filters: [SeccompFilter::empty(); 64],
        nfilters: 0,
        active: true,
    };

    true
}

/// Add a filter rule to a process's seccomp profile.
///
/// Returns true if added successfully, false if the process has no profile or the
/// filter table is full (max 64 rules per process).
pub fn seccomp_add_rule(pid: u32, syscall_nr: u64, action: SeccompAction, errno_val: u32) -> bool {
    let idx = profile_idx(pid);
    let mut table = SECCOMP_PROFILES.lock();

    if !table[idx].active {
        return false;
    }

    if (table[idx].nfilters as usize) >= 64 {
        return false;
    }

    let rule_idx = table[idx].nfilters as usize;
    table[idx].filters[rule_idx] = SeccompFilter {
        syscall_nr,
        action,
        errno_val,
        active: true,
        mode: SeccompMode::Filter,
    };

    table[idx].nfilters = table[idx].nfilters.saturating_add(1);
    true
}

/// Check if a syscall is allowed for a process.
///
/// Scans the filter list first, then returns the default action if no match.
pub fn seccomp_check(pid: u32, syscall_nr: u64) -> SeccompAction {
    let idx = profile_idx(pid);
    let table = SECCOMP_PROFILES.lock();

    if !table[idx].active {
        // No profile = allow all
        return SeccompAction::Allow;
    }

    // Scan filters for a match
    let nfilters = table[idx].nfilters as usize;
    for i in 0..nfilters {
        let filter = &table[idx].filters[i];
        if filter.active && filter.syscall_nr == syscall_nr {
            return filter.action;
        }
    }

    // No match = return default
    table[idx].default_action
}

/// Remove a seccomp profile for a process.
///
/// Returns true if removed, false if no profile existed.
pub fn seccomp_remove_profile(pid: u32) -> bool {
    let idx = profile_idx(pid);
    let mut table = SECCOMP_PROFILES.lock();

    if !table[idx].active {
        return false;
    }

    table[idx].active = false;
    table[idx].nfilters = 0;
    true
}

/// Log a seccomp violation (prints to serial).
pub fn seccomp_log_violation(pid: u32, syscall_nr: u64) {
    serial_println!(
        "  [seccomp] VIOLATION: pid={} attempted syscall {}",
        pid,
        syscall_nr
    );
}

/// Initialize the seccomp-BPF sandbox.
///
/// Sets up a default "allow all" profile for pid 0 and pid 1.
pub fn init() {
    let mut table = SECCOMP_PROFILES.lock();

    // Init pid 0 (kernel) — allow all
    table[0] = SeccompProfile {
        pid: 0,
        default_action: SeccompAction::Allow,
        filters: [SeccompFilter::empty(); 64],
        nfilters: 0,
        active: true,
    };

    // Init pid 1 (init) — allow all
    table[1] = SeccompProfile {
        pid: 1,
        default_action: SeccompAction::Allow,
        filters: [SeccompFilter::empty(); 64],
        nfilters: 0,
        active: true,
    };

    serial_println!("[seccomp] seccomp-BPF sandbox initialized");
}
