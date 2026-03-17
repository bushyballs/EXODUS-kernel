use crate::serial_println;
use crate::smp::MAX_CPUS;
use crate::sync::Mutex;
/// NUMA-aware scheduling for Genesis
///
/// Parses the ACPI SRAT (System Resource Affinity Table) and SLIT (System
/// Locality Information Table) to build a NUMA topology map, then provides
/// placement hints to the CFS scheduler so that tasks preferentially run
/// on CPUs that are local to their memory allocations.
///
/// Design:
///   - Up to 8 NUMA nodes; up to 64 CPUs per system (same limit as smp.rs).
///   - Topology is populated once at boot from the SRAT; thereafter read-only
///     from the scheduler hot path (no lock needed on NUM_NODES / the arrays).
///   - The SLIT-derived distance table is a flat 8×8 array of u8.  Local
///     distance = 10, remote = 20 (ACPI defaults); actual values from SLIT
///     override these when present.
///   - Per-task NUMA node is tracked in a separate lightweight table keyed by
///     PID (up to 1 024 processes), updated on page-fault or explicit hint.
///   - `numa_migrate_pages` is a best-effort stub that records migrations;
///     actual page-table manipulation is the MM subsystem's responsibility.
///
/// No std, no heap, no float casts, no panics.  All arithmetic is saturating
/// or wrapping as appropriate.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Capacity limits
// ---------------------------------------------------------------------------

/// Maximum NUMA nodes supported (ACPI SRAT can describe up to 256; we cap at 8).
pub const MAX_NUMA_NODES: usize = 8;

/// Maximum processes for which we track a preferred NUMA node.
const MAX_TRACKED_PIDS: usize = 1024;

// ---------------------------------------------------------------------------
// ACPI table signature constants
// ---------------------------------------------------------------------------

const SRAT_SIG: [u8; 4] = *b"SRAT";
const SLIT_SIG: [u8; 4] = *b"SLIT";

// SRAT sub-structure types (ACPI 6.4 §5.2.16)
const SRAT_TYPE_CPU_AFFINITY: u8 = 0; // Processor Local APIC/SAPIC Affinity
const SRAT_TYPE_MEM_AFFINITY: u8 = 1; // Memory Affinity
const SRAT_TYPE_X2APIC_AFFINITY: u8 = 2; // Processor Local x2APIC Affinity

// Affinity flags
const SRAT_FLAG_ENABLED: u32 = 1;

// ---------------------------------------------------------------------------
// NUMA node descriptor
// ---------------------------------------------------------------------------

/// Describes one NUMA node parsed from the ACPI SRAT table.
#[derive(Clone, Copy)]
pub struct NumaNode {
    /// Proximity domain / node ID (0-based)
    pub id: u32,
    /// Bitmap of logical CPU IDs that belong to this node (bit N → CPU N)
    pub cpu_mask: u64,
    /// Physical memory range start (inclusive)
    pub mem_start: u64,
    /// Physical memory range end (exclusive)
    pub mem_end: u64,
    /// Whether this node is currently online
    pub online: bool,
}

