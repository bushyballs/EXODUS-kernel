/// CPU affinity management for Genesis
///
/// Manages per-process CPU affinity masks that control which logical
/// processors a process is allowed to run on. Supports explicit pinning,
/// preferred core selection, CPU migration tracking, and NUMA-aware
/// scheduling hints.
///
/// The affinity mask is a 64-bit bitmask where each bit corresponds to a
/// logical CPU. When a process is scheduled, the scheduler consults the
/// affinity mask to determine valid CPUs.
///
/// Inspired by: Linux sched_setaffinity(2), FreeBSD cpuset, Windows
/// SetProcessAffinityMask. All code is original.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Constants ────────────────────────────────────────────────────────

/// Maximum number of logical CPUs supported
pub const MAX_CPUS: usize = 64;

/// Maximum NUMA nodes
pub const MAX_NUMA_NODES: usize = 8;

/// Default affinity: all CPUs allowed (all bits set)
pub const AFFINITY_ALL: u64 = u64::MAX;

// ── CPU topology ─────────────────────────────────────────────────────

/// Physical CPU topology information
#[derive(Debug, Clone, Copy)]
pub struct CpuTopology {
    /// Logical CPU ID (0-63)
    pub cpu_id: u8,
    /// Physical core ID (may share with HT sibling)
    pub core_id: u8,
    /// Package (socket) ID
    pub package_id: u8,
    /// NUMA node ID
    pub numa_node: u8,
    /// Whether this CPU is online
    pub online: bool,
    /// Current load (Q16 fixed-point: 0 = idle, 65536 = 100%)
    pub load_q16: i32,
    /// Number of processes pinned to this CPU
    pub pinned_count: u32,
}

impl CpuTopology {
    pub const fn new(cpu_id: u8) -> Self {
        CpuTopology {
            cpu_id,
            core_id: cpu_id,
            package_id: 0,
            numa_node: 0,
            online: true,
            load_q16: 0,
            pinned_count: 0,
        }
    }
}

// ── NUMA node information ────────────────────────────────────────────

/// NUMA node descriptor
#[derive(Debug, Clone, Copy)]
pub struct NumaNode {
    /// Node ID
    pub id: u8,
    /// CPUs belonging to this node (bitmask)
    pub cpu_mask: u64,
    /// Total memory on this node in bytes
    pub total_memory: u64,
    /// Free memory on this node in bytes
    pub free_memory: u64,
    /// Distance to other nodes (Q16 fixed-point, index = other node)
    pub distances: [i32; MAX_NUMA_NODES],
}

impl NumaNode {
    pub const fn new(id: u8) -> Self {
        // Distance to self = 65536 (1.0 in Q16), all others = 655360 (10.0)
        let mut distances = [655360i32; MAX_NUMA_NODES];
        distances[id as usize] = 65536; // Q16 1.0 = local
        NumaNode {
            id,
            cpu_mask: 0,
            total_memory: 0,
            free_memory: 0,
            distances,
        }
    }
}

// ── Per-process affinity ─────────────────────────────────────────────

/// CPU affinity state for a single process
#[derive(Debug, Clone)]
pub struct ProcessAffinity {
    /// PID
    pub pid: u32,
    /// Allowed CPU mask (bit N = CPU N allowed)
    pub cpu_mask: u64,
    /// Preferred CPU (the scheduler tries this first, -1 = no preference)
    pub preferred_cpu: i8,
    /// Last CPU this process ran on
    pub last_cpu: i8,
    /// NUMA node preference (-1 = no preference)
    pub numa_preference: i8,
    /// Number of CPU migrations (moves between CPUs)
    pub migration_count: u64,
    /// Whether affinity was explicitly set (vs inherited/default)
    pub explicit: bool,
    /// Whether to avoid migrating this process if possible
    pub migration_disabled: bool,
}

impl ProcessAffinity {
    /// Create with default affinity (all CPUs)
    pub fn new(pid: u32) -> Self {
        ProcessAffinity {
            pid,
            cpu_mask: AFFINITY_ALL,
            preferred_cpu: -1,
            last_cpu: -1,
            numa_preference: -1,
            migration_count: 0,
            explicit: false,
            migration_disabled: false,
        }
    }

    /// Check if a specific CPU is allowed
    pub fn is_cpu_allowed(&self, cpu: u8) -> bool {
        if cpu >= MAX_CPUS as u8 {
            return false;
        }
        (self.cpu_mask & (1u64 << cpu)) != 0
    }

    /// Count number of allowed CPUs
    pub fn allowed_cpu_count(&self) -> u32 {
        self.cpu_mask.count_ones()
    }

