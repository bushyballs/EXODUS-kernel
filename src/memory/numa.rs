use crate::serial_println;
/// numa --- NUMA topology for Genesis (no-heap, no-float, no-panic)
///
/// Implements full NUMA node registry with:
///   - Per-node page-frame ranges (base_pfn / end_pfn)
///   - CPU-to-node affinity via per-node cpu_mask bitmask (up to 16 CPUs)
///   - SLIT distance matrix between nodes
///   - Free-page counters per node
///   - Single-socket boot: one node covering all physical memory
///
/// All data is fixed-size, Copy, and lives in static Mutex storage.
/// No Vec, Box, String, or alloc. No float casts. No unwrap/expect/panic.
/// Counters use saturating arithmetic; sequence numbers use wrapping_add.
///
/// Inspired by Linux NUMA topology (mm/numa.c, include/linux/nodemask.h).
/// All code is original.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of NUMA nodes supported
pub const MAX_NUMA_NODES: usize = 8;

/// Maximum CPUs per NUMA node (drives the cpu_mask width)
pub const MAX_CPUS_PER_NODE: usize = 16;

/// SLIT local-access distance (same node)
pub const NUMA_LOCAL_DIST: u8 = 10;

/// SLIT remote-access distance (different node, same system)
pub const NUMA_REMOTE_DIST: u8 = 20;

/// Sentinel distance meaning "unreachable / not populated"
const NUMA_MAX_DIST: u8 = 255;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Descriptor for a single NUMA node.
///
/// Stored in a fixed-size array; inactive entries have `active == false`.
#[derive(Debug, Clone, Copy)]
pub struct NumaNode {
    /// Node identifier (0-based)
    pub id: u32,
    /// First page-frame number belonging to this node
    pub base_pfn: u64,
    /// One-past-last page-frame number belonging to this node
    pub end_pfn: u64,
    /// Bitmask of CPUs on this node.
    /// Bit `n` of `cpu_mask[n / 8]` corresponds to CPU `n`.
    /// Supports up to `MAX_CPUS_PER_NODE` == 16 CPUs (2 bytes).
    pub cpu_mask: [u8; 2],
    /// Total pages on this node (== end_pfn - base_pfn)
    pub total_pages: u64,
    /// Current free pages on this node
    pub free_pages: u64,
    /// Whether this node slot is populated
    pub active: bool,
}

impl NumaNode {
    /// Construct an inactive, zeroed node.
    pub const fn empty() -> Self {
        NumaNode {
            id: 0,
            base_pfn: 0,
            end_pfn: 0,
            cpu_mask: [0u8; 2],
            total_pages: 0,
            free_pages: 0,
            active: false,
        }
    }
}

// Copy is required for the static Mutex<[NumaNode; N]> pattern.
// NumaNode holds only primitive types so this is safe.

/// SLIT distance table between nodes.
///
/// `distances[a][b]` is the access-latency distance from node `a` to node `b`.
/// Diagonal entries should be `NUMA_LOCAL_DIST`. Off-diagonal defaults are
/// `NUMA_REMOTE_DIST` once the relevant nodes are registered.
#[derive(Debug, Clone, Copy)]
pub struct NumaDistances {
    pub distances: [[u8; MAX_NUMA_NODES]; MAX_NUMA_NODES],
}

impl NumaDistances {
    /// Return a zeroed-out distance table with diagonal == `NUMA_LOCAL_DIST`
    /// and off-diagonal == `NUMA_REMOTE_DIST` (populated lazily on register).
    pub const fn default() -> Self {
        // We cannot loop in a const context with mutable indexing in stable
        // Rust targeting no_std, so we initialise to NUMA_REMOTE_DIST
        // everywhere; the diagonal is fixed up in numa_register_node().
        NumaDistances {
            distances: [[NUMA_REMOTE_DIST; MAX_NUMA_NODES]; MAX_NUMA_NODES],
        }
    }
}

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

/// Registry of all NUMA nodes.
static NUMA_NODES: Mutex<[NumaNode; MAX_NUMA_NODES]> = {
    const EMPTY: NumaNode = NumaNode::empty();
    Mutex::new([EMPTY; MAX_NUMA_NODES])
};

/// SLIT distance matrix.
static NUMA_DIST: Mutex<NumaDistances> = Mutex::new(NumaDistances::default());

/// Number of active nodes registered so far.
pub static NUM_NODES: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a NUMA node covering `[base_pfn, end_pfn)`.
///
/// Assigns the next available node id and marks the node active.
/// Sets `total_pages = end_pfn - base_pfn` and `free_pages = total_pages`.
/// Also sets the self-distance in NUMA_DIST to `NUMA_LOCAL_DIST`.
///
/// Returns the assigned node id on success, or `None` if the node table
/// is already full (`MAX_NUMA_NODES` reached) or the range is empty.
pub fn numa_register_node(base_pfn: u64, end_pfn: u64) -> Option<u32> {
    if end_pfn <= base_pfn {
        return None;
    }

    // Claim the next slot atomically to determine the node id.
    // We use compare-exchange so we don't exceed MAX_NUMA_NODES.
    let mut current = NUM_NODES.load(Ordering::Relaxed);
    loop {
        if current as usize >= MAX_NUMA_NODES {
            return None;
        }
        match NUM_NODES.compare_exchange_weak(
            current,
            current.wrapping_add(1),
            Ordering::SeqCst,
            Ordering::Relaxed,
        ) {
            Ok(id) => {
                let node_id = id as usize;
                let total = end_pfn.saturating_sub(base_pfn);

                // Fill in the node descriptor.
                {
                    let mut nodes = NUMA_NODES.lock();
                    nodes[node_id] = NumaNode {
                        id: id,
                        base_pfn,
                        end_pfn,
                        cpu_mask: [0u8; 2],
                        total_pages: total,
                        free_pages: total,
                        active: true,
                    };
                }

                // Update SLIT: self-distance = NUMA_LOCAL_DIST.
                {
                    let mut dist = NUMA_DIST.lock();
                    dist.distances[node_id][node_id] = NUMA_LOCAL_DIST;
                }

                serial_println!(
                    "  [numa] node {} registered: pfn {:#x}-{:#x} ({} pages)",
                    id,
                    base_pfn,
                    end_pfn,
                    total
                );
                return Some(id);
            }
            Err(actual) => {
                current = actual;
            }
        }
    }
}

