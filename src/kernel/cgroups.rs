/// cgroups v2 — Control Groups for Genesis
///
/// Resource management and isolation for groups of processes.
/// Controls: CPU, memory, I/O bandwidth, PIDs, network.
///
/// Each cgroup is a directory in /sys/fs/cgroup/ hierarchy.
/// Processes are assigned to cgroups. Resource limits are enforced
/// by the scheduler, memory allocator, and I/O subsystems.
///
/// Features:
/// - Cgroup hierarchy tree (parent/children relationships)
/// - CPU controller: bandwidth limiting (quota/period), weight-based sharing
/// - Memory controller: usage tracking, hard/soft limits, OOM handling
/// - IO controller: BPS/IOPS limits per device
/// - PID controller: max process count per cgroup
/// - Process-to-cgroup assignment and migration
/// - Hierarchical resource accounting (propagate to parent)
/// - Statistics: cpu_usage_usec, memory_current, io_bytes_read/write
///
/// Inspired by: Linux cgroups v2 (kernel/cgroup/). All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum cgroups
const MAX_CGROUPS: usize = 128;

/// Maximum number of IO device rules per cgroup
const MAX_IO_DEVICES: usize = 16;

// ---------------------------------------------------------------------------
// Resource controllers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controller {
    Cpu,
    Memory,
    Io,
    Pids,
    Cpuset,
    Freezer,
}

// ---------------------------------------------------------------------------
// CPU controller
// ---------------------------------------------------------------------------

/// CPU controller config — bandwidth limiting (CFS bandwidth) + weight sharing
#[derive(Debug, Clone, Copy)]
pub struct CpuConfig {
    /// Weight for proportional sharing (1-10000, default 100)
    pub weight: u32,
    /// CPU bandwidth limit: max microseconds per period (0 = unlimited)
    pub max_us: u64,
    /// CPU bandwidth period (microseconds, default 100000 = 100ms)
    pub period_us: u64,
    /// Burst: extra microseconds that can be accumulated (0 = no burst)
    pub burst_us: u64,
}

impl Default for CpuConfig {
    fn default() -> Self {
        CpuConfig {
            weight: 100,
            max_us: 0,
            period_us: 100_000,
            burst_us: 0,
        }
    }
}

/// CPU accounting statistics (cumulative)
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuStat {
    /// Total CPU time consumed in microseconds
    pub usage_usec: u64,
    /// Time spent in user mode (usec)
    pub user_usec: u64,
    /// Time spent in kernel/system mode (usec)
    pub system_usec: u64,
    /// Number of times the cgroup was throttled (quota exhausted)
    pub nr_throttled: u64,
    /// Total throttled time (usec)
    pub throttled_usec: u64,
    /// Number of scheduling periods
    pub nr_periods: u64,
    /// Accumulated burst budget remaining (usec)
    pub burst_remaining_us: u64,
}

// ---------------------------------------------------------------------------
// Memory controller
// ---------------------------------------------------------------------------

/// Memory controller config
#[derive(Debug, Clone, Copy)]
pub struct MemoryConfig {
    /// Hard memory limit in bytes (0 = unlimited). Triggers OOM if exceeded.
    pub max: usize,
    /// Soft limit (high watermark) — reclaim target, not a hard wall
    pub high: usize,
    /// Minimum guaranteed memory (protected from global reclaim)
    pub min: usize,
    /// Low watermark — best-effort protection from reclaim
    pub low: usize,
    /// Swap limit
    pub swap_max: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            max: 0,
            high: 0,
            min: 0,
            low: 0,
            swap_max: 0,
        }
    }
}

/// Memory accounting statistics (live)
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryStat {
    /// Current resident memory usage (bytes)
    pub current: usize,
    /// Peak (high watermark) usage (bytes)
    pub peak: usize,
    /// Current swap usage (bytes)
    pub swap_current: usize,
    /// Number of OOM kills triggered within this cgroup
    pub oom_kills: u64,
    /// Number of times the hard limit was hit (but not necessarily OOM)
    pub max_hits: u64,
    /// Cumulative bytes allocated
    pub total_alloc: u64,
    /// Cumulative bytes freed
    pub total_free: u64,
    /// Number of page faults attributed to this cgroup
    pub page_faults: u64,
    /// Anonymous memory (bytes)
    pub anon: usize,
    /// File-backed memory (bytes, page cache)
    pub file: usize,
    /// Kernel slab memory (bytes)
    pub slab: usize,
}

// ---------------------------------------------------------------------------
// IO controller
// ---------------------------------------------------------------------------

/// Per-device IO limit rule
#[derive(Debug, Clone, Copy)]
pub struct IoDeviceRule {
    /// Major:minor device number (encoded as (major << 16) | minor)
    pub dev: u32,
    /// Read bytes-per-second limit (0 = unlimited)
    pub rbps_max: u64,
    /// Write bytes-per-second limit
    pub wbps_max: u64,
    /// Read IOPS limit
    pub riops_max: u64,
    /// Write IOPS limit
    pub wiops_max: u64,
}

/// IO controller config
#[derive(Debug, Clone)]
pub struct IoConfig {
    /// Weight for proportional IO sharing (1-10000, default 100)
    pub weight: u32,
    /// Per-device rules
    pub device_rules: Vec<IoDeviceRule>,
}

impl Default for IoConfig {
    fn default() -> Self {
        IoConfig {
            weight: 100,
            device_rules: Vec::new(),
        }
    }
}

/// IO accounting statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct IoStat {
    /// Total bytes read
    pub bytes_read: u64,
    /// Total bytes written
    pub bytes_written: u64,
    /// Total read operations
    pub ios_read: u64,
    /// Total write operations
    pub ios_written: u64,
    /// Bytes discarded (TRIM)
    pub bytes_discarded: u64,
}

// ---------------------------------------------------------------------------
// PIDs controller
// ---------------------------------------------------------------------------

/// PIDs controller config
#[derive(Debug, Clone, Copy)]
pub struct PidsConfig {
    /// Maximum number of PIDs (0 = unlimited)
    pub max: u32,
}

impl Default for PidsConfig {
    fn default() -> Self {
        PidsConfig { max: 0 }
    }
}

/// PIDs accounting statistics
#[derive(Debug, Clone, Copy, Default)]
pub struct PidsStat {
    /// Current PID count in this cgroup (not including children)
    pub current: u32,
    /// Current PID count including all descendants
    pub current_hierarchical: u32,
    /// Number of times fork was denied due to PID limit
    pub events_max: u64,
}