    /// Get the first allowed CPU
    pub fn first_allowed_cpu(&self) -> Option<u8> {
        if self.cpu_mask == 0 {
            return None;
        }
        Some(self.cpu_mask.trailing_zeros() as u8)
    }

    /// Get all allowed CPUs as a list
    pub fn allowed_cpus(&self) -> Vec<u8> {
        let mut cpus = Vec::new();
        for i in 0..MAX_CPUS as u8 {
            if self.is_cpu_allowed(i) {
                cpus.push(i);
            }
        }
        cpus
    }

    /// Record a CPU migration
    pub fn record_migration(&mut self, from_cpu: i8, to_cpu: i8) {
        if from_cpu != to_cpu && from_cpu >= 0 {
            self.migration_count = self.migration_count.saturating_add(1);
        }
        self.last_cpu = to_cpu;
    }

    /// Select the best CPU for this process given current loads
    pub fn select_cpu(&self, topology: &[CpuTopology]) -> Option<u8> {
        // If preferred CPU is set and allowed, use it
        if self.preferred_cpu >= 0 && self.is_cpu_allowed(self.preferred_cpu as u8) {
            return Some(self.preferred_cpu as u8);
        }

        // If last CPU is allowed, prefer it (cache affinity)
        if self.last_cpu >= 0 && self.is_cpu_allowed(self.last_cpu as u8) {
            let last = self.last_cpu as usize;
            if last < topology.len() && topology[last].online {
                // Only if load is reasonable (below 75% in Q16)
                if topology[last].load_q16 < 49152 {
                    return Some(self.last_cpu as u8);
                }
            }
        }

        // Find the least loaded allowed CPU
        let mut best_cpu: Option<u8> = None;
        let mut best_load = i32::MAX;

        for topo in topology.iter() {
            if !topo.online {
                continue;
            }
            if !self.is_cpu_allowed(topo.cpu_id) {
                continue;
            }
            if topo.load_q16 < best_load {
                best_load = topo.load_q16;
                best_cpu = Some(topo.cpu_id);
            }
        }

        best_cpu
    }

    /// Select the best CPU considering NUMA locality
    pub fn select_cpu_numa(&self, topology: &[CpuTopology], numa_nodes: &[NumaNode]) -> Option<u8> {
        if self.numa_preference < 0 || self.numa_preference as usize >= numa_nodes.len() {
            return self.select_cpu(topology);
        }

        let preferred_node = &numa_nodes[self.numa_preference as usize];
        let node_cpus = preferred_node.cpu_mask;

        // Try CPUs on the preferred NUMA node first
        let mut best_cpu: Option<u8> = None;
        let mut best_load = i32::MAX;

        for topo in topology.iter() {
            if !topo.online || !self.is_cpu_allowed(topo.cpu_id) {
                continue;
            }
            // Prefer NUMA-local CPUs
            let is_local = (node_cpus & (1u64 << topo.cpu_id)) != 0;
            // Q16 bonus for local: reduce apparent load by 25% (16384)
            let effective_load = if is_local {
                topo.load_q16 - 16384
            } else {
                topo.load_q16
            };
            if effective_load < best_load {
                best_load = effective_load;
                best_cpu = Some(topo.cpu_id);
            }
        }

        best_cpu
    }
}

// ── Affinity manager ─────────────────────────────────────────────────

/// Maximum PIDs tracked
const MAX_PIDS: usize = 256;

/// Global affinity manager
pub struct AffinityManager {
    /// Per-process affinity (indexed by PID)
    affinities: [Option<ProcessAffinity>; MAX_PIDS],
    /// CPU topology
    topology: [CpuTopology; MAX_CPUS],
    /// Number of online CPUs
    online_cpus: u32,
    /// NUMA nodes
    numa_nodes: [NumaNode; MAX_NUMA_NODES],
    /// Number of NUMA nodes
    num_numa_nodes: u32,
    /// Total migrations across all processes
    total_migrations: u64,
    /// Total affinity changes
    total_affinity_changes: u64,
}

impl AffinityManager {
    pub const fn new() -> Self {
        const NONE_AFFINITY: Option<ProcessAffinity> = None;
        const fn make_topology() -> [CpuTopology; MAX_CPUS] {
            let mut arr = [CpuTopology::new(0); MAX_CPUS];
            let mut i = 0;
            while i < MAX_CPUS {
                arr[i] = CpuTopology::new(i as u8);
                i += 1;
            }
            arr
        }
        const fn make_numa() -> [NumaNode; MAX_NUMA_NODES] {
            let mut arr = [NumaNode::new(0); MAX_NUMA_NODES];
            let mut i = 0;
            while i < MAX_NUMA_NODES {
                arr[i] = NumaNode::new(i as u8);
                i += 1;
            }
            arr
        }
        AffinityManager {
            affinities: [NONE_AFFINITY; MAX_PIDS],
            topology: make_topology(),
            online_cpus: 1,
            numa_nodes: make_numa(),
            num_numa_nodes: 1,
            total_migrations: 0,
            total_affinity_changes: 0,
        }
    }