/// Set the SLIT distance from node `from` to node `to`.
///
/// Returns `true` on success, `false` if either index is out of bounds.
pub fn numa_set_distance(from: usize, to: usize, dist: u8) -> bool {
    if from >= MAX_NUMA_NODES || to >= MAX_NUMA_NODES {
        return false;
    }
    let mut d = NUMA_DIST.lock();
    d.distances[from][to] = dist;
    true
}

/// Find the node that contains page-frame `pfn`.
///
/// Iterates active nodes and returns the first whose `[base_pfn, end_pfn)`
/// range contains `pfn`. Returns `None` if no node matches.
pub fn numa_node_of_pfn(pfn: u64) -> Option<u32> {
    let nodes = NUMA_NODES.lock();
    let count = NUM_NODES.load(Ordering::Relaxed) as usize;
    for i in 0..count {
        let n = &nodes[i];
        if n.active && pfn >= n.base_pfn && pfn < n.end_pfn {
            return Some(n.id);
        }
    }
    None
}

/// Find the NUMA node for CPU `cpu_id`.
///
/// Checks bit `cpu_id` in each node's `cpu_mask`. Returns `None` if the
/// CPU is not assigned to any node or `cpu_id >= MAX_CPUS_PER_NODE`.
pub fn numa_cpu_to_node(cpu_id: u8) -> Option<u32> {
    if cpu_id as usize >= MAX_CPUS_PER_NODE {
        return None;
    }
    let byte = (cpu_id / 8) as usize;
    let bit = cpu_id % 8;
    let nodes = NUMA_NODES.lock();
    let count = NUM_NODES.load(Ordering::Relaxed) as usize;
    for i in 0..count {
        let n = &nodes[i];
        if n.active && (n.cpu_mask[byte] & (1 << bit)) != 0 {
            return Some(n.id);
        }
    }
    None
}

/// Assign CPU `cpu_id` to node `node_id` by setting the appropriate bit
/// in the node's `cpu_mask`.
///
/// Returns `true` on success, `false` if `node_id` is out of range or
/// inactive, or if `cpu_id >= MAX_CPUS_PER_NODE`.
pub fn numa_add_cpu_to_node(node_id: u32, cpu_id: u8) -> bool {
    if node_id as usize >= MAX_NUMA_NODES {
        return false;
    }
    if cpu_id as usize >= MAX_CPUS_PER_NODE {
        return false;
    }
    let byte = (cpu_id / 8) as usize;
    let bit = cpu_id % 8;
    let mut nodes = NUMA_NODES.lock();
    let n = &mut nodes[node_id as usize];
    if !n.active {
        return false;
    }
    n.cpu_mask[byte] |= 1 << bit;
    true
}

/// Update the free-page count for node `node_id`.
///
/// Silently does nothing if `node_id` is out of range or the node is
/// inactive.
pub fn numa_update_free(node_id: u32, free_pages: u64) {
    if node_id as usize >= MAX_NUMA_NODES {
        return;
    }
    let mut nodes = NUMA_NODES.lock();
    let n = &mut nodes[node_id as usize];
    if n.active {
        n.free_pages = free_pages;
    }
}

/// Return `(total_pages, free_pages)` for node `node_id`.
///
/// Returns `None` if the node is inactive or `node_id` is out of range.
pub fn numa_get_stats(node_id: u32) -> Option<(u64, u64)> {
    if node_id as usize >= MAX_NUMA_NODES {
        return None;
    }
    let nodes = NUMA_NODES.lock();
    let n = &nodes[node_id as usize];
    if n.active {
        Some((n.total_pages, n.free_pages))
    } else {
        None
    }
}

/// Return the preferred allocation node for the current context.
///
/// Simplified implementation: always returns node 0. A future scheduler-
/// aware version would read the current CPU's NUMA affinity.
#[inline]
pub fn numa_preferred_node() -> u32 {
    0
}

/// Initialize the NUMA subsystem.
///
/// Registers a single node (node 0) covering the full physical memory range
/// as tracked by the frame allocator, assigns CPU 0 to it, and prints a
/// boot message.
pub fn init() {
    // Derive the physical address range from the frame allocator constants.
    let total_bytes = crate::memory::frame_allocator::MAX_MEMORY as u64;
    let frame_size = crate::memory::frame_allocator::FRAME_SIZE as u64;

    // Guard against zero frame size (should never happen, but follow rules).
    if frame_size == 0 {
        serial_println!("  [numa] ERROR: FRAME_SIZE is zero, cannot initialize");
        return;
    }

    let total_pfn = total_bytes / frame_size;

    if let Some(node_id) = numa_register_node(0, total_pfn) {
        numa_add_cpu_to_node(node_id, 0);
    }

    serial_println!("  [numa] topology initialized (1 node)");
}