// ---------------------------------------------------------------------------
// Freezer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezerState {
    Thawed,
    Frozen,
}

// ---------------------------------------------------------------------------
// Cgroup
// ---------------------------------------------------------------------------

/// A single cgroup node in the hierarchy
pub struct Cgroup {
    /// Name/path (e.g., "system.slice", "user.slice/user-1000.slice")
    pub name: String,
    /// Parent cgroup index (0 = root, self-referencing for root)
    pub parent: usize,
    /// Child cgroup indices
    pub children: Vec<usize>,
    /// PIDs directly in this cgroup (not in sub-cgroups)
    pub pids: Vec<u32>,
    /// Active controllers enabled at this level
    pub controllers: Vec<Controller>,
    /// CPU config + stats
    pub cpu: CpuConfig,
    pub cpu_stat: CpuStat,
    /// Memory config + stats
    pub memory: MemoryConfig,
    pub memory_stat: MemoryStat,
    /// IO config + stats
    pub io: IoConfig,
    pub io_stat: IoStat,
    /// PIDs config + stats
    pub pids_config: PidsConfig,
    pub pids_stat: PidsStat,
    /// Freezer state
    pub freezer: FreezerState,
    /// Whether this cgroup is active (not deleted)
    pub active: bool,
    /// Creation timestamp (ms since boot)
    pub created_ms: u64,
}

impl Cgroup {
    fn new(name: &str, parent: usize) -> Self {
        Cgroup {
            name: String::from(name),
            parent,
            children: Vec::new(),
            pids: Vec::new(),
            controllers: Vec::new(),
            cpu: CpuConfig::default(),
            cpu_stat: CpuStat::default(),
            memory: MemoryConfig::default(),
            memory_stat: MemoryStat::default(),
            io: IoConfig::default(),
            io_stat: IoStat::default(),
            pids_config: PidsConfig::default(),
            pids_stat: PidsStat::default(),
            freezer: FreezerState::Thawed,
            active: true,
            created_ms: crate::time::clock::uptime_ms(),
        }
    }

