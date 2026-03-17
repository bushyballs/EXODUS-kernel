use crate::serial_println;
/// IPC Namespace implementation for Genesis
///
/// Provides Linux-compatible IPC namespace isolation. Each namespace
/// maintains independent ID spaces for:
///   - POSIX message queues (msqid)
///   - System V / POSIX shared memory segments (shmid)
///   - System V semaphore sets (semid)
///
/// Destroying a namespace signals that all IPC objects belonging to it
/// should be cleaned up (callers must handle actual resource teardown).
///
/// Rules enforced:
/// - No heap (no Vec/Box/String/alloc::*)
/// - No float casts
/// - No panics (no unwrap/expect/panic!)
/// - Saturating arithmetic for non-wraparound counters
/// - wrapping_add for sequence-number ID allocations
/// - All structs are Copy with const fn empty()
use crate::sync::Mutex;

/// Maximum number of concurrent IPC namespaces.
pub const MAX_IPC_NAMESPACES: usize = 16;

// ---------------------------------------------------------------------------
// IpcNamespace
// ---------------------------------------------------------------------------

/// Per-namespace IPC bookkeeping.
#[derive(Clone, Copy, Debug)]
pub struct IpcNamespace {
    /// Unique namespace ID (0 = init/root namespace).
    pub id: u32,
    /// Whether this slot is occupied.
    pub active: bool,
    /// Next message queue ID to hand out (wraps around).
    pub next_msqid: u32,
    /// Next shared memory segment ID to hand out (wraps around).
    pub next_shmid: u32,
    /// Next semaphore set ID to hand out (wraps around).
    pub next_semid: u32,
}

impl IpcNamespace {
    pub const fn empty() -> Self {
        IpcNamespace {
            id: 0,
            active: false,
            next_msqid: 0,
            next_shmid: 0,
            next_semid: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global namespace table
// ---------------------------------------------------------------------------

struct IpcNsTable {
    slots: [IpcNamespace; MAX_IPC_NAMESPACES],
    next_ns_id: u32,
}

impl IpcNsTable {
    const fn new() -> Self {
        const EMPTY: IpcNamespace = IpcNamespace::empty();
        IpcNsTable {
            slots: [EMPTY; MAX_IPC_NAMESPACES],
            next_ns_id: 1,
        }
    }
}

static IPC_NAMESPACES: Mutex<IpcNsTable> = Mutex::new(IpcNsTable::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_slot(tbl: &IpcNsTable, ns_id: u32) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if slot.active && slot.id == ns_id {
            return Some(i);
        }
    }
    None
}

fn find_free_slot(tbl: &IpcNsTable) -> Option<usize> {
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

/// Initialize the IPC namespace subsystem.
///
/// Creates the root (init) namespace with id=0.
pub fn init() {
    let mut tbl = IPC_NAMESPACES.lock();
    tbl.slots[0] = IpcNamespace {
        id: 0,
        active: true,
        next_msqid: 0,
        next_shmid: 0,
        next_semid: 0,
    };
    tbl.next_ns_id = 1;
    serial_println!("  IpcNs: root namespace initialized (id=0)");
}

/// Create a new, empty IPC namespace.
///
/// All ID counters start from 0 in the new namespace. Returns the new
/// namespace ID on success, or None if the table is full.
pub fn ipc_ns_create() -> Option<u32> {
    let mut tbl = IPC_NAMESPACES.lock();
    let free_idx = find_free_slot(&tbl)?;

    let new_id = tbl.next_ns_id;
    tbl.slots[free_idx] = IpcNamespace {
        id: new_id,
        active: true,
        next_msqid: 0,
        next_shmid: 0,
        next_semid: 0,
    };
    tbl.next_ns_id = tbl.next_ns_id.saturating_add(1);

    serial_println!("  IpcNs: created ns_id={}", new_id);
    Some(new_id)
}

/// Destroy an IPC namespace.
///
/// Marks the namespace inactive. Callers are responsible for cleaning up
/// all IPC objects (message queues, shared memory, semaphores) that were
/// allocated within this namespace before or after calling this function.
///
/// Never destroys the root namespace (id=0).
pub fn ipc_ns_destroy(ns_id: u32) {
    if ns_id == 0 {
        return;
    }
    let mut tbl = IPC_NAMESPACES.lock();
    if let Some(idx) = find_slot(&tbl, ns_id) {
        tbl.slots[idx] = IpcNamespace::empty();
    }
    serial_println!(
        "  IpcNs: destroyed ns_id={} — IPC objects for this NS must be cleaned up",
        ns_id
    );
}

/// Allocate the next message queue ID in namespace `ns_id`.
///
/// Uses wrapping addition so the counter cycles through the full u32 range.
/// Returns 0 if the namespace does not exist.
pub fn ipc_ns_alloc_msqid(ns_id: u32) -> u32 {
    let mut tbl = IPC_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return 0,
    };
    let id = tbl.slots[idx].next_msqid;
    tbl.slots[idx].next_msqid = tbl.slots[idx].next_msqid.wrapping_add(1);
    id
}

/// Allocate the next shared memory segment ID in namespace `ns_id`.
///
/// Uses wrapping addition so the counter cycles through the full u32 range.
/// Returns 0 if the namespace does not exist.
pub fn ipc_ns_alloc_shmid(ns_id: u32) -> u32 {
    let mut tbl = IPC_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return 0,
    };
    let id = tbl.slots[idx].next_shmid;
    tbl.slots[idx].next_shmid = tbl.slots[idx].next_shmid.wrapping_add(1);
    id
}

/// Allocate the next semaphore set ID in namespace `ns_id`.
///
/// Uses wrapping addition so the counter cycles through the full u32 range.
/// Returns 0 if the namespace does not exist.
pub fn ipc_ns_alloc_semid(ns_id: u32) -> u32 {
    let mut tbl = IPC_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return 0,
    };
    let id = tbl.slots[idx].next_semid;
    tbl.slots[idx].next_semid = tbl.slots[idx].next_semid.wrapping_add(1);
    id
}
