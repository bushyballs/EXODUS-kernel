/// Cgroup resource management for services
///
/// Part of the AIOS init_system subsystem.
///
/// Implements a hierarchical control-group system that enforces resource
/// limits (CPU weight, memory ceiling, I/O weight) on a per-service basis.
/// Processes are assigned to cgroups and their cumulative resource usage
/// is tracked. Enforcement is cooperative: the scheduler and memory
/// allocator query these limits.
///
/// Original implementation for Hoags OS. No external crates.

use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── FNV-1a helper ──────────────────────────────────────────────────────────

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── Resource limits ────────────────────────────────────────────────────────

/// Resource limits for a cgroup.
#[derive(Clone, Copy)]
pub struct CgroupLimits {
    /// Maximum memory in bytes (0 = unlimited).
    pub max_memory_bytes: u64,
    /// CPU weight (1-10000, default 100). Higher = more CPU share.
    pub cpu_weight: u32,
    /// I/O weight (1-10000, default 100). Higher = more I/O bandwidth.
    pub io_weight: u32,
    /// Maximum number of processes (0 = unlimited).
    pub max_pids: u32,
    /// CPU quota: percentage * 100 (e.g., 5000 = 50%). 0 = no limit.
    pub cpu_quota_centipct: u32,
}

impl CgroupLimits {
    /// Default limits: no restrictions.
    pub fn default() -> Self {
        CgroupLimits {
            max_memory_bytes: 0,
            cpu_weight: 100,
            io_weight: 100,
            max_pids: 0,
            cpu_quota_centipct: 0,
        }
    }
}

// ── Resource accounting ────────────────────────────────────────────────────

/// Accumulated resource usage for a cgroup.
#[derive(Clone, Copy)]
struct CgroupUsage {
    /// Current memory usage in bytes.
    memory_bytes: u64,
    /// Total CPU time consumed in microseconds.
    cpu_time_us: u64,
    /// Total I/O bytes read.
    io_read_bytes: u64,
    /// Total I/O bytes written.
    io_write_bytes: u64,
    /// Number of OOM kills.
    oom_kills: u32,
    /// Number of times CPU quota was throttled.
    cpu_throttle_count: u32,
}

impl CgroupUsage {
    fn new() -> Self {
        CgroupUsage {
            memory_bytes: 0,
            cpu_time_us: 0,
            io_read_bytes: 0,
            io_write_bytes: 0,
            oom_kills: 0,
            cpu_throttle_count: 0,
        }
    }
}

// ── Cgroup node ────────────────────────────────────────────────────────────

/// A single cgroup in the hierarchy.
#[derive(Clone)]
struct CgroupNode {
    /// Service name this cgroup belongs to.
    name: String,
    name_hash: u64,
    /// Resource limits.
    limits: CgroupLimits,
    /// Resource usage tracking.
    usage: CgroupUsage,
    /// PIDs assigned to this cgroup.
    pids: Vec<u64>,
    /// Index of parent cgroup (None = root).
    parent: Option<usize>,
    /// Indices of child cgroups.
    children: Vec<usize>,
    /// Whether this cgroup is active.
    active: bool,
}

// ── Cgroup manager ─────────────────────────────────────────────────────────

/// Manages the cgroup hierarchy and resource enforcement.
struct CgroupManagerInner {
    /// All cgroup nodes. Index 0 is the root cgroup.
    nodes: Vec<CgroupNode>,
}

impl CgroupManagerInner {
    fn new() -> Self {
        // Create root cgroup
        let root = CgroupNode {
            name: String::from("/"),
            name_hash: fnv1a_hash(b"/"),
            limits: CgroupLimits::default(),
            usage: CgroupUsage::new(),
            pids: Vec::new(),
            parent: None,
            children: Vec::new(),
            active: true,
        };

        CgroupManagerInner {
            nodes: { let mut v = Vec::new(); v.push(root); v },
        }
    }

    /// Find a cgroup by service name.
    fn find_index(&self, service: &str) -> Option<usize> {
        let hash = fnv1a_hash(service.as_bytes());
        self.nodes.iter().position(|n| n.name_hash == hash)
    }