    /// Register a new process with default affinity
    pub fn register_process(&mut self, pid: u32) {
        if (pid as usize) < MAX_PIDS {
            self.affinities[pid as usize] = Some(ProcessAffinity::new(pid));
        }
    }

    /// Remove a process
    pub fn unregister_process(&mut self, pid: u32) {
        if (pid as usize) < MAX_PIDS {
            self.affinities[pid as usize] = None;
        }
    }

    /// Set CPU affinity mask for a process
    pub fn set_affinity(&mut self, pid: u32, mask: u64) -> Result<(), &'static str> {
        if mask == 0 {
            return Err("affinity mask cannot be zero");
        }
        let affinity = self.affinities[pid as usize]
            .as_mut()
            .ok_or("process not registered")?;
        affinity.cpu_mask = mask;
        affinity.explicit = true;
        self.total_affinity_changes = self.total_affinity_changes.saturating_add(1);
        serial_println!("    [affinity] PID {} affinity set to {:#018X}", pid, mask);
        Ok(())
    }

    /// Get CPU affinity mask for a process
    pub fn get_affinity(&self, pid: u32) -> Option<u64> {
        self.affinities[pid as usize].as_ref().map(|a| a.cpu_mask)
    }

    /// Set preferred CPU for a process
    pub fn set_preferred_cpu(&mut self, pid: u32, cpu: i8) -> Result<(), &'static str> {
        let affinity = self.affinities[pid as usize]
            .as_mut()
            .ok_or("process not registered")?;

        if cpu >= 0 && !affinity.is_cpu_allowed(cpu as u8) {
            return Err("preferred CPU not in affinity mask");
        }

        affinity.preferred_cpu = cpu;
        Ok(())
    }

    /// Set NUMA node preference for a process
    pub fn set_numa_preference(&mut self, pid: u32, node: i8) -> Result<(), &'static str> {
        if node >= 0 && node as u32 >= self.num_numa_nodes {
            return Err("invalid NUMA node");
        }
        let affinity = self.affinities[pid as usize]
            .as_mut()
            .ok_or("process not registered")?;
        affinity.numa_preference = node;
        Ok(())
    }

    /// Select the best CPU for a process
    pub fn select_cpu_for(&self, pid: u32) -> Option<u8> {
        let affinity = self.affinities[pid as usize].as_ref()?;
        if self.num_numa_nodes > 1 {
            affinity.select_cpu_numa(
                &self.topology[..self.online_cpus as usize],
                &self.numa_nodes[..self.num_numa_nodes as usize],
            )
        } else {
            affinity.select_cpu(&self.topology[..self.online_cpus as usize])
        }
    }

    /// Record that a process was migrated to a new CPU
    pub fn record_migration(&mut self, pid: u32, from_cpu: i8, to_cpu: i8) {
        if let Some(affinity) = self.affinities[pid as usize].as_mut() {
            affinity.record_migration(from_cpu, to_cpu);
            if from_cpu != to_cpu && from_cpu >= 0 {
                self.total_migrations = self.total_migrations.saturating_add(1);
            }
        }
    }

    /// Update CPU load (Q16 fixed-point)
    pub fn update_cpu_load(&mut self, cpu: u8, load_q16: i32) {
        if (cpu as usize) < MAX_CPUS {
            self.topology[cpu as usize].load_q16 = load_q16;
        }
    }

    /// Set the number of online CPUs
    pub fn set_online_cpus(&mut self, count: u32) {
        self.online_cpus = core::cmp::min(count, MAX_CPUS as u32);
    }

    /// Set the number of NUMA nodes
    pub fn set_numa_nodes(&mut self, count: u32) {
        self.num_numa_nodes = core::cmp::min(count, MAX_NUMA_NODES as u32);
    }

    /// Configure NUMA node CPU mask
    pub fn set_numa_cpus(&mut self, node: u8, cpu_mask: u64) {
        if (node as usize) < MAX_NUMA_NODES {
            self.numa_nodes[node as usize].cpu_mask = cpu_mask;
        }
    }

    /// Configure NUMA node memory
    pub fn set_numa_memory(&mut self, node: u8, total: u64, free: u64) {
        if (node as usize) < MAX_NUMA_NODES {
            self.numa_nodes[node as usize].total_memory = total;
            self.numa_nodes[node as usize].free_memory = free;
        }
    }

    /// Disable migration for a process (pin to current CPU)
    pub fn disable_migration(&mut self, pid: u32) -> Result<(), &'static str> {
        let affinity = self.affinities[pid as usize]
            .as_mut()
            .ok_or("process not registered")?;
        affinity.migration_disabled = true;

        // If process has a last_cpu, pin to it
        if affinity.last_cpu >= 0 {
            let cpu = affinity.last_cpu as u8;
            affinity.cpu_mask = 1u64 << cpu;
            affinity.preferred_cpu = cpu as i8;
        }
        Ok(())
    }

    /// Enable migration for a previously pinned process
    pub fn enable_migration(&mut self, pid: u32) -> Result<(), &'static str> {
        let affinity = self.affinities[pid as usize]
            .as_mut()
            .ok_or("process not registered")?;
        affinity.migration_disabled = false;
        // Restore to all-CPUs if not explicitly set
        if !affinity.explicit {
            affinity.cpu_mask = AFFINITY_ALL;
        }
        Ok(())
    }

    /// Get migration count for a process
    pub fn migration_count(&self, pid: u32) -> u64 {
        self.affinities[pid as usize]
            .as_ref()
            .map(|a| a.migration_count)
            .unwrap_or(0)
    }
}