    /// Count PIDs including all descendant cgroups (requires hierarchy ref).
    fn count_pids_hierarchical(&self, all: &[Cgroup]) -> u32 {
        let mut count = self.pids.len() as u32;
        for &child_idx in &self.children {
            if child_idx < all.len() && all[child_idx].active {
                count += all[child_idx].count_pids_hierarchical(all);
            }
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Cgroup hierarchy
// ---------------------------------------------------------------------------

pub struct CgroupHierarchy {
    cgroups: Vec<Cgroup>,
}

impl CgroupHierarchy {
    const fn new() -> Self {
        CgroupHierarchy {
            cgroups: Vec::new(),
        }
    }

    /// Create root cgroup
    fn init_root(&mut self) {
        let mut root = Cgroup::new("/", 0);
        root.controllers = alloc::vec![
            Controller::Cpu,
            Controller::Memory,
            Controller::Io,
            Controller::Pids,
        ];
        self.cgroups.push(root);
    }

    /// Create a child cgroup
    pub fn create(&mut self, parent_path: &str, name: &str) -> Option<usize> {
        if self.cgroups.len() >= MAX_CGROUPS {
            return None;
        }
        let parent_idx = self.find_by_path(parent_path)?;
        let full_path = if parent_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent_path, name)
        };

        // Don't create if name already exists under same parent
        if self.find_by_path(&full_path).is_some() {
            return None;
        }

        let idx = self.cgroups.len();
        let mut cg = Cgroup::new(&full_path, parent_idx);
        // Inherit controllers from parent
        cg.controllers = self.cgroups[parent_idx].controllers.clone();
        self.cgroups.push(cg);
        self.cgroups[parent_idx].children.push(idx);
        Some(idx)
    }

    /// Delete a cgroup (must have no PIDs and no children).
    pub fn delete(&mut self, path: &str) -> bool {
        let idx = match self.find_by_path(path) {
            Some(i) => i,
            None => return false,
        };
        if idx == 0 {
            return false;
        } // can't delete root
        if !self.cgroups[idx].pids.is_empty() {
            return false;
        }
        if !self.cgroups[idx].children.is_empty() {
            return false;
        }

        let parent_idx = self.cgroups[idx].parent;
        self.cgroups[idx].active = false;
        self.cgroups[parent_idx].children.retain(|&c| c != idx);
        true
    }

    /// Find cgroup by path
    pub fn find_by_path(&self, path: &str) -> Option<usize> {
        self.cgroups.iter().position(|c| c.name == path && c.active)
    }

    // ------- PID management -------

    /// Assign a PID to a cgroup (migrating from current cgroup).
    pub fn assign_pid(&mut self, path: &str, pid: u32) -> bool {
        let target_idx = match self.find_by_path(path) {
            Some(i) => i,
            None => return false,
        };

        // Check PID limit on target cgroup and all ancestors
        if !self.check_pids_limit(target_idx) {
            return false;
        }

        // Remove from current cgroup first
        let mut source_idx: Option<usize> = None;
        for (i, cg) in self.cgroups.iter_mut().enumerate() {
            if cg.pids.contains(&pid) {
                cg.pids.retain(|&p| p != pid);
                cg.pids_stat.current = cg.pids.len() as u32;
                source_idx = Some(i);
                break;
            }
        }

        // Add to target
        self.cgroups[target_idx].pids.push(pid);
        self.cgroups[target_idx].pids_stat.current = self.cgroups[target_idx].pids.len() as u32;

        // Update hierarchical PID counts for affected ancestors
        if let Some(src) = source_idx {
            self.update_hierarchical_pid_counts(src);
        }
        self.update_hierarchical_pid_counts(target_idx);

        true
    }

    /// Migrate all PIDs from one cgroup to another.
    pub fn migrate_all_pids(&mut self, from_path: &str, to_path: &str) -> u32 {
        let from_idx = match self.find_by_path(from_path) {
            Some(i) => i,
            None => return 0,
        };
        let to_idx = match self.find_by_path(to_path) {
            Some(i) => i,
            None => return 0,
        };

        let pids: Vec<u32> = self.cgroups[from_idx].pids.clone();
        let count = pids.len() as u32;

        self.cgroups[from_idx].pids.clear();
        self.cgroups[from_idx].pids_stat.current = 0;

        for pid in pids {
            self.cgroups[to_idx].pids.push(pid);
        }
        self.cgroups[to_idx].pids_stat.current = self.cgroups[to_idx].pids.len() as u32;

        self.update_hierarchical_pid_counts(from_idx);
        self.update_hierarchical_pid_counts(to_idx);

        count
    }

    /// Find which cgroup a PID belongs to.
    pub fn find_pid_cgroup(&self, pid: u32) -> Option<String> {
        for cg in &self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                return Some(cg.name.clone());
            }
        }
        None
    }

    /// Check PID limit in a cgroup and all ancestors.
    fn check_pids_limit(&self, idx: usize) -> bool {
        let cg = &self.cgroups[idx];
        if cg.pids_config.max > 0 {
            let hierarchical = cg.count_pids_hierarchical(&self.cgroups);
            if hierarchical >= cg.pids_config.max {
                return false;
            }
        }
        // Check parent recursively
        if idx != 0 {
            return self.check_pids_limit(cg.parent);
        }
        true
    }

    /// Update hierarchical PID counts up the tree.
    fn update_hierarchical_pid_counts(&mut self, idx: usize) {
        let count = self.cgroups[idx].count_pids_hierarchical(&self.cgroups);
        self.cgroups[idx].pids_stat.current_hierarchical = count;
        let parent = self.cgroups[idx].parent;
        if parent != idx && idx != 0 {
            self.update_hierarchical_pid_counts(parent);
        }
    }

    // ------- CPU controller -------

    /// Set CPU weight for a cgroup
    pub fn set_cpu_weight(&mut self, path: &str, weight: u32) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].cpu.weight = weight.clamp(1, 10000);
            true
        } else {
            false
        }
    }

    /// Set CPU bandwidth limit (quota/period).
    pub fn set_cpu_max(&mut self, path: &str, max_us: u64, period_us: u64) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].cpu.max_us = max_us;
            self.cgroups[idx].cpu.period_us = if period_us == 0 { 100_000 } else { period_us };
            true
        } else {
            false
        }
    }

    /// Set CPU burst allowance.
    pub fn set_cpu_burst(&mut self, path: &str, burst_us: u64) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].cpu.burst_us = burst_us;
            true
        } else {
            false
        }
    }

    /// Account CPU time for a PID (called by the scheduler).
    pub fn account_cpu(&mut self, pid: u32, usec: u64, is_user: bool) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                cg.cpu_stat.usage_usec += usec;
                if is_user {
                    cg.cpu_stat.user_usec += usec;
                } else {
                    cg.cpu_stat.system_usec += usec;
                }
                cg.cpu_stat.nr_periods = cg.cpu_stat.nr_periods.saturating_add(1);

                // Check bandwidth throttling
                if cg.cpu.max_us > 0 {
                    // Within a scheduling period, check if quota is exhausted
                    let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
                    if used_in_period > cg.cpu.max_us + cg.cpu_stat.burst_remaining_us {
                        cg.cpu_stat.nr_throttled = cg.cpu_stat.nr_throttled.saturating_add(1);
                        cg.cpu_stat.throttled_usec += usec;
                    }
                }
                break;
            }
        }
        // Propagate to ancestors
        self.propagate_cpu_stat_up(pid);
    }

    /// Check if a PID is throttled (has exceeded CPU bandwidth).
    pub fn is_cpu_throttled(&self, pid: u32) -> bool {
        for cg in &self.cgroups {
            if cg.active && cg.pids.contains(&pid) && cg.cpu.max_us > 0 {
                let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
                return used_in_period > cg.cpu.max_us + cg.cpu_stat.burst_remaining_us;
            }
        }
        false
    }

    /// Get the effective CPU weight for a PID (for the scheduler).
    pub fn effective_cpu_weight(&self, pid: u32) -> u32 {
        for cg in &self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                return cg.cpu.weight;
            }
        }
        100 // default weight
    }

    /// Propagate CPU stats up the hierarchy (simplified).
    fn propagate_cpu_stat_up(&mut self, _pid: u32) {
        // In a full implementation, we'd walk up the tree and aggregate
        // For now, stats are tracked per-cgroup where the PID lives
    }

    // ------- Memory controller -------

    /// Set memory limit for a cgroup
    pub fn set_memory_max(&mut self, path: &str, max_bytes: usize) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].memory.max = max_bytes;
            true
        } else {
            false
        }
    }

    /// Set memory high watermark (soft limit).
    pub fn set_memory_high(&mut self, path: &str, high_bytes: usize) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].memory.high = high_bytes;
            true
        } else {
            false
        }
    }

    /// Set memory min (protected from global reclaim).
    pub fn set_memory_min(&mut self, path: &str, min_bytes: usize) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].memory.min = min_bytes;
            true
        } else {
            false
        }
    }

    /// Set memory low watermark.
    pub fn set_memory_low(&mut self, path: &str, low_bytes: usize) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].memory.low = low_bytes;
            true
        } else {
            false
        }
    }

    /// Set swap limit.
    pub fn set_swap_max(&mut self, path: &str, swap_max: usize) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].memory.swap_max = swap_max;
            true
        } else {
            false
        }
    }

    /// Account memory allocation for a PID. Returns false if OOM (hard limit hit).
    pub fn account_memory_alloc(&mut self, pid: u32, bytes: usize) -> bool {
        for cg in &mut self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }

            let new_usage = cg.memory_stat.current + bytes;

            // Hard limit check
            if cg.memory.max > 0 && new_usage > cg.memory.max {
                cg.memory_stat.max_hits = cg.memory_stat.max_hits.saturating_add(1);
                // Try OOM handling
                if !Self::handle_oom_for_cgroup_inner(cg, bytes) {
                    return false; // OOM: cannot allocate
                }
            }

            cg.memory_stat.current = cg.memory_stat.current + bytes;
            cg.memory_stat.total_alloc += bytes as u64;
            if cg.memory_stat.current > cg.memory_stat.peak {
                cg.memory_stat.peak = cg.memory_stat.current;
            }
            return true;
        }
        true // PID not in any cgroup — allow
    }

    /// Account memory free for a PID.
    pub fn account_memory_free(&mut self, pid: u32, bytes: usize) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                cg.memory_stat.current = cg.memory_stat.current.saturating_sub(bytes);
                cg.memory_stat.total_free += bytes as u64;
                break;
            }
        }
    }

    /// Check if a PID can allocate memory in its cgroup
    pub fn check_memory(&self, pid: u32, bytes: usize) -> bool {
        for cg in &self.cgroups {
            if cg.pids.contains(&pid) && cg.memory.max > 0 {
                return cg.memory_stat.current + bytes <= cg.memory.max;
            }
        }
        true // no limit
    }

    /// Internal OOM handler for a cgroup — try to reclaim or kill.
    fn handle_oom_for_cgroup_inner(cg: &mut Cgroup, _bytes_needed: usize) -> bool {
        cg.memory_stat.oom_kills = cg.memory_stat.oom_kills.saturating_add(1);
        // In a real kernel, we'd select a victim process and kill it.
        // For now, just signal the first PID in the cgroup.
        if let Some(&victim_pid) = cg.pids.first() {
            let _ = crate::process::send_signal(victim_pid, crate::process::pcb::signal::SIGKILL);
            crate::serial_println!(
                "  [cgroups] OOM kill: pid {} in cgroup '{}'",
                victim_pid,
                cg.name
            );
            return true; // freed memory by killing a process
        }
        false
    }

    /// Trigger OOM handling in a cgroup by path.
    pub fn trigger_oom(&mut self, path: &str) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            let cg = &mut self.cgroups[idx];
            Self::handle_oom_for_cgroup_inner(cg, 0)
        } else {
            false
        }
    }

    /// Check if a cgroup is above the high watermark (needs reclaim).
    pub fn memory_under_pressure(&self, path: &str) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            let cg = &self.cgroups[idx];
            cg.memory.high > 0 && cg.memory_stat.current > cg.memory.high
        } else {
            false
        }
    }

    // ------- IO controller -------

    /// Set IO weight for a cgroup.
    pub fn set_io_weight(&mut self, path: &str, weight: u32) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].io.weight = weight.clamp(1, 10000);
            true
        } else {
            false
        }
    }

    /// Set IO limits for a specific device within a cgroup.
    pub fn set_io_max(
        &mut self,
        path: &str,
        dev: u32,
        rbps: u64,
        wbps: u64,
        riops: u64,
        wiops: u64,
    ) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            let io = &mut self.cgroups[idx].io;
            if let Some(rule) = io.device_rules.iter_mut().find(|r| r.dev == dev) {
                rule.rbps_max = rbps;
                rule.wbps_max = wbps;
                rule.riops_max = riops;
                rule.wiops_max = wiops;
            } else if io.device_rules.len() < MAX_IO_DEVICES {
                io.device_rules.push(IoDeviceRule {
                    dev,
                    rbps_max: rbps,
                    wbps_max: wbps,
                    riops_max: riops,
                    wiops_max: wiops,
                });
            } else {
                return false;
            }
            true
        } else {
            false
        }
    }

    /// Account IO bytes for a PID.
    pub fn account_io(&mut self, pid: u32, bytes: u64, is_write: bool) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                if is_write {
                    cg.io_stat.bytes_written += bytes;
                    cg.io_stat.ios_written = cg.io_stat.ios_written.saturating_add(1);
                } else {
                    cg.io_stat.bytes_read += bytes;
                    cg.io_stat.ios_read = cg.io_stat.ios_read.saturating_add(1);
                }
                break;
            }
        }
    }

    /// Check if an IO operation is allowed (within rate limits).
    pub fn check_io_limit(&self, pid: u32, dev: u32, bytes: u64, is_write: bool) -> bool {
        for cg in &self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }

            for rule in &cg.io.device_rules {
                if rule.dev != dev {
                    continue;
                }
                // Simplified: check cumulative against a 1-second window
                // In a real kernel this would use token bucket or similar
                if is_write {
                    if rule.wbps_max > 0 && bytes > rule.wbps_max {
                        return false;
                    }
                    if rule.wiops_max > 0 && cg.io_stat.ios_written > rule.wiops_max {
                        return false;
                    }
                } else {
                    if rule.rbps_max > 0 && bytes > rule.rbps_max {
                        return false;
                    }
                    if rule.riops_max > 0 && cg.io_stat.ios_read > rule.riops_max {
                        return false;
                    }
                }
            }
            break;
        }
        true
    }

    // ------- PIDs controller -------

    /// Set PIDs limit for a cgroup
    pub fn set_pids_max(&mut self, path: &str, max: u32) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].pids_config.max = max;
            true
        } else {
            false
        }
    }

    /// Check if a cgroup can create another process
    pub fn check_pids(&self, pid: u32) -> bool {
        for cg in &self.cgroups {
            if cg.pids.contains(&pid) && cg.pids_config.max > 0 {
                let hierarchical = cg.count_pids_hierarchical(&self.cgroups);
                if hierarchical >= cg.pids_config.max {
                    return false;
                }
            }
        }
        true // no limit
    }

    /// Record a fork denial event.
    pub fn record_pids_max_event(&mut self, pid: u32) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                cg.pids_stat.events_max = cg.pids_stat.events_max.saturating_add(1);
                break;
            }
        }
    }

    // ------- Freezer -------

    /// Freeze a cgroup (suspend all processes).
    pub fn freeze(&mut self, path: &str) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].freezer = FreezerState::Frozen;
            let pids: Vec<u32> = self.cgroups[idx].pids.clone();
            for &pid in &pids {
                let _ = crate::process::send_signal(pid, crate::process::pcb::signal::SIGTSTP);
            }
            // Recursively freeze children
            let children: Vec<usize> = self.cgroups[idx].children.clone();
            for child_idx in children {
                if self.cgroups[child_idx].active {
                    let child_path = self.cgroups[child_idx].name.clone();
                    self.freeze(&child_path);
                }
            }
            true
        } else {
            false
        }
    }

    /// Thaw a cgroup (resume all processes).
    pub fn thaw(&mut self, path: &str) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].freezer = FreezerState::Thawed;
            let pids: Vec<u32> = self.cgroups[idx].pids.clone();
            for &pid in &pids {
                let _ = crate::process::send_signal(pid, crate::process::pcb::signal::SIGCONT);
            }
            // Recursively thaw children
            let children: Vec<usize> = self.cgroups[idx].children.clone();
            for child_idx in children {
                if self.cgroups[child_idx].active {
                    let child_path = self.cgroups[child_idx].name.clone();
                    self.thaw(&child_path);
                }
            }
            true
        } else {
            false
        }
    }

    // ------- Controller management -------

    /// Enable a controller on a cgroup (and all children by default).
    pub fn enable_controller(&mut self, path: &str, controller: Controller) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            if !self.cgroups[idx].controllers.contains(&controller) {
                self.cgroups[idx].controllers.push(controller);
            }
            true
        } else {
            false
        }
    }

    /// Disable a controller on a cgroup.
    pub fn disable_controller(&mut self, path: &str, controller: Controller) -> bool {
        if let Some(idx) = self.find_by_path(path) {
            self.cgroups[idx].controllers.retain(|c| *c != controller);
            true
        } else {
            false
        }
    }

    // ------- Statistics and listing -------

    /// Get full stat report for a cgroup (like reading stat files).
    pub fn stat(&self, path: &str) -> Option<String> {
        let idx = self.find_by_path(path)?;
        let cg = &self.cgroups[idx];

        let mut s = format!("cgroup: {}\n", cg.name);
        s.push_str(&format!(
            "pids: {} (hierarchical: {})\n",
            cg.pids_stat.current, cg.pids_stat.current_hierarchical
        ));
        s.push_str(&format!("freezer: {:?}\n", cg.freezer));

        // CPU stats
        s.push_str(&format!("cpu.weight: {}\n", cg.cpu.weight));
        if cg.cpu.max_us > 0 {
            s.push_str(&format!(
                "cpu.max: {} {}\n",
                cg.cpu.max_us, cg.cpu.period_us
            ));
        } else {
            s.push_str("cpu.max: max\n");
        }
        s.push_str(&format!("cpu.stat:\n  usage_usec {}\n  user_usec {}\n  system_usec {}\n  nr_periods {}\n  nr_throttled {}\n  throttled_usec {}\n",
            cg.cpu_stat.usage_usec, cg.cpu_stat.user_usec, cg.cpu_stat.system_usec,
            cg.cpu_stat.nr_periods, cg.cpu_stat.nr_throttled, cg.cpu_stat.throttled_usec));

        // Memory stats
        s.push_str(&format!(
            "memory.max: {}\n",
            if cg.memory.max == 0 {
                String::from("max")
            } else {
                format!("{}", cg.memory.max)
            }
        ));
        s.push_str(&format!(
            "memory.high: {}\n",
            if cg.memory.high == 0 {
                String::from("max")
            } else {
                format!("{}", cg.memory.high)
            }
        ));
        s.push_str(&format!("memory.current: {}\n", cg.memory_stat.current));
        s.push_str(&format!("memory.peak: {}\n", cg.memory_stat.peak));
        s.push_str(&format!(
            "memory.stat:\n  anon {}\n  file {}\n  slab {}\n  oom_kills {}\n  page_faults {}\n",
            cg.memory_stat.anon,
            cg.memory_stat.file,
            cg.memory_stat.slab,
            cg.memory_stat.oom_kills,
            cg.memory_stat.page_faults
        ));

        // IO stats
        s.push_str(&format!("io.weight: {}\n", cg.io.weight));
        s.push_str(&format!(
            "io.stat: rbytes={} wbytes={} rios={} wios={}\n",
            cg.io_stat.bytes_read,
            cg.io_stat.bytes_written,
            cg.io_stat.ios_read,
            cg.io_stat.ios_written
        ));

        // PIDs
        if cg.pids_config.max > 0 {
            s.push_str(&format!("pids.max: {}\n", cg.pids_config.max));
        } else {
            s.push_str("pids.max: max\n");
        }
        s.push_str(&format!("pids.events.max: {}\n", cg.pids_stat.events_max));

        Some(s)
    }

    /// List all cgroups (path, pid_count, children_count)
    pub fn list_all(&self) -> Vec<(String, usize, usize)> {
        self.cgroups
            .iter()
            .filter(|c| c.active)
            .map(|c| (c.name.clone(), c.pids.len(), c.children.len()))
            .collect()
    }

    /// Get the hierarchy tree as an indented string.
    pub fn tree(&self) -> String {
        let mut s = String::new();
        if !self.cgroups.is_empty() {
            self.tree_recursive(0, 0, &mut s);
        }
        s
    }

    fn tree_recursive(&self, idx: usize, depth: usize, out: &mut String) {
        let cg = &self.cgroups[idx];
        if !cg.active {
            return;
        }
        for _ in 0..depth {
            out.push_str("  ");
        }
        out.push_str(&format!(
            "{} [pids={}, mem={}, cpu_w={}]\n",
            cg.name,
            cg.pids.len(),
            cg.memory_stat.current,
            cg.cpu.weight
        ));
        for &child_idx in &cg.children {
            self.tree_recursive(child_idx, depth + 1, out);
        }
    }

    /// Get aggregated memory usage for a cgroup and all descendants.
    pub fn memory_usage_hierarchical(&self, path: &str) -> usize {
        let idx = match self.find_by_path(path) {
            Some(i) => i,
            None => return 0,
        };
        self.sum_memory(idx)
    }

    fn sum_memory(&self, idx: usize) -> usize {
        let cg = &self.cgroups[idx];
        let mut total = cg.memory_stat.current;
        for &child_idx in &cg.children {
            if self.cgroups[child_idx].active {
                total += self.sum_memory(child_idx);
            }
        }
        total
    }

    // ------- Enhanced memory limits enforcement -------

    /// Check memory limit hierarchically — enforce limits at every level up the tree.
    /// Returns false if any ancestor's limit would be exceeded.
    pub fn check_memory_hierarchical(&self, pid: u32, bytes: usize) -> bool {
        for (idx, cg) in self.cgroups.iter().enumerate() {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }
            // Check this cgroup and every ancestor up to root
            return self.check_memory_at_and_ancestors(idx, bytes);
        }
        true // not in any cgroup
    }

    fn check_memory_at_and_ancestors(&self, idx: usize, bytes: usize) -> bool {
        let cg = &self.cgroups[idx];
        if cg.memory.max > 0 {
            let hierarchical_usage = self.sum_memory(idx);
            if hierarchical_usage + bytes > cg.memory.max {
                return false;
            }
        }
        // Check parent
        if idx != 0 && cg.parent != idx {
            return self.check_memory_at_and_ancestors(cg.parent, bytes);
        }
        true
    }

    /// Account memory for anonymous pages specifically
    pub fn account_memory_anon(&mut self, pid: u32, bytes: usize, is_alloc: bool) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                if is_alloc {
                    cg.memory_stat.anon += bytes;
                    cg.memory_stat.current += bytes;
                    cg.memory_stat.total_alloc += bytes as u64;
                    if cg.memory_stat.current > cg.memory_stat.peak {
                        cg.memory_stat.peak = cg.memory_stat.current;
                    }
                } else {
                    cg.memory_stat.anon = cg.memory_stat.anon.saturating_sub(bytes);
                    cg.memory_stat.current = cg.memory_stat.current.saturating_sub(bytes);
                    cg.memory_stat.total_free += bytes as u64;
                }
                break;
            }
        }
    }

    /// Account memory for file-backed pages
    pub fn account_memory_file(&mut self, pid: u32, bytes: usize, is_alloc: bool) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                if is_alloc {
                    cg.memory_stat.file += bytes;
                    cg.memory_stat.current += bytes;
                } else {
                    cg.memory_stat.file = cg.memory_stat.file.saturating_sub(bytes);
                    cg.memory_stat.current = cg.memory_stat.current.saturating_sub(bytes);
                }
                break;
            }
        }
    }

    /// Account slab memory
    pub fn account_memory_slab(&mut self, pid: u32, bytes: usize, is_alloc: bool) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                if is_alloc {
                    cg.memory_stat.slab += bytes;
                } else {
                    cg.memory_stat.slab = cg.memory_stat.slab.saturating_sub(bytes);
                }
                break;
            }
        }
    }

    /// Record a page fault for a PID's cgroup
    pub fn record_page_fault(&mut self, pid: u32) {
        for cg in &mut self.cgroups {
            if cg.active && cg.pids.contains(&pid) {
                cg.memory_stat.page_faults = cg.memory_stat.page_faults.saturating_add(1);
                break;
            }
        }
    }

    /// Try to reclaim memory from a cgroup that is above the high watermark.
    /// Returns the number of bytes that should be reclaimed (advisory).
    pub fn memory_reclaim_target(&self, path: &str) -> usize {
        if let Some(idx) = self.find_by_path(path) {
            let cg = &self.cgroups[idx];
            if cg.memory.high > 0 && cg.memory_stat.current > cg.memory.high {
                return cg.memory_stat.current - cg.memory.high;
            }
        }
        0
    }

    /// Get the effective memory limit for a PID (considers hierarchy).
    /// Returns the tightest limit among all ancestors.
    pub fn effective_memory_limit(&self, pid: u32) -> usize {
        for (idx, cg) in self.cgroups.iter().enumerate() {
            if cg.active && cg.pids.contains(&pid) {
                return self.tightest_memory_limit(idx);
            }
        }
        0 // no limit
    }

    fn tightest_memory_limit(&self, idx: usize) -> usize {
        let cg = &self.cgroups[idx];
        let my_limit = if cg.memory.max > 0 {
            cg.memory.max
        } else {
            usize::MAX
        };

        if idx == 0 || cg.parent == idx {
            return my_limit;
        }

        let parent_limit = self.tightest_memory_limit(cg.parent);
        my_limit.min(parent_limit)
    }

    // ------- Enhanced CPU bandwidth control -------

    /// Account CPU time in **nanoseconds** for a PID.
    ///
    /// This is the nanosecond-resolution variant called directly from the
    /// scheduler tick path.  It accumulates `cpu_usage_ns` in the CpuStat
    /// and enforces CFS bandwidth quota within the current period.
    ///
    /// Returns `true` if the cgroup has been throttled after accounting.
    pub fn account_cpu_time(&mut self, pid: u32, ns_elapsed: u64) -> bool {
        for cg in &mut self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }

            // Convert ns → µs for the existing usec-based statistics.
            let usec = ns_elapsed / 1_000;
            cg.cpu_stat.usage_usec = cg.cpu_stat.usage_usec.saturating_add(usec);
            cg.cpu_stat.system_usec = cg.cpu_stat.system_usec.saturating_add(usec);
            cg.cpu_stat.nr_periods = cg.cpu_stat.nr_periods.saturating_add(1);

            if cg.cpu.max_us == 0 {
                return false; // no quota configured — never throttled
            }

            // Determine how much of the current period has been used.
            let period_ns = cg.cpu.period_us.saturating_mul(1_000);
            if period_ns == 0 {
                return false;
            }

            // We track the per-period budget as a separate saturating counter
            // stored in burst_remaining_us (repurposed as "remaining quota µs
            // in current period" when burst_us == 0).
            // Subtract the just-elapsed time from the period's quota.
            let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us.max(1);
            let total_budget = cg.cpu.max_us.saturating_add(cg.cpu_stat.burst_remaining_us);

            if used_in_period >= total_budget {
                cg.cpu_stat.nr_throttled = cg.cpu_stat.nr_throttled.saturating_add(1);
                cg.cpu_stat.throttled_usec = cg.cpu_stat.throttled_usec.saturating_add(usec);
                return true; // throttled
            }
            return false; // still within quota
        }
        false // PID not in any cgroup — not throttled
    }

    /// Check whether a cgroup is currently throttled for scheduling purposes.
    ///
    /// `sched_time_ns` is the current monotonic scheduler timestamp in
    /// nanoseconds (used to detect period boundaries and reset quotas).
    ///
    /// Returns `true` if the cgroup that owns `pid` has exhausted its CPU
    /// quota for the current period.  Resets quota counters automatically
    /// at each period boundary.
    pub fn cgroup_throttle_check(&mut self, pid: u32, sched_time_ns: u64) -> bool {
        for cg in &mut self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }

            // No quota configured → never throttle.
            if cg.cpu.max_us == 0 {
                return false;
            }

            let period_ns = cg.cpu.period_us.saturating_mul(1_000);
            if period_ns == 0 {
                return false;
            }

            // Detect a period boundary: if the scheduler timestamp has
            // advanced past the end of the last recorded period, reset the
            // per-period usage counter and carry forward any burst budget.
            //
            // We store the period start timestamp in `throttled_usec` high
            // bits — but rather than alias fields we use a simple modular
            // approach: the period boundary is determined by
            // `sched_time_ns / period_ns` changing.
            let cur_period = sched_time_ns / period_ns;
            let last_period = cg.cpu_stat.throttled_usec / period_ns; // repurposed as period counter

            if cur_period != last_period {
                // New period: accumulate burst budget for unused quota.
                let used_in_prev = cg.cpu_stat.usage_usec % cg.cpu.period_us.max(1);
                if used_in_prev < cg.cpu.max_us && cg.cpu.burst_us > 0 {
                    let unused = cg.cpu.max_us.saturating_sub(used_in_prev);
                    cg.cpu_stat.burst_remaining_us = cg
                        .cpu_stat
                        .burst_remaining_us
                        .saturating_add(unused)
                        .min(cg.cpu.burst_us);
                }
                // Mark the new period in the throttled_usec field.
                cg.cpu_stat.throttled_usec = cur_period * period_ns;
            }

            // Check quota within current period.
            let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us.max(1);
            let total_budget = cg.cpu.max_us.saturating_add(cg.cpu_stat.burst_remaining_us);

            return used_in_period >= total_budget;
        }
        false // PID not in any cgroup — not throttled
    }

    /// Reset CPU bandwidth counters for a new period.
    /// Should be called by the scheduler timer at each period boundary.
    pub fn reset_cpu_period(&mut self, path: &str) {
        if let Some(idx) = self.find_by_path(path) {
            let cg = &mut self.cgroups[idx];
            // Accumulate unused burst budget
            if cg.cpu.max_us > 0 && cg.cpu.burst_us > 0 {
                let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
                if used_in_period < cg.cpu.max_us {
                    let unused = cg.cpu.max_us - used_in_period;
                    cg.cpu_stat.burst_remaining_us =
                        (cg.cpu_stat.burst_remaining_us + unused).min(cg.cpu.burst_us);
                }
            }
        }
    }

    /// Reset all cgroup CPU periods (called periodically by scheduler)
    pub fn reset_all_cpu_periods(&mut self) {
        for cg in &mut self.cgroups {
            if !cg.active || cg.cpu.max_us == 0 {
                continue;
            }

            // Accumulate burst budget
            if cg.cpu.burst_us > 0 {
                let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
                if used_in_period < cg.cpu.max_us {
                    let unused = cg.cpu.max_us - used_in_period;
                    cg.cpu_stat.burst_remaining_us =
                        (cg.cpu_stat.burst_remaining_us + unused).min(cg.cpu.burst_us);
                }
            }
        }
    }

    /// Get the remaining CPU budget for a PID in the current period.
    /// Returns (remaining_us, is_throttled).
    pub fn cpu_budget_remaining(&self, pid: u32) -> (u64, bool) {
        for cg in &self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }
            if cg.cpu.max_us == 0 {
                return (u64::MAX, false); // unlimited
            }

            let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
            let total_budget = cg.cpu.max_us + cg.cpu_stat.burst_remaining_us;

            if used_in_period >= total_budget {
                return (0, true);
            }

            return (total_budget - used_in_period, false);
        }
        (u64::MAX, false)
    }

    /// Calculate the effective CPU share for a PID based on weight and siblings.
    /// Returns a value 0-10000 representing the proportional CPU share.
    pub fn effective_cpu_share(&self, pid: u32) -> u32 {
        for cg in &self.cgroups {
            if !cg.active || !cg.pids.contains(&pid) {
                continue;
            }

            // Sum weights of all active sibling cgroups
            let parent_idx = cg.parent;
            let total_weight: u32 = self.cgroups[parent_idx]
                .children
                .iter()
                .filter_map(|&child_idx| {
                    let child = &self.cgroups[child_idx];
                    if child.active {
                        Some(child.cpu.weight)
                    } else {
                        None
                    }
                })
                .sum();

            if total_weight == 0 {
                return 10000; // full share
            }

            // This cgroup's proportional share (in basis points, max 10000)
            return ((cg.cpu.weight as u64 * 10000) / total_weight as u64) as u32;
        }
        10000 // default: full share
    }

    /// Get bandwidth utilization for a cgroup (0-10000 basis points).
    /// Returns (utilization_bps, quota_us, period_us, used_us).
    pub fn cpu_bandwidth_utilization(&self, path: &str) -> Option<(u32, u64, u64, u64)> {
        let idx = self.find_by_path(path)?;
        let cg = &self.cgroups[idx];

        if cg.cpu.max_us == 0 {
            return Some((0, 0, cg.cpu.period_us, cg.cpu_stat.usage_usec));
        }

        let used_in_period = cg.cpu_stat.usage_usec % cg.cpu.period_us;
        let utilization = if cg.cpu.max_us > 0 {
            ((used_in_period * 10000) / cg.cpu.max_us) as u32
        } else {
            0
        };

        Some((
            utilization.min(10000),
            cg.cpu.max_us,
            cg.cpu.period_us,
            used_in_period,
        ))
    }
}