    /// Create a cgroup for a service with given limits.
    fn create(&mut self, service: &str, limits: CgroupLimits) -> usize {
        // Check for existing
        if let Some(idx) = self.find_index(service) {
            // Update limits on existing cgroup
            self.nodes[idx].limits = limits;
            self.nodes[idx].active = true;
            return idx;
        }

        let idx = self.nodes.len();
        self.nodes.push(CgroupNode {
            name: String::from(service),
            name_hash: fnv1a_hash(service.as_bytes()),
            limits,
            usage: CgroupUsage::new(),
            pids: Vec::new(),
            parent: Some(0), // parent is root
            children: Vec::new(),
            active: true,
        });

        // Add as child of root
        self.nodes[0].children.push(idx);

        serial_println!(
            "[init_system::cgroup_mgr] created cgroup for {} (mem_max={}, cpu_wt={}, io_wt={})",
            service, limits.max_memory_bytes, limits.cpu_weight, limits.io_weight
        );

        idx
    }

    /// Assign a process to a service's cgroup.
    fn assign_process(&mut self, service: &str, pid: u64) -> Result<(), ()> {
        let idx = self.find_index(service).ok_or(())?;

        // Check max PIDs limit
        if self.nodes[idx].limits.max_pids > 0
            && self.nodes[idx].pids.len() as u32 >= self.nodes[idx].limits.max_pids
        {
            serial_println!(
                "[init_system::cgroup_mgr] {} pid limit reached (max={})",
                service, self.nodes[idx].limits.max_pids
            );
            return Err(());
        }

        if !self.nodes[idx].pids.contains(&pid) {
            self.nodes[idx].pids.push(pid);
        }

        Ok(())
    }

    /// Remove a process from its cgroup.
    fn remove_process(&mut self, service: &str, pid: u64) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].pids.retain(|p| *p != pid);
        }
    }

    /// Update resource limits for an existing cgroup.
    fn update_limits(&mut self, service: &str, limits: CgroupLimits) -> Result<(), ()> {
        let idx = self.find_index(service).ok_or(())?;
        self.nodes[idx].limits = limits;
        serial_println!(
            "[init_system::cgroup_mgr] updated limits for {} (mem_max={}, cpu_wt={})",
            service, limits.max_memory_bytes, limits.cpu_weight
        );
        Ok(())
    }

    /// Check if a memory allocation would exceed the cgroup limit.
    fn check_memory(&self, service: &str, alloc_bytes: u64) -> bool {
        let idx = match self.find_index(service) {
            Some(i) => i,
            None => return true, // no cgroup = no limit
        };

        let limit = self.nodes[idx].limits.max_memory_bytes;
        if limit == 0 {
            return true; // unlimited
        }

        self.nodes[idx].usage.memory_bytes + alloc_bytes <= limit
    }

    /// Record a memory allocation for accounting.
    fn account_memory_alloc(&mut self, service: &str, bytes: u64) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].usage.memory_bytes += bytes;

            // Check if we've hit the limit
            let limit = self.nodes[idx].limits.max_memory_bytes;
            if limit > 0 && self.nodes[idx].usage.memory_bytes > limit {
                self.nodes[idx].usage.oom_kills = self.nodes[idx].usage.oom_kills.saturating_add(1);
                serial_println!(
                    "[init_system::cgroup_mgr] OOM for {} (usage={}, limit={})",
                    service, self.nodes[idx].usage.memory_bytes, limit
                );
            }
        }
    }

    /// Record a memory free for accounting.
    fn account_memory_free(&mut self, service: &str, bytes: u64) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].usage.memory_bytes =
                self.nodes[idx].usage.memory_bytes.saturating_sub(bytes);
        }
    }

    /// Record CPU time for accounting.
    fn account_cpu_time(&mut self, service: &str, us: u64) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].usage.cpu_time_us += us;
        }
    }

    /// Record I/O for accounting.
    fn account_io(&mut self, service: &str, read_bytes: u64, write_bytes: u64) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].usage.io_read_bytes += read_bytes;
            self.nodes[idx].usage.io_write_bytes += write_bytes;
        }
    }

    /// Get CPU weight for scheduling decisions.
    fn get_cpu_weight(&self, service: &str) -> u32 {
        match self.find_index(service) {
            Some(idx) => self.nodes[idx].limits.cpu_weight,
            None => 100, // default weight
        }
    }

    /// Deactivate a cgroup (service stopped).
    fn deactivate(&mut self, service: &str) {
        if let Some(idx) = self.find_index(service) {
            self.nodes[idx].active = false;
            self.nodes[idx].pids.clear();
            self.nodes[idx].usage = CgroupUsage::new();
        }
    }

    /// Get total memory usage across all active cgroups.
    fn total_memory_usage(&self) -> u64 {
        self.nodes.iter()
            .filter(|n| n.active)
            .map(|n| n.usage.memory_bytes)
            .sum()
    }

    /// Get count of active cgroups.
    fn active_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.active && n.parent.is_some()).count()
    }
}

