/// CPU Topology — NUMA awareness and CPU topology discovery for Genesis
///
/// Discovers and represents the hardware topology of the system:
/// - Physical packages (sockets)
/// - Cores per package
/// - Hardware threads (SMT/hyperthreading) per core
/// - NUMA nodes and memory affinity
/// - Cache topology (L1/L2/L3 shared between cores)
///
/// This information is used by the scheduler for:
/// - Cache-aware task placement (prefer same L2/L3)
/// - NUMA-aware memory allocation
/// - SMT-aware scheduling (avoid co-scheduling competing tasks on SMT siblings)
/// - Power management (consolidate tasks to fewer packages)
///
/// Topology is detected via CPUID (leaf 0x0B extended topology enumeration)
/// and ACPI SRAT (System Resource Affinity Table) for NUMA.
///
/// Inspired by: Linux arch/x86/kernel/cpu/topology.c, kernel/sched/topology.c
/// All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum cache levels
const MAX_CACHE_LEVELS: usize = 4;

// ---------------------------------------------------------------------------
// Cache topology
// ---------------------------------------------------------------------------

/// CPU cache level information
#[derive(Debug, Clone, Copy)]
pub struct CacheInfo {
    /// Cache level (1=L1, 2=L2, 3=L3)
    pub level: u8,
    /// Cache type: 1=data, 2=instruction, 3=unified
    pub cache_type: u8,
    /// Cache size in bytes
    pub size: u32,
    /// Line size in bytes
    pub line_size: u16,
    /// Associativity (ways)
    pub associativity: u16,
    /// Number of sets
    pub sets: u32,
    /// Number of CPUs sharing this cache
    pub shared_cpus: u32,
    /// Bitmask of CPUs sharing this cache
    pub shared_mask: u64,
}

