use crate::serial_println;
/// UTS Namespace implementation for Genesis
///
/// Provides Linux-compatible UTS (Unix Time-sharing System) namespace
/// isolation. Each namespace stores an independent hostname and domain
/// name. The root namespace (id=0) defaults to hostname="genesis",
/// domainname="local".
///
/// Rules enforced:
/// - No heap (no Vec/Box/String/alloc::*)
/// - No float casts
/// - No panics (no unwrap/expect/panic!)
/// - Saturating arithmetic for counters
/// - All structs are Copy with const fn empty()
use crate::sync::Mutex;

/// Maximum number of concurrent UTS namespaces.
pub const MAX_UTS_NAMESPACES: usize = 16;

// ---------------------------------------------------------------------------
// UtsNamespace
// ---------------------------------------------------------------------------

/// Per-namespace UTS data (hostname + domain name).
#[derive(Clone, Copy, Debug)]
pub struct UtsNamespace {
    /// Unique namespace ID (0 = init/root namespace).
    pub id: u32,
    /// Whether this slot is occupied.
    pub active: bool,
    /// NUL-padded hostname (max 63 chars + NUL).
    pub hostname: [u8; 64],
    /// NUL-padded domain name (max 63 chars + NUL).
    pub domainname: [u8; 64],
}

impl UtsNamespace {
    pub const fn empty() -> Self {
        UtsNamespace {
            id: 0,
            active: false,
            hostname: [0u8; 64],
            domainname: [0u8; 64],
        }
    }
}

// ---------------------------------------------------------------------------
// Global namespace table
// ---------------------------------------------------------------------------

struct UtsNsTable {
    slots: [UtsNamespace; MAX_UTS_NAMESPACES],
    next_ns_id: u32,
}

impl UtsNsTable {
    const fn new() -> Self {
        const EMPTY: UtsNamespace = UtsNamespace::empty();
        UtsNsTable {
            slots: [EMPTY; MAX_UTS_NAMESPACES],
            next_ns_id: 1,
        }
    }
}

static UTS_NAMESPACES: Mutex<UtsNsTable> = Mutex::new(UtsNsTable::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_slot(tbl: &UtsNsTable, ns_id: u32) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if slot.active && slot.id == ns_id {
            return Some(i);
        }
    }
    None
}

fn find_free_slot(tbl: &UtsNsTable) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if !slot.active {
            return Some(i);
        }
    }
    None
}

/// Copy `src` bytes into a fixed 64-byte buffer, zero-padding the rest.
/// Copies at most 63 bytes to leave room for a NUL terminator.
fn copy_name(dst: &mut [u8; 64], src: &[u8]) {
    let n = src.len().min(63);
    let (head, tail) = dst.split_at_mut(n);
    head.copy_from_slice(&src[..n]);
    for b in tail.iter_mut() {
        *b = 0;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the UTS namespace subsystem.
///
/// Creates the root namespace (id=0) with hostname="genesis" and
/// domainname="local".
pub fn init() {
    let mut tbl = UTS_NAMESPACES.lock();

    let mut root = UtsNamespace {
        id: 0,
        active: true,
        hostname: [0u8; 64],
        domainname: [0u8; 64],
    };

    copy_name(&mut root.hostname, b"genesis");
    copy_name(&mut root.domainname, b"local");

    tbl.slots[0] = root;
    tbl.next_ns_id = 1;

    serial_println!("  UtsNs: root namespace initialized (id=0, hostname=genesis, domain=local)");
}

/// Create a new UTS namespace as a child of `parent_id`.
///
/// The new namespace inherits the parent's hostname and domain name.
/// Returns the new namespace ID on success, or None if the table is full
/// or the parent does not exist.
pub fn uts_ns_create(parent_id: u32) -> Option<u32> {
    let mut tbl = UTS_NAMESPACES.lock();

    let parent_idx = find_slot(&tbl, parent_id)?;
    let parent_hostname = tbl.slots[parent_idx].hostname;
    let parent_domain = tbl.slots[parent_idx].domainname;

    let free_idx = find_free_slot(&tbl)?;

    let new_id = tbl.next_ns_id;
    tbl.slots[free_idx] = UtsNamespace {
        id: new_id,
        active: true,
        hostname: parent_hostname,
        domainname: parent_domain,
    };
    tbl.next_ns_id = tbl.next_ns_id.saturating_add(1);

    serial_println!("  UtsNs: created ns_id={} parent={}", new_id, parent_id);
    Some(new_id)
}

/// Set the hostname for a namespace.
///
/// `name` is a byte slice; at most 63 bytes are used.
/// Returns false if the namespace does not exist.
pub fn uts_ns_set_hostname(ns_id: u32, name: &[u8]) -> bool {
    let mut tbl = UTS_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return false,
    };
    copy_name(&mut tbl.slots[idx].hostname, name);
    true
}

/// Copy the hostname for a namespace into `out`.
///
/// Returns false if the namespace does not exist.
pub fn uts_ns_get_hostname(ns_id: u32, out: &mut [u8; 64]) -> bool {
    let tbl = UTS_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return false,
    };
    *out = tbl.slots[idx].hostname;
    true
}

/// Set the domain name for a namespace.
///
/// `name` is a byte slice; at most 63 bytes are used.
/// Returns false if the namespace does not exist.
pub fn uts_ns_set_domainname(ns_id: u32, name: &[u8]) -> bool {
    let mut tbl = UTS_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return false,
    };
    copy_name(&mut tbl.slots[idx].domainname, name);
    true
}

/// Copy the domain name for a namespace into `out`.
///
/// Returns false if the namespace does not exist.
pub fn uts_ns_get_domainname(ns_id: u32, out: &mut [u8; 64]) -> bool {
    let tbl = UTS_NAMESPACES.lock();
    let idx = match find_slot(&tbl, ns_id) {
        Some(i) => i,
        None => return false,
    };
    *out = tbl.slots[idx].domainname;
    true
}

/// Destroy a UTS namespace.
///
/// Never destroys the root namespace (id=0).
pub fn uts_ns_destroy(ns_id: u32) {
    if ns_id == 0 {
        return;
    }
    let mut tbl = UTS_NAMESPACES.lock();
    if let Some(idx) = find_slot(&tbl, ns_id) {
        tbl.slots[idx] = UtsNamespace::empty();
    }
    serial_println!("  UtsNs: destroyed ns_id={}", ns_id);
}