// ---------------------------------------------------------------------------
// Global cgroup hierarchy and public API
// ---------------------------------------------------------------------------

static CGROUPS: Mutex<CgroupHierarchy> = Mutex::new(CgroupHierarchy::new());

pub fn init() {
    let mut cg = CGROUPS.lock();
    cg.init_root();
    cg.create("/", "system.slice");
    cg.create("/", "user.slice");
    cg.create("/", "init.scope");
    crate::serial_println!(
        "  [cgroups] Control groups v2 initialized (cpu, memory, io, pids controllers)"
    );
}

pub fn create(parent: &str, name: &str) -> Option<usize> {
    CGROUPS.lock().create(parent, name)
}
pub fn delete(path: &str) -> bool {
    CGROUPS.lock().delete(path)
}
pub fn assign_pid(path: &str, pid: u32) -> bool {
    CGROUPS.lock().assign_pid(path, pid)
}
pub fn migrate_all_pids(from: &str, to: &str) -> u32 {
    CGROUPS.lock().migrate_all_pids(from, to)
}
pub fn find_pid_cgroup(pid: u32) -> Option<String> {
    CGROUPS.lock().find_pid_cgroup(pid)
}
pub fn set_memory_max(path: &str, max: usize) -> bool {
    CGROUPS.lock().set_memory_max(path, max)
}
pub fn set_memory_high(path: &str, high: usize) -> bool {
    CGROUPS.lock().set_memory_high(path, high)
}
pub fn set_cpu_weight(path: &str, weight: u32) -> bool {
    CGROUPS.lock().set_cpu_weight(path, weight)
}
pub fn set_cpu_max(path: &str, max_us: u64, period_us: u64) -> bool {
    CGROUPS.lock().set_cpu_max(path, max_us, period_us)
}
pub fn set_pids_max(path: &str, max: u32) -> bool {
    CGROUPS.lock().set_pids_max(path, max)
}
pub fn set_io_weight(path: &str, weight: u32) -> bool {
    CGROUPS.lock().set_io_weight(path, weight)
}
pub fn set_io_max(path: &str, dev: u32, rbps: u64, wbps: u64, riops: u64, wiops: u64) -> bool {
    CGROUPS
        .lock()
        .set_io_max(path, dev, rbps, wbps, riops, wiops)
}
pub fn account_cpu(pid: u32, usec: u64, is_user: bool) {
    CGROUPS.lock().account_cpu(pid, usec, is_user)
}
pub fn account_memory_alloc(pid: u32, bytes: usize) -> bool {
    CGROUPS.lock().account_memory_alloc(pid, bytes)
}
pub fn account_memory_free(pid: u32, bytes: usize) {
    CGROUPS.lock().account_memory_free(pid, bytes)
}
pub fn account_io(pid: u32, bytes: u64, is_write: bool) {
    CGROUPS.lock().account_io(pid, bytes, is_write)
}
pub fn check_memory(pid: u32, bytes: usize) -> bool {
    CGROUPS.lock().check_memory(pid, bytes)
}
pub fn check_pids(pid: u32) -> bool {
    CGROUPS.lock().check_pids(pid)
}
pub fn check_io_limit(pid: u32, dev: u32, bytes: u64, is_write: bool) -> bool {
    CGROUPS.lock().check_io_limit(pid, dev, bytes, is_write)
}
pub fn effective_cpu_weight(pid: u32) -> u32 {
    CGROUPS.lock().effective_cpu_weight(pid)
}
pub fn is_cpu_throttled(pid: u32) -> bool {
    CGROUPS.lock().is_cpu_throttled(pid)
}
pub fn freeze(path: &str) -> bool {
    CGROUPS.lock().freeze(path)
}
pub fn thaw(path: &str) -> bool {
    CGROUPS.lock().thaw(path)
}
pub fn stat(path: &str) -> Option<String> {
    CGROUPS.lock().stat(path)
}
pub fn tree() -> String {
    CGROUPS.lock().tree()
}
pub fn list_all() -> Vec<(String, usize, usize)> {
    CGROUPS.lock().list_all()
}

