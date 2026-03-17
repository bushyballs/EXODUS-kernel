/// Resource limits (rlimit) for Genesis
///
/// Implements per-process resource limits similar to POSIX getrlimit/setrlimit.
/// Enforces soft and hard limits on CPU time, memory, file sizes, open file
/// descriptors, stack size, and other resources. The kernel checks these
/// limits before allocating resources to processes.
///
/// Inspired by: Linux rlimit, FreeBSD resource limits, POSIX.1-2008.
/// All code is original.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ── Resource types ───────────────────────────────────────────────────

/// Resource type identifiers (RLIMIT_*)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Resource {
    /// CPU time in seconds
    CpuTime = 0,
    /// Maximum file size in bytes
    FileSize = 1,
    /// Maximum data segment size (heap) in bytes
    Data = 2,
    /// Maximum stack size in bytes
    Stack = 3,
    /// Maximum core dump size in bytes
    Core = 4,
    /// Maximum resident set size in bytes
    Rss = 5,
    /// Maximum number of open file descriptors
    NoFile = 6,
    /// Maximum address space size in bytes
    AddressSpace = 7,
    /// Maximum number of processes per user
    NProc = 8,
    /// Maximum size of locked memory in bytes
    MemLock = 9,
    /// Maximum number of pending signals
    SigPending = 10,
    /// Maximum size of message queue in bytes
    MsgQueue = 11,
}

/// Sentinel for "unlimited" resource
pub const RLIM_INFINITY: u64 = u64::MAX;

/// Total number of resource types
const NUM_RESOURCES: usize = 12;

// ── Limit values ─────────────────────────────────────────────────────

/// A soft/hard limit pair
#[derive(Debug, Clone, Copy)]
pub struct RLimit {
    /// Current (soft) limit — enforced, can be raised up to hard
    pub soft: u64,
    /// Maximum (hard) limit — only root can raise this
    pub hard: u64,
}

impl RLimit {
    /// Create an unlimited limit
    pub const fn unlimited() -> Self {
        RLimit {
            soft: RLIM_INFINITY,
            hard: RLIM_INFINITY,
        }
    }

    /// Create a limit with the same soft and hard value
    pub const fn fixed(value: u64) -> Self {
        RLimit {
            soft: value,
            hard: value,
        }
    }

    /// Create a limit with different soft and hard values
    pub const fn new(soft: u64, hard: u64) -> Self {
        RLimit { soft, hard }
    }

    /// Check if a value exceeds the soft limit
    pub fn exceeds_soft(&self, value: u64) -> bool {
        self.soft != RLIM_INFINITY && value > self.soft
    }

    /// Check if a value exceeds the hard limit
    pub fn exceeds_hard(&self, value: u64) -> bool {
        self.hard != RLIM_INFINITY && value > self.hard
    }

    /// Check if this limit is unlimited
    pub fn is_unlimited(&self) -> bool {
        self.soft == RLIM_INFINITY && self.hard == RLIM_INFINITY
    }

    /// Remaining capacity under soft limit (returns 0 if unlimited)
    pub fn remaining(&self, current: u64) -> u64 {
        if self.soft == RLIM_INFINITY {
            return RLIM_INFINITY;
        }
        if current >= self.soft {
            return 0;
        }
        self.soft - current
    }
}

// ── Per-process resource limits ──────────────────────────────────────

/// Resource limit set for a single process
#[derive(Debug, Clone)]
pub struct ProcessLimits {
    /// Limits array indexed by Resource
    pub limits: [RLimit; NUM_RESOURCES],
    /// Current resource usage (for enforcement)
    pub usage: [u64; NUM_RESOURCES],
}

