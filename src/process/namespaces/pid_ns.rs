use crate::serial_println;
/// PID Namespace implementation for Genesis
///
/// Provides Linux-compatible PID namespace isolation. Each PID namespace
/// has its own PID numbering space. Processes in a namespace see local PIDs
/// that are translated to global PIDs via a base offset.
///
/// Rules enforced:
/// - No heap (no Vec/Box/String/alloc::*)
/// - No float casts
/// - No panics (no unwrap/expect/panic!)
/// - Saturating arithmetic for counters
/// - All structs are Copy with const fn empty()
use crate::sync::Mutex;

/// Maximum number of concurrent PID namespaces.
pub const MAX_PID_NAMESPACES: usize = 16;

/// Maximum PIDs allocatable within a single namespace.
pub const NS_MAX_PIDS: u32 = 32768;

/// PID namespace record.
///
/// All fields are plain integers so the struct is Copy and can live in a
/// static Mutex without heap allocation.
#[derive(Clone, Copy, Debug)]
pub struct PidNamespace {
    /// Unique namespace ID (0 = init/root namespace).
    pub id: u32,
    /// Parent namespace ID (same as id for the root namespace).
    pub parent_id: u32,
    /// Global-PID base. A local PID `l` maps to global PID `l + pid_base`.
    pub pid_base: u32,
    /// Upper bound on local PIDs within this namespace.
    pub max_pids: u32,
    /// Whether this slot is currently in use.
    pub active: bool,
}