impl NumaNode {
    const fn empty() -> Self {
        NumaNode {
            id: 0,
            cpu_mask: 0,
            mem_start: 0,
            mem_end: 0,
            online: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static topology tables
// ---------------------------------------------------------------------------

const EMPTY_NODE: Option<NumaNode> = None;

/// The NUMA node array; populated once during `init_from_srat`.
static NUMA_NODES: Mutex<[Option<NumaNode>; MAX_NUMA_NODES]> =
    Mutex::new([EMPTY_NODE; MAX_NUMA_NODES]);

/// Number of online NUMA nodes discovered from SRAT.
static NUM_NODES: AtomicU32 = AtomicU32::new(1);

/// NUMA distance matrix — distance[from * MAX_NUMA_NODES + to].
/// Default: 10 for local, 20 for remote (ACPI SLIT defaults).
static NUMA_DISTANCE: Mutex<[u8; MAX_NUMA_NODES * MAX_NUMA_NODES]> =
    Mutex::new([0u8; MAX_NUMA_NODES * MAX_NUMA_NODES]);

// ---------------------------------------------------------------------------
// Per-task NUMA affinity table
// ---------------------------------------------------------------------------

/// Lightweight per-PID NUMA node preference.
/// Index is PID % MAX_TRACKED_PIDS; collisions are resolved by storing the
/// PID alongside the node so stale entries are detected.
#[derive(Clone, Copy)]
struct TaskNumaEntry {
    pid: u32,
    node: u32,
}

impl TaskNumaEntry {
    const fn empty() -> Self {
        TaskNumaEntry {
            pid: u32::MAX,
            node: 0,
        }
    }
}

const EMPTY_TASK_ENTRY: TaskNumaEntry = TaskNumaEntry::empty();
static TASK_NUMA: Mutex<[TaskNumaEntry; MAX_TRACKED_PIDS]> =
    Mutex::new([EMPTY_TASK_ENTRY; MAX_TRACKED_PIDS]);

// ---------------------------------------------------------------------------
// NUMA statistics
// ---------------------------------------------------------------------------

/// Per-system NUMA allocation and migration counters.
pub struct NumaStats {
    /// Allocations that were local to the running CPU's node
    pub local_allocs: u64,
    /// Allocations that were remote (cross-node)
    pub remote_allocs: u64,
    /// Pages migrated to a local node
    pub migrations: u64,
}

// Internal atomic backing store
static STAT_LOCAL_ALLOCS: AtomicU64 = AtomicU64::new(0);
static STAT_REMOTE_ALLOCS: AtomicU64 = AtomicU64::new(0);
static STAT_MIGRATIONS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the NUMA subsystem.
///
/// Sets up a single-node topology (node 0, all CPUs, all memory) as a
/// reasonable default for systems without SRAT, or as the starting state
/// before `init_from_srat` is called.
pub fn init() {
    // Populate distance matrix with defaults (10 = local, 20 = remote).
    {
        let mut dist = NUMA_DISTANCE.lock();
        for from in 0..MAX_NUMA_NODES {
            for to in 0..MAX_NUMA_NODES {
                let idx = from * MAX_NUMA_NODES + to;
                dist[idx] = if from == to { 10 } else { 20 };
            }
        }
    }

    // Single-node fallback: node 0 owns all CPUs and all addressable memory.
    {
        let mut nodes = NUMA_NODES.lock();
        nodes[0] = Some(NumaNode {
            id: 0,
            cpu_mask: u64::MAX, // all CPUs on node 0
            mem_start: 0,
            mem_end: u64::MAX,
            online: true,
        });
    }

    NUM_NODES.store(1, Ordering::Relaxed);
    serial_println!("  Process: NUMA subsystem initialized (1 node default)");
}

/// Parse NUMA topology from the ACPI SRAT table.
///
/// `srat_ptr` is the physical address of the SRAT table header.  The caller
/// is responsible for identity-mapping the ACPI tables before calling this.
///
/// The function reads sub-structures sequentially; any malformed entry is
/// silently skipped to prevent a panic on hardware with quirky firmware.
pub fn init_from_srat(srat_ptr: u64) {
    if srat_ptr == 0 {
        return;
    }

    // SAFETY: The caller guarantees srat_ptr is a valid, identity-mapped
    // physical address pointing at an ACPI SRAT table.  We read only within
    // the bounds reported by the table's own length field.
    let hdr = unsafe { core::slice::from_raw_parts(srat_ptr as *const u8, 8) };

    // Verify signature
    if hdr[0..4] != SRAT_SIG {
        serial_println!("  Process: SRAT signature mismatch — skipping NUMA init");
        return;
    }

    let table_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if table_len < 48 {
        return;
    }

    let table = unsafe { core::slice::from_raw_parts(srat_ptr as *const u8, table_len) };

    let mut nodes = NUMA_NODES.lock();
    // Clear fallback node; we'll rebuild from SRAT.
    for slot in nodes.iter_mut() {
        *slot = None;
    }

    let mut node_count: u32 = 0;

    // SRAT header is 48 bytes; sub-structures start at offset 48.
    let mut offset = 48usize;
    while offset.saturating_add(2) <= table_len {
        if offset + 1 >= table_len {
            break;
        }
        let sub_type = table[offset];
        let sub_len = table[offset + 1] as usize;
        if sub_len < 2 || offset.saturating_add(sub_len) > table_len {
            break; // malformed
        }

        let sub = &table[offset..offset.saturating_add(sub_len)];

        match sub_type {
            SRAT_TYPE_CPU_AFFINITY if sub_len >= 16 => {
                // ACPI 6.4 §5.2.16.1
                let proximity_lo = sub[2] as u32;
                let apic_id = sub[3];
                let flags = u32::from_le_bytes([sub[4], sub[5], sub[6], sub[7]]);
                let proximity_hi = u32::from_le_bytes([sub[8], sub[9], sub[10], sub[11]]);
                let proximity = proximity_lo | (proximity_hi << 8);

                if (flags & SRAT_FLAG_ENABLED) == 0 {
                    offset = offset.saturating_add(sub_len);
                    continue;
                }

                let node_id = (proximity as usize).min(MAX_NUMA_NODES - 1);
                let cpu_bit = if apic_id < 64 { 1u64 << apic_id } else { 0 };

                ensure_node(&mut nodes, node_id, &mut node_count);
                if let Some(ref mut node) = nodes[node_id] {
                    node.cpu_mask |= cpu_bit;
                }
            }

            SRAT_TYPE_X2APIC_AFFINITY if sub_len >= 24 => {
                // ACPI 6.4 §5.2.16.3
                let flags = u32::from_le_bytes([sub[4], sub[5], sub[6], sub[7]]);
                let proximity = u32::from_le_bytes([sub[8], sub[9], sub[10], sub[11]]);
                let x2apic_id = u32::from_le_bytes([sub[12], sub[13], sub[14], sub[15]]);

                if (flags & SRAT_FLAG_ENABLED) == 0 {
                    offset = offset.saturating_add(sub_len);
                    continue;
                }

                let node_id = (proximity as usize).min(MAX_NUMA_NODES - 1);
                let cpu_bit = if x2apic_id < 64 { 1u64 << x2apic_id } else { 0 };

                ensure_node(&mut nodes, node_id, &mut node_count);
                if let Some(ref mut node) = nodes[node_id] {
                    node.cpu_mask |= cpu_bit;
                }
            }

            SRAT_TYPE_MEM_AFFINITY if sub_len >= 40 => {
                // ACPI 6.4 §5.2.16.2
                let proximity = u32::from_le_bytes([sub[2], sub[3], sub[4], sub[5]]);
                let base_lo = u32::from_le_bytes([sub[8], sub[9], sub[10], sub[11]]);
                let base_hi = u32::from_le_bytes([sub[12], sub[13], sub[14], sub[15]]);
                let len_lo = u32::from_le_bytes([sub[16], sub[17], sub[18], sub[19]]);
                let len_hi = u32::from_le_bytes([sub[20], sub[21], sub[22], sub[23]]);
                let flags = u32::from_le_bytes([sub[28], sub[29], sub[30], sub[31]]);

                if (flags & SRAT_FLAG_ENABLED) == 0 {
                    offset = offset.saturating_add(sub_len);
                    continue;
                }

                let base = ((base_hi as u64) << 32) | (base_lo as u64);
                let size = ((len_hi as u64) << 32) | (len_lo as u64);
                let end = base.saturating_add(size);

                let node_id = (proximity as usize).min(MAX_NUMA_NODES - 1);
                ensure_node(&mut nodes, node_id, &mut node_count);
                if let Some(ref mut node) = nodes[node_id] {
                    // Extend memory range to cover this region.
                    if node.mem_start == 0 && node.mem_end == 0 {
                        node.mem_start = base;
                        node.mem_end = end;
                    } else {
                        if base < node.mem_start {
                            node.mem_start = base;
                        }
                        if end > node.mem_end {
                            node.mem_end = end;
                        }
                    }
                }
            }

            _ => {}
        }

        offset = offset.saturating_add(sub_len);
    }

    // Ensure at least node 0 exists as a fallback.
    if node_count == 0 {
        nodes[0] = Some(NumaNode {
            id: 0,
            cpu_mask: u64::MAX,
            mem_start: 0,
            mem_end: u64::MAX,
            online: true,
        });
        node_count = 1;
    }

    NUM_NODES.store(node_count, Ordering::Relaxed);
    serial_println!("  Process: NUMA: {} node(s) parsed from SRAT", node_count);
}

/// Ensure a node slot exists for `node_id`; create it if it doesn't yet.
fn ensure_node(
    nodes: &mut [Option<NumaNode>; MAX_NUMA_NODES],
    node_id: usize,
    node_count: &mut u32,
) {
    if node_id >= MAX_NUMA_NODES {
        return;
    }
    if nodes[node_id].is_none() {
        nodes[node_id] = Some(NumaNode {
            id: node_id as u32,
            cpu_mask: 0,
            mem_start: 0,
            mem_end: 0,
            online: true,
        });
        *node_count = node_count.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// SLIT — NUMA distance table
// ---------------------------------------------------------------------------

/// Populate the NUMA distance matrix from the ACPI SLIT table.
///
/// Should be called after `init_from_srat`; `slit_ptr` is the physical
/// address of the SLIT table header.  Silently ignored if 0 or malformed.
pub fn init_from_slit(slit_ptr: u64) {
    if slit_ptr == 0 {
        return;
    }

    let hdr = unsafe { core::slice::from_raw_parts(slit_ptr as *const u8, 8) };
    if hdr[0..4] != SLIT_SIG {
        return;
    }

    let table_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if table_len < 44 {
        return;
    }

    let table = unsafe { core::slice::from_raw_parts(slit_ptr as *const u8, table_len) };

    // SLIT header: bytes 36..44 hold the LocalityCount (u64 LE)
    let locality_count = u64::from_le_bytes([
        table[36], table[37], table[38], table[39], table[40], table[41], table[42], table[43],
    ]) as usize;
    let entries = locality_count.min(MAX_NUMA_NODES);

    // Matrix starts at offset 44
    let matrix_start = 44usize;
    let matrix_end = matrix_start.saturating_add(locality_count.saturating_mul(locality_count));
    if matrix_end > table_len {
        return;
    }

    let mut dist = NUMA_DISTANCE.lock();
    for from in 0..entries {
        for to in 0..entries {
            let src_idx = matrix_start
                .saturating_add(from.saturating_mul(locality_count))
                .saturating_add(to);
            if src_idx >= table_len {
                break;
            }
            let dst_idx = from * MAX_NUMA_NODES + to;
            dist[dst_idx] = table[src_idx];
        }
    }

    serial_println!(
        "  Process: NUMA: SLIT distance matrix loaded ({} nodes)",
        entries
    );
}

// ---------------------------------------------------------------------------
// CPU / memory → node lookup
// ---------------------------------------------------------------------------

/// Return the NUMA node ID for a logical CPU.
///
/// Returns 0 if the CPU is not found in any node's cpu_mask (safe fallback).
pub fn cpu_to_node(cpu_id: u32) -> u32 {
    if cpu_id >= MAX_CPUS as u32 {
        return 0;
    }
    let bit = 1u64 << cpu_id;
    let nodes = NUMA_NODES.lock();
    for slot in nodes.iter().flatten() {
        if slot.online && (slot.cpu_mask & bit) != 0 {
            return slot.id;
        }
    }
    0
}

/// Return the NUMA node ID that owns `phys_addr`.
///
/// Returns 0 if the address is not covered by any node's range.
pub fn addr_to_node(phys_addr: u64) -> u32 {
    let nodes = NUMA_NODES.lock();
    for slot in nodes.iter().flatten() {
        if slot.online && phys_addr >= slot.mem_start && phys_addr < slot.mem_end {
            return slot.id;
        }
    }
    0
}

/// Return the NUMA distance between two nodes.
///
/// Local = 10, remote = 20 (ACPI SLIT defaults), or actual values if SLIT
/// was parsed.  Returns 255 (maximum penalty) if either node ID is out of range.
pub fn numa_distance(from: u32, to: u32) -> u8 {
    let f = from as usize;
    let t = to as usize;
    if f >= MAX_NUMA_NODES || t >= MAX_NUMA_NODES {
        return 255;
    }
    let dist = NUMA_DISTANCE.lock();
    dist[f * MAX_NUMA_NODES + t]
}

// ---------------------------------------------------------------------------
// Task placement
// ---------------------------------------------------------------------------

/// Return the best CPU ID for a task whose memory is on `task_node`.
///
/// Iterates over all CPUs in `task_node`'s cpu_mask and returns the one with
/// the lowest current run-queue length.  Falls back to CPU 0 if the node
/// has no online CPUs.
pub fn preferred_cpu_for_task(task_node: u32) -> u32 {
    let nodes = NUMA_NODES.lock();
    let node_idx = task_node as usize;
    if node_idx >= MAX_NUMA_NODES {
        return 0;
    }
    let mask = match nodes[node_idx] {
        Some(ref n) if n.online => n.cpu_mask,
        _ => return 0,
    };
    drop(nodes);

    let n_cpus = NUM_NODES.load(Ordering::Relaxed) as usize;
    let search_limit = MAX_CPUS.min(n_cpus.saturating_mul(8)); // heuristic upper bound

    let mut best_cpu = u32::MAX;
    let mut best_load = u64::MAX;

    for cpu in 0..MAX_CPUS {
        if (mask & (1u64 << cpu)) == 0 {
            continue;
        }
        // Use the sched_core per-CPU run-queue length as a load proxy.
        let load = crate::process::sched_core::cpu_rq_len_pub(cpu) as u64;
        if load < best_load {
            best_load = load;
            best_cpu = cpu as u32;
        }
        if cpu >= search_limit {
            break;
        }
    }

    if best_cpu == u32::MAX {
        0
    } else {
        best_cpu
    }
}

/// Return the NUMA node that should be used for memory allocation by `cpu_id`.
///
/// This is simply the node the CPU belongs to; memory allocated here is local.
pub fn preferred_mem_node(cpu_id: u32) -> u32 {
    cpu_to_node(cpu_id)
}

// ---------------------------------------------------------------------------
// Per-task NUMA node tracking
// ---------------------------------------------------------------------------

/// Record that task `pid` has memory affinity to `node`.
/// Called by the memory allocator or page-fault handler.
pub fn set_task_node(pid: u32, node: u32) {
    let idx = (pid as usize) % MAX_TRACKED_PIDS;
    let mut table = TASK_NUMA.lock();
    table[idx] = TaskNumaEntry { pid, node };
}

/// Retrieve the last recorded NUMA node for `pid`.
/// Returns 0 if no entry exists (safe fallback to node 0).
pub fn get_task_node(pid: u32) -> u32 {
    let idx = (pid as usize) % MAX_TRACKED_PIDS;
    let table = TASK_NUMA.lock();
    let entry = &table[idx];
    if entry.pid == pid {
        entry.node
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// NUMA balancing
// ---------------------------------------------------------------------------

/// Attempt to migrate pages belonging to `pid` to the node local to its
/// current CPU.
///
/// This is a best-effort stub: the actual TLB shootdown and page-table
/// remapping must be driven by the memory management subsystem.  This
/// function records the intent and increments the migrations counter.
///
/// Returns the number of pages "migrated" (always 0 in this stub; the MM
/// subsystem should increment the real counter directly via
/// `numa_record_migration`).
pub fn numa_migrate_pages(pid: u32) -> u32 {
    let current_cpu = crate::smp::current_cpu();
    let current_node = cpu_to_node(current_cpu);
    let task_node = get_task_node(pid);

    if current_node == task_node {
        STAT_LOCAL_ALLOCS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }

    // Update the task's preferred node to its current execution node.
    set_task_node(pid, current_node);
    STAT_REMOTE_ALLOCS.fetch_add(1, Ordering::Relaxed);

    // The MM subsystem will call numa_record_migration() for each page it moves.
    0
}

/// Record that `count` pages were successfully migrated.
/// Called from the MM subsystem after physical page moves.
pub fn numa_record_migration(count: u64) {
    STAT_MIGRATIONS.fetch_add(count, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return a snapshot of NUMA allocation statistics.
pub fn numa_stats() -> NumaStats {
    NumaStats {
        local_allocs: STAT_LOCAL_ALLOCS.load(Ordering::Relaxed),
        remote_allocs: STAT_REMOTE_ALLOCS.load(Ordering::Relaxed),
        migrations: STAT_MIGRATIONS.load(Ordering::Relaxed),
    }
}

/// Record one local allocation in the statistics counters.
pub fn numa_stat_local_alloc() {
    STAT_LOCAL_ALLOCS.fetch_add(1, Ordering::Relaxed);
}

/// Record one remote allocation in the statistics counters.
pub fn numa_stat_remote_alloc() {
    STAT_REMOTE_ALLOCS.fetch_add(1, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Scheduler integration
// ---------------------------------------------------------------------------

/// Return the preferred CPU for `pid` considering NUMA locality.
///
/// This is the hook called from `sched_core::pick_next_cpu()`.
/// It reads the task's recorded NUMA node and returns the least-loaded CPU
/// in that node.  If the task has no recorded node, the current CPU is
/// returned unchanged to preserve cache locality.
pub fn sched_numa_hint(pid: u32, current_cpu: u32) -> u32 {
    let task_node = get_task_node(pid);
    let cpu_node = cpu_to_node(current_cpu);

    if task_node == cpu_node {
        // Already on a CPU in the preferred node — don't migrate.
        return current_cpu;
    }

    let dist = numa_distance(cpu_node, task_node);
    if dist <= 10 {
        // Local or equal-distance — stay put.
        return current_cpu;
    }

    // Suggest a CPU in the preferred node.
    preferred_cpu_for_task(task_node)
}