impl ProcessLimits {
    /// Create default resource limits (sensible defaults for a new process)
    pub const fn new() -> Self {
        let mut limits = [RLimit::unlimited(); NUM_RESOURCES];

        // CPU time: soft 3600s (1hr), hard unlimited
        limits[Resource::CpuTime as usize] = RLimit::new(3600, RLIM_INFINITY);
        // File size: 1 GB soft, 4 GB hard
        limits[Resource::FileSize as usize] =
            RLimit::new(1024 * 1024 * 1024, 4 * 1024 * 1024 * 1024);
        // Data (heap): 256 MB soft, 1 GB hard
        limits[Resource::Data as usize] = RLimit::new(256 * 1024 * 1024, 1024 * 1024 * 1024);
        // Stack: 8 MB soft, 64 MB hard
        limits[Resource::Stack as usize] = RLimit::new(8 * 1024 * 1024, 64 * 1024 * 1024);
        // Core dump: 0 soft (disabled by default), 64 MB hard
        limits[Resource::Core as usize] = RLimit::new(0, 64 * 1024 * 1024);
        // RSS: 512 MB soft, 2 GB hard
        limits[Resource::Rss as usize] = RLimit::new(512 * 1024 * 1024, 2 * 1024 * 1024 * 1024);
        // Open files: 1024 soft, 4096 hard
        limits[Resource::NoFile as usize] = RLimit::new(1024, 4096);
        // Address space: 4 GB soft, unlimited hard
        limits[Resource::AddressSpace as usize] =
            RLimit::new(4 * 1024 * 1024 * 1024, RLIM_INFINITY);
        // NProc: 256 soft, 1024 hard
        limits[Resource::NProc as usize] = RLimit::new(256, 1024);
        // MemLock: 64 KB soft, 1 MB hard
        limits[Resource::MemLock as usize] = RLimit::new(64 * 1024, 1024 * 1024);
        // SigPending: 128 soft, 1024 hard
        limits[Resource::SigPending as usize] = RLimit::new(128, 1024);
        // MsgQueue: 8 MB soft, 64 MB hard
        limits[Resource::MsgQueue as usize] = RLimit::new(8 * 1024 * 1024, 64 * 1024 * 1024);

        ProcessLimits {
            limits,
            usage: [0u64; NUM_RESOURCES],
        }
    }

    /// Get the current limit for a resource
    pub fn get(&self, resource: Resource) -> RLimit {
        self.limits[resource as usize]
    }

    /// Set the limit for a resource (enforces hard limit ceiling)
    pub fn set(
        &mut self,
        resource: Resource,
        new_limit: RLimit,
        is_privileged: bool,
    ) -> Result<(), &'static str> {
        let idx = resource as usize;
        let current = &self.limits[idx];

        // Soft limit cannot exceed hard limit
        if new_limit.soft != RLIM_INFINITY
            && new_limit.hard != RLIM_INFINITY
            && new_limit.soft > new_limit.hard
        {
            return Err("soft limit exceeds hard limit");
        }

        // Only privileged processes can raise the hard limit
        if !is_privileged {
            if new_limit.hard != RLIM_INFINITY
                && current.hard != RLIM_INFINITY
                && new_limit.hard > current.hard
            {
                return Err("cannot raise hard limit without privilege");
            }
        }