impl PidNamespace {
    /// Construct a zeroed-out, inactive slot.
    pub const fn empty() -> Self {
        PidNamespace {
            id: 0,
            parent_id: 0,
            pid_base: 0,
            max_pids: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-process namespace assignment table
// ---------------------------------------------------------------------------

/// Maps (pid index) -> pid_ns_id.
/// Entry is 0 (root namespace) by default.
static PROC_NS_MAP: Mutex<[u32; 1024]> = Mutex::new([0u32; 1024]);

// ---------------------------------------------------------------------------
// Global namespace table
// ---------------------------------------------------------------------------

struct PidNsTable {
    slots: [PidNamespace; MAX_PID_NAMESPACES],
    /// Next available pid_base for a new child namespace.
    next_pid_base: u32,
    /// Counter used to assign unique namespace IDs.
    next_ns_id: u32,
}

impl PidNsTable {
    const fn new() -> Self {
        const EMPTY: PidNamespace = PidNamespace::empty();
        PidNsTable {
            slots: [EMPTY; MAX_PID_NAMESPACES],
            next_pid_base: 0,
            next_ns_id: 1,
        }
    }
}

static PID_NAMESPACES: Mutex<PidNsTable> = Mutex::new(PidNsTable::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find a slot index by namespace ID. Returns None if not found.
fn find_slot(table: &PidNsTable, ns_id: u32) -> Option<usize> {
    for (i, slot) in table.slots.iter().enumerate() {
        if slot.active && slot.id == ns_id {
            return Some(i);
        }
    }
    None
}

/// Find a free (inactive) slot index.
fn find_free_slot(table: &PidNsTable) -> Option<usize> {
    for (i, slot) in table.slots.iter().enumerate() {
        if !slot.active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the PID namespace subsystem.
///
/// Creates the root (init) namespace with id=0, pid_base=0.
pub fn init() {
    let mut tbl = PID_NAMESPACES.lock();
    // Slot 0 is always the root namespace.
    tbl.slots[0] = PidNamespace {
        id: 0,
        parent_id: 0,
        pid_base: 0,
        max_pids: NS_MAX_PIDS,
        active: true,
    };
    // IDs start from 1; the root already occupies 0.
    tbl.next_ns_id = 1;
    // The root namespace claims PIDs 0..NS_MAX_PIDS.
    tbl.next_pid_base = NS_MAX_PIDS;
    serial_println!("  PidNs: root namespace initialized (id=0, base=0)");
}

/// Create a new PID namespace that is a child of `parent_id`.
///
/// Returns the new namespace ID on success, or None if the table is full or
/// the parent namespace does not exist.
pub fn pid_ns_create(parent_id: u32) -> Option<u32> {
    let mut tbl = PID_NAMESPACES.lock();

    // Validate parent exists.
    if find_slot(&tbl, parent_id).is_none() {
        return None;
    }

    let free_idx = find_free_slot(&tbl)?;

    let new_id = tbl.next_ns_id;
    let pid_base = tbl.next_pid_base;

    tbl.slots[free_idx] = PidNamespace {
        id: new_id,
        parent_id,
        pid_base,
        max_pids: NS_MAX_PIDS,
        active: true,
    };

    // Advance counters using saturating/wrapping arithmetic.
    tbl.next_ns_id = tbl.next_ns_id.saturating_add(1);
    tbl.next_pid_base = tbl.next_pid_base.saturating_add(NS_MAX_PIDS);

    serial_println!(
        "  PidNs: created ns_id={} parent={} base={}",
        new_id,
        parent_id,
        pid_base
    );
    Some(new_id)
}

/// Destroy a PID namespace.
///
/// Marks the namespace inactive. Any processes that were in this namespace
/// should be reassigned to the parent namespace by the caller.
pub fn pid_ns_destroy(ns_id: u32) {
    // Never destroy the root namespace.
    if ns_id == 0 {
        return;
    }

    let mut tbl = PID_NAMESPACES.lock();

    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return,
    };

    let parent_id = tbl.slots[idx].parent_id;

    // Clear the slot.
    tbl.slots[idx] = PidNamespace::empty();

    serial_println!("  PidNs: destroyed ns_id={} (parent={})", ns_id, parent_id);

    // Reassign processes that were in this namespace to the parent.
    let mut map = PROC_NS_MAP.lock();
    for entry in map.iter_mut() {
        if *entry == ns_id {
            *entry = parent_id;
        }
    }
}

/// Convert a local (namespace-relative) PID to a global PID.
///
/// Returns 0 if the namespace does not exist or `local_pid` is out of range.
pub fn pid_ns_translate(ns_id: u32, local_pid: u32) -> u32 {
    let tbl = PID_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return 0,
    };
    let ns = &tbl.slots[idx];
    if local_pid >= ns.max_pids {
        return 0;
    }
    ns.pid_base.saturating_add(local_pid)
}

/// Convert a global PID to a local (namespace-relative) PID.
///
/// Returns None if the namespace does not exist or the global PID is outside
/// the namespace's range.
pub fn pid_ns_local(ns_id: u32, global_pid: u32) -> Option<u32> {
    let tbl = PID_NAMESPACES.lock();
    let idx = find_slot(&tbl, ns_id)?;
    let ns = &tbl.slots[idx];

    if global_pid < ns.pid_base {
        return None;
    }
    let local = global_pid - ns.pid_base;
    if local >= ns.max_pids {
        return None;
    }
    Some(local)
}

/// Return the root (init) namespace ID, which is always 0.
pub fn pid_ns_get_root() -> u32 {
    0
}

/// Look up which PID namespace a process belongs to.
///
/// Returns the root namespace ID (0) if the PID is out of range or the
/// process has no explicit namespace assignment.
pub fn get_process_ns(pid: u32) -> u32 {
    let map = PROC_NS_MAP.lock();
    let idx = pid as usize;
    if idx >= map.len() {
        return 0;
    }
    map[idx]
}

/// Assign a process to a PID namespace.
///
/// Returns false if `ns_id` does not exist or `pid` is out of range.
pub fn set_process_ns(ns_id: u32, pid: u32) -> bool {
    // Validate the namespace exists.
    {
        let tbl = PID_NAMESPACES.lock();
        if find_slot(&tbl, ns_id).is_none() {
            return false;
        }
    }

    let mut map = PROC_NS_MAP.lock();
    let idx = pid as usize;
    if idx >= map.len() {
        return false;
    }
    map[idx] = ns_id;
    true
}
