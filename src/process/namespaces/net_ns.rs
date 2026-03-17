use crate::serial_println;
/// Network Namespace implementation for Genesis
///
/// Provides Linux-compatible network namespace isolation. Each namespace
/// gets its own isolated loopback interface (127.0.0.1/8). The host
/// network namespace is always ID 0.
///
/// veth pair creation is logged as a stub — full packet routing is out
/// of scope for the namespace layer.
///
/// Rules enforced:
/// - No heap (no Vec/Box/String/alloc::*)
/// - No float casts
/// - No panics (no unwrap/expect/panic!)
/// - Saturating arithmetic for counters
/// - All structs are Copy with const fn empty()
use crate::sync::Mutex;

/// Maximum number of concurrent network namespaces.
pub const MAX_NET_NAMESPACES: usize = 16;

// ---------------------------------------------------------------------------
// NetNamespace
// ---------------------------------------------------------------------------

/// Per-namespace network configuration.
#[derive(Clone, Copy, Debug)]
pub struct NetNamespace {
    /// Unique namespace ID (0 = host/init namespace).
    pub id: u32,
    /// Whether this slot is occupied.
    pub active: bool,
    /// Loopback IPv4 address (e.g. [127, 0, 0, 1]).
    pub lo_ip: [u8; 4],
    /// Loopback subnet mask (e.g. [255, 0, 0, 0] for /8).
    pub lo_mask: [u8; 4],
}

impl NetNamespace {
    pub const fn empty() -> Self {
        NetNamespace {
            id: 0,
            active: false,
            lo_ip: [0u8; 4],
            lo_mask: [0u8; 4],
        }
    }
}

// ---------------------------------------------------------------------------
// Per-process namespace assignment
// ---------------------------------------------------------------------------

/// Maps pid -> net_ns_id.  Default 0 (host namespace).
static PROC_NET_NS: Mutex<[u32; 1024]> = Mutex::new([0u32; 1024]);

// ---------------------------------------------------------------------------
// Global namespace table
// ---------------------------------------------------------------------------

struct NetNsTable {
    slots: [NetNamespace; MAX_NET_NAMESPACES],
    next_ns_id: u32,
}

impl NetNsTable {
    const fn new() -> Self {
        const EMPTY: NetNamespace = NetNamespace::empty();
        NetNsTable {
            slots: [EMPTY; MAX_NET_NAMESPACES],
            next_ns_id: 1,
        }
    }
}

static NET_NAMESPACES: Mutex<NetNsTable> = Mutex::new(NetNsTable::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_slot(tbl: &NetNsTable, ns_id: u32) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if slot.active && slot.id == ns_id {
            return Some(i);
        }
    }
    None
}

fn find_free_slot(tbl: &NetNsTable) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if !slot.active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the network namespace subsystem.
///
/// Creates the host (init) namespace with id=0 and loopback 127.0.0.1/8.
pub fn init() {
    let mut tbl = NET_NAMESPACES.lock();
    tbl.slots[0] = NetNamespace {
        id: 0,
        active: true,
        lo_ip: [127, 0, 0, 1],
        lo_mask: [255, 0, 0, 0],
    };
    tbl.next_ns_id = 1;
    serial_println!("  NetNs: host namespace initialized (id=0, lo=127.0.0.1/8)");
}

/// Create a new, isolated network namespace.
///
/// The new namespace is initialized with only loopback (127.0.0.1/8) and
/// no external interfaces. Returns the new namespace ID on success, or None
/// if the table is full.
pub fn net_ns_create() -> Option<u32> {
    let mut tbl = NET_NAMESPACES.lock();
    let free_idx = find_free_slot(&tbl)?;

    let new_id = tbl.next_ns_id;
    tbl.slots[free_idx] = NetNamespace {
        id: new_id,
        active: true,
        lo_ip: [127, 0, 0, 1],
        lo_mask: [255, 0, 0, 0],
    };
    tbl.next_ns_id = tbl.next_ns_id.saturating_add(1);

    serial_println!("  NetNs: created ns_id={}", new_id);
    Some(new_id)
}

/// Destroy a network namespace.
///
/// Never destroys the host namespace (id=0).
pub fn net_ns_destroy(ns_id: u32) {
    if ns_id == 0 {
        return;
    }

    {
        let mut tbl = NET_NAMESPACES.lock();
        if let Some(idx) = find_slot(&tbl, ns_id) {
            tbl.slots[idx] = NetNamespace::empty();
        }
    }

    // Reassign any processes that were in this namespace to the host.
    {
        let mut map = PROC_NET_NS.lock();
        for entry in map.iter_mut() {
            if *entry == ns_id {
                *entry = 0;
            }
        }
    }

    serial_println!("  NetNs: destroyed ns_id={}", ns_id);
}

/// Return the host (init) network namespace ID, which is always 0.
pub fn net_ns_get_default() -> u32 {
    0
}

/// Assign a process to a network namespace.
///
/// Returns false if the namespace does not exist or the PID is out of range.
pub fn net_ns_assign_process(ns_id: u32, pid: u32) -> bool {
    // Validate namespace exists.
    {
        let tbl = NET_NAMESPACES.lock();
        if find_slot(&tbl, ns_id).is_none() {
            return false;
        }
    }

    let mut map = PROC_NET_NS.lock();
    let idx = pid as usize;
    if idx >= map.len() {
        return false;
    }
    map[idx] = ns_id;
    true
}

/// Get the network namespace ID for a process.
///
/// Returns the host namespace (0) if the PID is out of range.
pub fn net_ns_get_for_process(pid: u32) -> u32 {
    let map = PROC_NET_NS.lock();
    let idx = pid as usize;
    if idx >= map.len() {
        return 0;
    }
    map[idx]
}

/// Return true if `ns_id` is the host network namespace.
pub fn net_ns_is_host(ns_id: u32) -> bool {
    ns_id == 0
}

/// Stub: log the intent to create a veth pair bridging two namespaces.
///
/// A veth pair is a virtual Ethernet link — one end lives in `ns1`,
/// the other in `ns2`. Full routing implementation is left to the
/// network driver layer. This function records the creation intent and
/// returns true to indicate success.
pub fn net_ns_create_veth(ns1: u32, ns2: u32, name1: &[u8], name2: &[u8]) -> bool {
    // Validate both namespaces exist.
    let tbl = NET_NAMESPACES.lock();
    if find_slot(&tbl, ns1).is_none() || find_slot(&tbl, ns2).is_none() {
        return false;
    }
    drop(tbl);

    // Log creation intent — actual packet routing not yet implemented.
    // name1 and name2 are interface names (e.g. b"veth0", b"veth1").
    let n1_len = name1.len().min(15);
    let n2_len = name2.len().min(15);
    let mut n1_buf = [0u8; 16];
    let mut n2_buf = [0u8; 16];
    n1_buf[..n1_len].copy_from_slice(&name1[..n1_len]);
    n2_buf[..n2_len].copy_from_slice(&name2[..n2_len]);

    serial_println!(
        "  NetNs: veth pair intent ns1={} iface={:?} <-> ns2={} iface={:?}",
        ns1,
        &n1_buf[..n1_len],
        ns2,
        &n2_buf[..n2_len]
    );

    true
}