        self.limits[idx] = new_limit;
        Ok(())
    }

    /// Check if allocating `amount` more of `resource` would exceed the soft limit
    pub fn check_soft(&self, resource: Resource, amount: u64) -> bool {
        let idx = resource as usize;
        let limit = &self.limits[idx];
        let new_usage = self.usage[idx].saturating_add(amount);
        !limit.exceeds_soft(new_usage)
    }

    /// Check if allocating `amount` more of `resource` would exceed the hard limit
    pub fn check_hard(&self, resource: Resource, amount: u64) -> bool {
        let idx = resource as usize;
        let limit = &self.limits[idx];
        let new_usage = self.usage[idx].saturating_add(amount);
        !limit.exceeds_hard(new_usage)
    }

    /// Try to consume a resource (returns false if hard limit exceeded)
    pub fn consume(&mut self, resource: Resource, amount: u64) -> Result<(), &'static str> {
        let idx = resource as usize;
        let new_usage = self.usage[idx].saturating_add(amount);

        if self.limits[idx].exceeds_hard(new_usage) {
            return Err("hard resource limit exceeded");
        }

        self.usage[idx] = new_usage;
        Ok(())
    }

    /// Release previously consumed resources
    pub fn release(&mut self, resource: Resource, amount: u64) {
        let idx = resource as usize;
        self.usage[idx] = self.usage[idx].saturating_sub(amount);
    }

    /// Record current usage for a resource
    pub fn set_usage(&mut self, resource: Resource, value: u64) {
        self.usage[resource as usize] = value;
    }

    /// Get current usage for a resource
    pub fn get_usage(&self, resource: Resource) -> u64 {
        self.usage[resource as usize]
    }

    /// Get remaining capacity under soft limit
    pub fn remaining_soft(&self, resource: Resource) -> u64 {
        let idx = resource as usize;
        self.limits[idx].remaining(self.usage[idx])
    }

    /// Inherit limits from parent (fork behavior)
    pub fn inherit_from(&mut self, parent: &ProcessLimits) {
        self.limits = parent.limits;
        // Usage starts fresh for the child
        self.usage = [0u64; NUM_RESOURCES];
    }
}

// ── Global default limits ────────────────────────────────────────────

/// System-wide default limits applied to new processes
pub struct SystemDefaults {
    pub defaults: ProcessLimits,
    pub total_enforcement_checks: u64,
    pub total_limit_exceeded: u64,
    pub total_setrlimit_calls: u64,
}

impl SystemDefaults {
    pub const fn new() -> Self {
        SystemDefaults {
            defaults: ProcessLimits::new(),
            total_enforcement_checks: 0,
            total_limit_exceeded: 0,
            total_setrlimit_calls: 0,
        }
    }

    /// Get default limits for a new process
    pub fn get_defaults(&self) -> ProcessLimits {
        self.defaults.clone()
    }

    /// Update system-wide default for a resource
    pub fn set_default(&mut self, resource: Resource, limit: RLimit) {
        self.defaults.limits[resource as usize] = limit;
    }
}

static SYSTEM_DEFAULTS: Mutex<SystemDefaults> = Mutex::new(SystemDefaults::new());

// ── Per-PID limit storage ────────────────────────────────────────────

/// Maximum PIDs tracked
const MAX_PIDS: usize = 256;

/// Per-process limit storage
static PROCESS_LIMITS: Mutex<[Option<ProcessLimits>; MAX_PIDS]> = {
    const NONE: Option<ProcessLimits> = None;
    Mutex::new([NONE; MAX_PIDS])
};

// ── Public API ───────────────────────────────────────────────────────

/// Initialize resource limits for a new process
pub fn init_process(pid: u32) {
    let defaults = SYSTEM_DEFAULTS.lock().get_defaults();
    let mut limits = PROCESS_LIMITS.lock();
    if (pid as usize) < MAX_PIDS {
        limits[pid as usize] = Some(defaults);
    }
}

/// Initialize limits for a child process (inherit from parent)
pub fn init_child(child_pid: u32, parent_pid: u32) {
    let limits_table = PROCESS_LIMITS.lock();
    let parent_limits = match limits_table[parent_pid as usize].as_ref() {
        Some(l) => l.clone(),
        None => {
            drop(limits_table);
            init_process(child_pid);
            return;
        }
    };
    drop(limits_table);

    let mut child_limits = parent_limits;
    child_limits.usage = [0u64; NUM_RESOURCES];

    let mut limits = PROCESS_LIMITS.lock();
    if (child_pid as usize) < MAX_PIDS {
        limits[child_pid as usize] = Some(child_limits);
    }
}

/// Clean up limits when a process exits
pub fn cleanup_process(pid: u32) {
    let mut limits = PROCESS_LIMITS.lock();
    if (pid as usize) < MAX_PIDS {
        limits[pid as usize] = None;
    }
}

/// Get the resource limit for a process
pub fn getrlimit(pid: u32, resource: Resource) -> Option<RLimit> {
    let limits = PROCESS_LIMITS.lock();
    limits[pid as usize].as_ref().map(|l| l.get(resource))
}