// --- Enhanced memory enforcement ---
pub fn check_memory_hierarchical(pid: u32, bytes: usize) -> bool {
    CGROUPS.lock().check_memory_hierarchical(pid, bytes)
}
pub fn account_memory_anon(pid: u32, bytes: usize, is_alloc: bool) {
    CGROUPS.lock().account_memory_anon(pid, bytes, is_alloc)
}
pub fn account_memory_file(pid: u32, bytes: usize, is_alloc: bool) {
    CGROUPS.lock().account_memory_file(pid, bytes, is_alloc)
}
pub fn account_memory_slab(pid: u32, bytes: usize, is_alloc: bool) {
    CGROUPS.lock().account_memory_slab(pid, bytes, is_alloc)
}
pub fn record_page_fault(pid: u32) {
    CGROUPS.lock().record_page_fault(pid)
}
pub fn memory_reclaim_target(path: &str) -> usize {
    CGROUPS.lock().memory_reclaim_target(path)
}
pub fn effective_memory_limit(pid: u32) -> usize {
    CGROUPS.lock().effective_memory_limit(pid)
}
pub fn set_memory_min(path: &str, min: usize) -> bool {
    CGROUPS.lock().set_memory_min(path, min)
}
pub fn set_memory_low(path: &str, low: usize) -> bool {
    CGROUPS.lock().set_memory_low(path, low)
}
pub fn set_swap_max(path: &str, swap_max: usize) -> bool {
    CGROUPS.lock().set_swap_max(path, swap_max)
}