/// Public wrapper matching original stub API.
pub struct CgroupManager {
    inner: CgroupManagerInner,
}

impl CgroupManager {
    pub fn new() -> Self {
        CgroupManager {
            inner: CgroupManagerInner::new(),
        }
    }

    /// Create a cgroup for a service with the given limits.
    pub fn create(&mut self, service: &str, limits: CgroupLimits) {
        self.inner.create(service, limits);
    }

    /// Place a process into a service's cgroup.
    pub fn assign_process(&mut self, service: &str, pid: u64) {
        let _ = self.inner.assign_process(service, pid);
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static CGROUP_MGR: Mutex<Option<CgroupManagerInner>> = Mutex::new(None);

/// Initialize the cgroup subsystem.
pub fn init() {
    let mut guard = CGROUP_MGR.lock();
    *guard = Some(CgroupManagerInner::new());
    serial_println!("[init_system::cgroup_mgr] cgroup manager initialized");
}

/// Create a cgroup for a service.
pub fn create(service: &str, limits: CgroupLimits) -> usize {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.create(service, limits)
}

/// Assign a process to a cgroup.
pub fn assign_process(service: &str, pid: u64) -> Result<(), ()> {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.assign_process(service, pid)
}

/// Remove a process from a cgroup.
pub fn remove_process(service: &str, pid: u64) {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.remove_process(service, pid);
}

/// Check if a memory allocation is allowed by cgroup limits.
pub fn check_memory(service: &str, alloc_bytes: u64) -> bool {
    let guard = CGROUP_MGR.lock();
    let mgr = guard.as_ref().expect("cgroup manager not initialized");
    mgr.check_memory(service, alloc_bytes)
}

/// Record a memory allocation.
pub fn account_alloc(service: &str, bytes: u64) {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.account_memory_alloc(service, bytes);
}

/// Record a memory free.
pub fn account_free(service: &str, bytes: u64) {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.account_memory_free(service, bytes);
}

/// Get CPU weight for scheduling.
pub fn cpu_weight(service: &str) -> u32 {
    let guard = CGROUP_MGR.lock();
    let mgr = guard.as_ref().expect("cgroup manager not initialized");
    mgr.get_cpu_weight(service)
}

/// Deactivate a cgroup when service stops.
pub fn deactivate(service: &str) {
    let mut guard = CGROUP_MGR.lock();
    let mgr = guard.as_mut().expect("cgroup manager not initialized");
    mgr.deactivate(service);
}

/// Get total memory usage across active cgroups.
pub fn total_memory() -> u64 {
    let guard = CGROUP_MGR.lock();
    let mgr = guard.as_ref().expect("cgroup manager not initialized");
    mgr.total_memory_usage()
}

/// Get number of active cgroups.
pub fn active_count() -> usize {
    let guard = CGROUP_MGR.lock();
    let mgr = guard.as_ref().expect("cgroup manager not initialized");
    mgr.active_count()
}