/// Set the resource limit for a process
pub fn setrlimit(
    pid: u32,
    resource: Resource,
    new_limit: RLimit,
    is_privileged: bool,
) -> Result<(), &'static str> {
    let mut limits = PROCESS_LIMITS.lock();
    let proc_limits = limits[pid as usize].as_mut().ok_or("no limits for pid")?;
    proc_limits.set(resource, new_limit, is_privileged)?;
    drop(limits);

    let mut sys = SYSTEM_DEFAULTS.lock();
    sys.total_setrlimit_calls += 1;
    Ok(())
}

/// Check if a resource allocation is allowed (soft limit check)
pub fn check_resource(pid: u32, resource: Resource, amount: u64) -> bool {
    let mut sys = SYSTEM_DEFAULTS.lock();
    sys.total_enforcement_checks += 1;
    drop(sys);

    let limits = PROCESS_LIMITS.lock();
    match limits[pid as usize].as_ref() {
        Some(l) => l.check_soft(resource, amount),
        None => true, // No limits configured = allow
    }
}

/// Try to consume a resource (hard limit enforcement)
pub fn consume_resource(pid: u32, resource: Resource, amount: u64) -> Result<(), &'static str> {
    let mut sys = SYSTEM_DEFAULTS.lock();
    sys.total_enforcement_checks += 1;
    drop(sys);

    let mut limits = PROCESS_LIMITS.lock();
    match limits[pid as usize].as_mut() {
        Some(l) => {
            let result = l.consume(resource, amount);
            if result.is_err() {
                drop(limits);
                SYSTEM_DEFAULTS.lock().total_limit_exceeded += 1;
            }
            result
        }
        None => Ok(()), // No limits = allow
    }
}

/// Release a previously consumed resource
pub fn release_resource(pid: u32, resource: Resource, amount: u64) {
    let mut limits = PROCESS_LIMITS.lock();
    if let Some(l) = limits[pid as usize].as_mut() {
        l.release(resource, amount);
    }
}

/// Get current usage for a resource
pub fn get_usage(pid: u32, resource: Resource) -> u64 {
    let limits = PROCESS_LIMITS.lock();
    limits[pid as usize]
        .as_ref()
        .map(|l| l.get_usage(resource))
        .unwrap_or(0)
}

/// Enforce CPU time limit (called from timer tick)
pub fn check_cpu_time(pid: u32, elapsed_seconds: u64) -> bool {
    let mut limits = PROCESS_LIMITS.lock();
    if let Some(l) = limits[pid as usize].as_mut() {
        l.set_usage(Resource::CpuTime, elapsed_seconds);
        let limit = l.get(Resource::CpuTime);
        if limit.exceeds_hard(elapsed_seconds) {
            serial_println!(
                "    [rlimit] PID {} exceeded CPU time hard limit ({} > {})",
                pid,
                elapsed_seconds,
                limit.hard
            );
            return false; // Kill the process
        }
        if limit.exceeds_soft(elapsed_seconds) {
            serial_println!(
                "    [rlimit] PID {} exceeded CPU time soft limit ({} > {})",
                pid,
                elapsed_seconds,
                limit.soft
            );
            // Send SIGXCPU — caller handles this
        }
    }
    true
}

/// Set system-wide default limit for a resource
pub fn set_system_default(resource: Resource, limit: RLimit) {
    SYSTEM_DEFAULTS.lock().set_default(resource, limit);
}

/// Get enforcement statistics: (checks, exceeded, setrlimit_calls)
pub fn stats() -> (u64, u64, u64) {
    let sys = SYSTEM_DEFAULTS.lock();
    (
        sys.total_enforcement_checks,
        sys.total_limit_exceeded,
        sys.total_setrlimit_calls,
    )
}

/// Initialize the resource limits subsystem
pub fn init() {
    serial_println!("    [rlimit] resource limits initialized (CPU, memory, files, stack)");
}