static AFFINITY_MGR: Mutex<AffinityManager> = Mutex::new(AffinityManager::new());

// ── Public API ───────────────────────────────────────────────────────

/// Register a new process with default affinity
pub fn register(pid: u32) {
    AFFINITY_MGR.lock().register_process(pid);
}

/// Unregister a process
pub fn unregister(pid: u32) {
    AFFINITY_MGR.lock().unregister_process(pid);
}

/// Set CPU affinity for a process (sched_setaffinity equivalent)
pub fn set_affinity(pid: u32, mask: u64) -> Result<(), &'static str> {
    AFFINITY_MGR.lock().set_affinity(pid, mask)
}

/// Get CPU affinity for a process (sched_getaffinity equivalent)
pub fn get_affinity(pid: u32) -> Option<u64> {
    AFFINITY_MGR.lock().get_affinity(pid)
}

/// Set preferred CPU for a process
pub fn set_preferred_cpu(pid: u32, cpu: i8) -> Result<(), &'static str> {
    AFFINITY_MGR.lock().set_preferred_cpu(pid, cpu)
}

/// Set NUMA node preference
pub fn set_numa_preference(pid: u32, node: i8) -> Result<(), &'static str> {
    AFFINITY_MGR.lock().set_numa_preference(pid, node)
}

/// Select the best CPU for scheduling a process
pub fn select_cpu(pid: u32) -> Option<u8> {
    AFFINITY_MGR.lock().select_cpu_for(pid)
}

/// Record a CPU migration
pub fn record_migration(pid: u32, from: i8, to: i8) {
    AFFINITY_MGR.lock().record_migration(pid, from, to);
}

/// Update CPU load for scheduling decisions (Q16 fixed-point)
pub fn update_load(cpu: u8, load_q16: i32) {
    AFFINITY_MGR.lock().update_cpu_load(cpu, load_q16);
}

/// Pin a process to its current CPU (disable migration)
pub fn pin_to_current(pid: u32) -> Result<(), &'static str> {
    AFFINITY_MGR.lock().disable_migration(pid)
}

/// Unpin a process (re-enable migration)
pub fn unpin(pid: u32) -> Result<(), &'static str> {
    AFFINITY_MGR.lock().enable_migration(pid)
}

/// Get stats: (online_cpus, total_migrations, total_affinity_changes)
pub fn stats() -> (u32, u64, u64) {
    let mgr = AFFINITY_MGR.lock();
    (
        mgr.online_cpus,
        mgr.total_migrations,
        mgr.total_affinity_changes,
    )
}

/// Initialize the CPU affinity subsystem
pub fn init() {
    // Detect number of CPUs (default: 1 for now; ACPI/MADT will update later)
    let mut mgr = AFFINITY_MGR.lock();
    mgr.set_online_cpus(1);
    mgr.set_numa_nodes(1);
    mgr.set_numa_cpus(0, 0x01); // CPU 0 on NUMA node 0
    drop(mgr);
    serial_println!("    [affinity] CPU affinity manager initialized (NUMA-aware, 64 CPUs max)");
}