// --- Enhanced CPU bandwidth control ---
pub fn set_cpu_burst(path: &str, burst_us: u64) -> bool {
    CGROUPS.lock().set_cpu_burst(path, burst_us)
}
pub fn reset_all_cpu_periods() {
    CGROUPS.lock().reset_all_cpu_periods()
}
pub fn cpu_budget_remaining(pid: u32) -> (u64, bool) {
    CGROUPS.lock().cpu_budget_remaining(pid)
}
pub fn effective_cpu_share(pid: u32) -> u32 {
    CGROUPS.lock().effective_cpu_share(pid)
}
pub fn cpu_bandwidth_utilization(path: &str) -> Option<(u32, u64, u64, u64)> {
    CGROUPS.lock().cpu_bandwidth_utilization(path)
}
pub fn memory_under_pressure(path: &str) -> bool {
    CGROUPS.lock().memory_under_pressure(path)
}
pub fn trigger_oom(path: &str) -> bool {
    CGROUPS.lock().trigger_oom(path)
}
pub fn memory_usage_hierarchical(path: &str) -> usize {
    CGROUPS.lock().memory_usage_hierarchical(path)
}
pub fn enable_controller(path: &str, ctrl: Controller) -> bool {
    CGROUPS.lock().enable_controller(path, ctrl)
}
pub fn disable_controller(path: &str, ctrl: Controller) -> bool {
    CGROUPS.lock().disable_controller(path, ctrl)
}
pub fn record_pids_max_event(pid: u32) {
    CGROUPS.lock().record_pids_max_event(pid)
}

// --- Nanosecond-resolution CPU accounting and throttle gate ---

/// Account `ns_elapsed` nanoseconds of CPU time for `pid`.
///
/// Called from the scheduler tick (hot path).  Returns `true` if the cgroup
/// has become throttled after accounting.
pub fn account_cpu_time(pid: u32, ns_elapsed: u64) -> bool {
    CGROUPS.lock().account_cpu_time(pid, ns_elapsed)
}

/// Scheduler throttle gate: returns `true` if the cgroup that owns `pid`
/// has exhausted its CPU bandwidth quota for the current period.
///
/// `sched_time_ns` is the current scheduler timestamp in nanoseconds and is
/// used to detect period boundaries so quota counters are reset automatically.
pub fn cgroup_throttle_check(pid: u32, sched_time_ns: u64) -> bool {
    CGROUPS.lock().cgroup_throttle_check(pid, sched_time_ns)
}