impl CacheInfo {
    const fn empty() -> Self {
        CacheInfo {
            level: 0,
            cache_type: 0,
            size: 0,
            line_size: 0,
            associativity: 0,
            sets: 0,
            shared_cpus: 0,
            shared_mask: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// CPU topology info
// ---------------------------------------------------------------------------

/// Per-CPU topology information
#[derive(Debug, Clone)]
pub struct CpuTopology {
    /// CPU index (0-based)
    pub cpu_id: u32,
    /// APIC ID
    pub apic_id: u32,
    /// Physical package (socket) ID
    pub package_id: u32,
    /// Core ID within the package
    pub core_id: u32,
    /// Thread ID within the core (SMT thread)
    pub thread_id: u32,
    /// NUMA node this CPU belongs to
    pub numa_node: u32,
    /// Whether this is an SMT sibling (hyperthreaded)
    pub is_smt: bool,
    /// SMT sibling CPU index (if SMT is enabled)
    pub smt_sibling: Option<u32>,
    /// L1 cache info
    pub l1_cache: CacheInfo,
    /// L2 cache info
    pub l2_cache: CacheInfo,
    /// L3 cache info (shared across cores in package)
    pub l3_cache: CacheInfo,
    /// Bitmask of CPUs sharing L2 cache
    pub l2_siblings: u64,
    /// Bitmask of CPUs sharing L3 cache (same package)
    pub l3_siblings: u64,
    /// CPU frequency (MHz, 0 if unknown)
    pub frequency_mhz: u32,
    /// Whether this CPU is online
    pub online: bool,
}

impl CpuTopology {
    const fn new() -> Self {
        CpuTopology {
            cpu_id: 0,
            apic_id: 0,
            package_id: 0,
            core_id: 0,
            thread_id: 0,
            numa_node: 0,
            is_smt: false,
            smt_sibling: None,
            l1_cache: CacheInfo::empty(),
            l2_cache: CacheInfo::empty(),
            l3_cache: CacheInfo::empty(),
            l2_siblings: 0,
            l3_siblings: 0,
            frequency_mhz: 0,
            online: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NUMA node
// ---------------------------------------------------------------------------

/// NUMA node information
#[derive(Debug, Clone)]
pub struct NumaNode {
    /// Node ID
    pub id: u32,
    /// CPUs in this node (bitmask)
    pub cpu_mask: u64,
    /// Memory start address
    pub mem_start: u64,
    /// Memory end address
    pub mem_end: u64,
    /// Total memory in bytes
    pub mem_total: u64,
    /// Available memory in bytes
    pub mem_available: u64,
    /// Distance to other nodes (index = node_id, value = distance)
    pub distances: Vec<u32>,
}

impl NumaNode {
    fn new(id: u32) -> Self {
        NumaNode {
            id,
            cpu_mask: 0,
            mem_start: 0,
            mem_end: 0,
            mem_total: 0,
            mem_available: 0,
            distances: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scheduling domain
// ---------------------------------------------------------------------------

/// Scheduling domain levels (from tightest to loosest)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedDomainLevel {
    /// SMT siblings (hyperthreaded core)
    Smt,
    /// Cores sharing L2 cache
    CoreL2,
    /// Cores within a package (sharing L3)
    Package,
    /// NUMA node
    Numa,
    /// Entire system
    System,
}

/// A scheduling domain — group of CPUs at a topology level
#[derive(Debug, Clone)]
pub struct SchedDomain {
    /// Domain level
    pub level: SchedDomainLevel,
    /// CPU mask (which CPUs are in this domain)
    pub cpu_mask: u64,
    /// Number of CPUs in this domain
    pub num_cpus: u32,
    /// Balance interval (ms) — how often to attempt load balancing within this domain
    pub balance_interval_ms: u64,
    /// Imbalance threshold (minimum load difference to trigger migration)
    pub imbalance_threshold: u32,
}

// ---------------------------------------------------------------------------
// Topology subsystem
// ---------------------------------------------------------------------------

struct TopologySubsystem {
    /// Per-CPU topology info
    cpus: Vec<CpuTopology>,
    /// NUMA nodes
    numa_nodes: Vec<NumaNode>,
    /// Scheduling domains (sorted tightest to loosest)
    sched_domains: Vec<SchedDomain>,
    /// Number of packages (sockets)
    num_packages: u32,
    /// Number of cores per package
    cores_per_package: u32,
    /// Number of threads per core (1 or 2 for SMT)
    threads_per_core: u32,
    /// Total online CPUs
    num_cpus: u32,
    /// Whether SMT (hyperthreading) is detected
    smt_detected: bool,
    /// Whether NUMA is detected
    numa_detected: bool,
}

impl TopologySubsystem {
    const fn new() -> Self {
        TopologySubsystem {
            cpus: Vec::new(),
            numa_nodes: Vec::new(),
            sched_domains: Vec::new(),
            num_packages: 1,
            cores_per_package: 1,
            threads_per_core: 1,
            num_cpus: 1,
            smt_detected: false,
            numa_detected: false,
        }
    }

    /// Detect CPU topology using CPUID
    fn detect_topology(&mut self) {
        let ncpus = crate::smp::num_cpus().max(1);
        self.num_cpus = ncpus;

        // Query CPUID leaf 1 for basic topology info
        let (max_logical, initial_apic_id) = unsafe {
            let _eax: u32;
            let ebx_val: u32;
            core::arch::asm!(
                "push rbx",
                "mov eax, 1",
                "cpuid",
                "mov {0:e}, eax",
                "mov {1:e}, ebx",
                "pop rbx",
                out(reg) _eax,
                out(reg) ebx_val,
                out("ecx") _,
                out("edx") _,
            );
            let max_logical = (ebx_val >> 16) & 0xFF;
            let initial_apic_id = (ebx_val >> 24) & 0xFF;
            (max_logical, initial_apic_id)
        };

        // Check CPUID leaf 4 for cores per package
        let cores_per_pkg = unsafe {
            let eax: u32;
            core::arch::asm!(
                "push rbx",
                "mov eax, 4",
                "xor ecx, ecx",
                "cpuid",
                "mov {0:e}, eax",
                "pop rbx",
                out(reg) eax,
                out("ecx") _,
                out("edx") _,
            );
            ((eax >> 26) & 0x3F) + 1
        };

        self.cores_per_package = cores_per_pkg;
        self.threads_per_core = if max_logical > cores_per_pkg && cores_per_pkg > 0 {
            max_logical / cores_per_pkg
        } else {
            1
        };
        self.smt_detected = self.threads_per_core > 1;
        self.num_packages = if cores_per_pkg > 0 {
            (ncpus + cores_per_pkg - 1) / cores_per_pkg
        } else {
            1
        };

        // Detect cache topology from CPUID leaf 4
        let caches = self.detect_caches();

        // Build per-CPU topology entries
        for i in 0..ncpus as usize {
            let apic_id = if i == 0 { initial_apic_id } else { i as u32 };
            let package_id = apic_id / self.cores_per_package;
            let core_in_pkg = apic_id % self.cores_per_package;
            let thread_in_core = if self.smt_detected {
                core_in_pkg % self.threads_per_core
            } else {
                0
            };
            let core_id = if self.smt_detected {
                core_in_pkg / self.threads_per_core
            } else {
                core_in_pkg
            };

            let mut topo = CpuTopology::new();
            topo.cpu_id = i as u32;
            topo.apic_id = apic_id;
            topo.package_id = package_id;
            topo.core_id = core_id;
            topo.thread_id = thread_in_core;
            topo.numa_node = 0; // default to node 0; NUMA detection fills this later
            topo.is_smt = self.smt_detected && thread_in_core > 0;
            topo.online = true;

            // Assign cache info
            if caches.len() > 0 {
                topo.l1_cache = caches[0];
            }
            if caches.len() > 1 {
                topo.l2_cache = caches[1];
            }
            if caches.len() > 2 {
                topo.l3_cache = caches[2];
            }

            self.cpus.push(topo);
        }

        // Compute sibling masks
        self.compute_sibling_masks();

        // Set up default NUMA node (single node for non-NUMA)
        self.setup_default_numa();

        // Build scheduling domains
        self.build_sched_domains();
    }

    /// Detect cache topology using CPUID leaf 4
    fn detect_caches(&self) -> Vec<CacheInfo> {
        let mut caches: Vec<CacheInfo> = Vec::new();

        for subleaf in 0..MAX_CACHE_LEVELS {
            let (eax, ebx, ecx) = unsafe {
                let eax_out: u32;
                let ebx_out: u32;
                let ecx_out: u32;
                core::arch::asm!(
                    "push rbx",
                    "mov eax, 4",
                    "mov ecx, {0:e}",
                    "cpuid",
                    "mov {1:e}, eax",
                    "mov {2:e}, ebx",
                    "mov {3:e}, ecx",
                    "pop rbx",
                    in(reg) subleaf as u32,
                    out(reg) eax_out,
                    out(reg) ebx_out,
                    out(reg) ecx_out,
                    out("edx") _,
                );
                (eax_out, ebx_out, ecx_out)
            };

            let cache_type = eax & 0x1F;
            if cache_type == 0 {
                break;
            } // no more cache levels

            let level = ((eax >> 5) & 0x07) as u8;
            let line_size = ((ebx & 0xFFF) + 1) as u16;
            let partitions = ((ebx >> 12) & 0x3FF) + 1;
            let assoc = ((ebx >> 22) & 0x3FF) + 1;
            let sets = ecx + 1;
            let shared_cpus = ((eax >> 14) & 0xFFF) + 1;

            let size = line_size as u32 * partitions * assoc * sets;

            caches.push(CacheInfo {
                level,
                cache_type: cache_type as u8,
                size,
                line_size,
                associativity: assoc as u16,
                sets,
                shared_cpus,
                shared_mask: 0, // filled in later
            });
        }

        caches
    }

    /// Compute sibling masks for SMT, L2, L3
    fn compute_sibling_masks(&mut self) {
        let n = self.cpus.len();

        for i in 0..n {
            let mut l2_mask: u64 = 0;
            let mut l3_mask: u64 = 0;
            let pkg_i = self.cpus[i].package_id;
            let core_i = self.cpus[i].core_id;

            for j in 0..n {
                // L2 siblings: same package and same core
                if self.cpus[j].package_id == pkg_i && self.cpus[j].core_id == core_i {
                    l2_mask |= 1u64 << j;

                    // SMT sibling
                    if i != j && self.smt_detected {
                        self.cpus[i].smt_sibling = Some(j as u32);
                    }
                }

                // L3 siblings: same package
                if self.cpus[j].package_id == pkg_i {
                    l3_mask |= 1u64 << j;
                }
            }

            self.cpus[i].l2_siblings = l2_mask;
            self.cpus[i].l3_siblings = l3_mask;
        }
    }

    /// Set up default NUMA topology (single node)
    fn setup_default_numa(&mut self) {
        let mut node = NumaNode::new(0);
        for cpu in &self.cpus {
            node.cpu_mask |= 1u64 << cpu.cpu_id;
        }
        // Default: all memory in node 0
        node.mem_start = 0;
        node.mem_end = 0xFFFF_FFFF_FFFF_FFFF; // placeholder
        node.distances.push(10); // distance to self = 10 (ACPI convention)
        self.numa_nodes.push(node);
    }

    /// Build scheduling domains based on detected topology
    fn build_sched_domains(&mut self) {
        self.sched_domains.clear();

        // SMT domain (if hyperthreading detected)
        if self.smt_detected {
            for i in 0..self.cpus.len() {
                let mask = self.cpus[i].l2_siblings;
                // Avoid duplicate domains
                let already = self
                    .sched_domains
                    .iter()
                    .any(|d| d.level == SchedDomainLevel::Smt && d.cpu_mask == mask);
                if !already {
                    self.sched_domains.push(SchedDomain {
                        level: SchedDomainLevel::Smt,
                        cpu_mask: mask,
                        num_cpus: mask.count_ones(),
                        balance_interval_ms: 1, // balance very frequently within SMT
                        imbalance_threshold: 1,
                    });
                }
            }
        }

        // Package domain (cores sharing L3)
        for i in 0..self.cpus.len() {
            let mask = self.cpus[i].l3_siblings;
            let already = self
                .sched_domains
                .iter()
                .any(|d| d.level == SchedDomainLevel::Package && d.cpu_mask == mask);
            if !already {
                self.sched_domains.push(SchedDomain {
                    level: SchedDomainLevel::Package,
                    cpu_mask: mask,
                    num_cpus: mask.count_ones(),
                    balance_interval_ms: 4, // balance within package fairly often
                    imbalance_threshold: 2,
                });
            }
        }

        // NUMA domain (per-node)
        for node in &self.numa_nodes {
            if node.cpu_mask != 0 {
                self.sched_domains.push(SchedDomain {
                    level: SchedDomainLevel::Numa,
                    cpu_mask: node.cpu_mask,
                    num_cpus: node.cpu_mask.count_ones(),
                    balance_interval_ms: 32, // NUMA rebalance less frequently
                    imbalance_threshold: 4,
                });
            }
        }

        // System domain (all CPUs)
        let mut all_mask: u64 = 0;
        for cpu in &self.cpus {
            all_mask |= 1u64 << cpu.cpu_id;
        }
        self.sched_domains.push(SchedDomain {
            level: SchedDomainLevel::System,
            cpu_mask: all_mask,
            num_cpus: all_mask.count_ones(),
            balance_interval_ms: 64, // system-wide balance is expensive
            imbalance_threshold: 8,
        });
    }

    // ------- Query API -------

    /// Get topology info for a specific CPU
    fn get_cpu_topology(&self, cpu: u32) -> Option<&CpuTopology> {
        self.cpus.get(cpu as usize)
    }

    /// Find the closest CPU to a given one (same L2 > same package > same node)
    fn closest_cpu(&self, cpu: u32) -> Option<u32> {
        let topo = self.cpus.get(cpu as usize)?;

        // Prefer L2 sibling
        let l2_sibs = topo.l2_siblings & !(1u64 << cpu);
        if l2_sibs != 0 {
            // Find lowest-numbered sibling
            for i in 0..crate::smp::MAX_CPUS {
                if l2_sibs & (1u64 << i) != 0 {
                    return Some(i as u32);
                }
            }
        }

        // Prefer L3 sibling (same package)
        let l3_sibs = topo.l3_siblings & !(1u64 << cpu);
        if l3_sibs != 0 {
            for i in 0..crate::smp::MAX_CPUS {
                if l3_sibs & (1u64 << i) != 0 {
                    return Some(i as u32);
                }
            }
        }

        // Any other online CPU
        for c in &self.cpus {
            if c.cpu_id != cpu && c.online {
                return Some(c.cpu_id);
            }
        }

        None
    }

    /// Check if two CPUs share the same L2 cache
    fn share_l2(&self, cpu_a: u32, cpu_b: u32) -> bool {
        if let Some(topo) = self.cpus.get(cpu_a as usize) {
            topo.l2_siblings & (1u64 << cpu_b) != 0
        } else {
            false
        }
    }

    /// Check if two CPUs are in the same package
    fn same_package(&self, cpu_a: u32, cpu_b: u32) -> bool {
        if let (Some(a), Some(b)) = (self.cpus.get(cpu_a as usize), self.cpus.get(cpu_b as usize)) {
            a.package_id == b.package_id
        } else {
            false
        }
    }

    /// Check if two CPUs are in the same NUMA node
    fn same_numa_node(&self, cpu_a: u32, cpu_b: u32) -> bool {
        if let (Some(a), Some(b)) = (self.cpus.get(cpu_a as usize), self.cpus.get(cpu_b as usize)) {
            a.numa_node == b.numa_node
        } else {
            false
        }
    }

    /// Get NUMA distance between two nodes
    fn numa_distance(&self, node_a: u32, node_b: u32) -> u32 {
        if node_a == node_b {
            return 10;
        } // same node
        if let Some(node) = self.numa_nodes.get(node_a as usize) {
            if let Some(&dist) = node.distances.get(node_b as usize) {
                return dist;
            }
        }
        255 // unknown / very far
    }

    /// Get scheduling domains for a CPU (from tightest to loosest)
    fn get_sched_domains(&self, cpu: u32) -> Vec<&SchedDomain> {
        self.sched_domains
            .iter()
            .filter(|d| d.cpu_mask & (1u64 << cpu) != 0)
            .collect()
    }

    /// Format topology as a human-readable string
    fn format_topology(&self) -> String {
        let mut s = format!("CPU Topology:\n");
        s.push_str(&format!(
            "  Packages: {}  Cores/pkg: {}  Threads/core: {}  Total CPUs: {}\n",
            self.num_packages, self.cores_per_package, self.threads_per_core, self.num_cpus
        ));
        s.push_str(&format!(
            "  SMT: {}  NUMA: {}\n",
            if self.smt_detected { "yes" } else { "no" },
            if self.numa_detected { "yes" } else { "no" }
        ));

        s.push_str("\nPer-CPU:\n");
        s.push_str("CPU  APIC  Pkg  Core  Thread  NUMA  SMT  L1      L2      L3\n");
        for cpu in &self.cpus {
            s.push_str(&format!(
                "{:>3}  {:>4}  {:>3}  {:>4}  {:>6}  {:>4}  {:>3}  {:>5}K  {:>5}K  {:>5}K\n",
                cpu.cpu_id,
                cpu.apic_id,
                cpu.package_id,
                cpu.core_id,
                cpu.thread_id,
                cpu.numa_node,
                if cpu.is_smt { "yes" } else { "no" },
                cpu.l1_cache.size / 1024,
                cpu.l2_cache.size / 1024,
                cpu.l3_cache.size / 1024,
            ));
        }

        if !self.numa_nodes.is_empty() {
            s.push_str("\nNUMA Nodes:\n");
            for node in &self.numa_nodes {
                s.push_str(&format!(
                    "  Node {}: CPUs={:#x} mem={:#x}-{:#x} ({} MB)\n",
                    node.id,
                    node.cpu_mask,
                    node.mem_start,
                    node.mem_end,
                    node.mem_total / (1024 * 1024)
                ));
            }
        }

        if !self.sched_domains.is_empty() {
            s.push_str("\nScheduling Domains:\n");
            for sd in &self.sched_domains {
                s.push_str(&format!(
                    "  {:?}: CPUs={:#x} ncpus={} balance={}ms imbalance={}\n",
                    sd.level,
                    sd.cpu_mask,
                    sd.num_cpus,
                    sd.balance_interval_ms,
                    sd.imbalance_threshold
                ));
            }
        }

        s
    }
}

// ---------------------------------------------------------------------------
// Global subsystem and public API
// ---------------------------------------------------------------------------

static TOPOLOGY: Mutex<TopologySubsystem> = Mutex::new(TopologySubsystem::new());

/// Get the topology of a specific CPU
pub fn get_cpu_topology(cpu: u32) -> Option<CpuTopology> {
    TOPOLOGY.lock().get_cpu_topology(cpu).cloned()
}

/// Find the closest CPU to a given one
pub fn closest_cpu(cpu: u32) -> Option<u32> {
    TOPOLOGY.lock().closest_cpu(cpu)
}

/// Check if two CPUs share L2 cache
pub fn share_l2(cpu_a: u32, cpu_b: u32) -> bool {
    TOPOLOGY.lock().share_l2(cpu_a, cpu_b)
}

/// Check if two CPUs are in the same package
pub fn same_package(cpu_a: u32, cpu_b: u32) -> bool {
    TOPOLOGY.lock().same_package(cpu_a, cpu_b)
}

/// Check if two CPUs are in the same NUMA node
pub fn same_numa_node(cpu_a: u32, cpu_b: u32) -> bool {
    TOPOLOGY.lock().same_numa_node(cpu_a, cpu_b)
}

/// Get NUMA distance between two nodes
pub fn numa_distance(node_a: u32, node_b: u32) -> u32 {
    TOPOLOGY.lock().numa_distance(node_a, node_b)
}

/// Get number of packages
pub fn num_packages() -> u32 {
    TOPOLOGY.lock().num_packages
}

/// Get cores per package
pub fn cores_per_package() -> u32 {
    TOPOLOGY.lock().cores_per_package
}

/// Get threads per core
pub fn threads_per_core() -> u32 {
    TOPOLOGY.lock().threads_per_core
}

/// Check if SMT is detected
pub fn smt_detected() -> bool {
    TOPOLOGY.lock().smt_detected
}

/// Get the formatted topology string
pub fn format_topology() -> String {
    TOPOLOGY.lock().format_topology()
}

/// Get NUMA node info
pub fn get_numa_nodes() -> Vec<NumaNode> {
    TOPOLOGY.lock().numa_nodes.clone()
}

/// Get scheduling domains for a CPU
pub fn get_sched_domains(cpu: u32) -> Vec<SchedDomain> {
    TOPOLOGY
        .lock()
        .get_sched_domains(cpu)
        .into_iter()
        .cloned()
        .collect()
}

pub fn init() {
    let mut topo = TOPOLOGY.lock();
    topo.detect_topology();

    let npkg = topo.num_packages;
    let ncores = topo.cores_per_package;
    let nthreads = topo.threads_per_core;
    let ncpus = topo.num_cpus;
    let smt = topo.smt_detected;
    let numa_nodes = topo.numa_nodes.len();
    let sched_doms = topo.sched_domains.len();

    drop(topo);

    crate::serial_println!(
        "  [topology] CPU topology: {} pkg x {} cores x {} threads = {} CPUs (SMT={}, NUMA nodes={}, sched domains={})",
        npkg, ncores, nthreads, ncpus, smt, numa_nodes, sched_doms);
}
