use crate::serial_println;
/// Mount Namespace implementation for Genesis
///
/// Provides Linux-compatible mount namespace isolation. Each namespace has
/// its own view of the filesystem mount table. Creating a new mount
/// namespace copies the parent's mounts (clone semantics, like Linux's
/// CLONE_NEWNS).
///
/// Rules enforced:
/// - No heap (no Vec/Box/String/alloc::*)
/// - No float casts
/// - No panics (no unwrap/expect/panic!)
/// - Saturating arithmetic for counters
/// - All structs are Copy with const fn empty()
use crate::sync::Mutex;

/// Maximum number of concurrent mount namespaces.
pub const MAX_MNT_NAMESPACES: usize = 16;

/// Maximum total mount entries across all namespaces.
pub const MAX_MOUNT_ENTRIES: usize = 128;

/// Maximum per-namespace mounts returned by mnt_ns_list.
pub const MAX_LIST_ENTRIES: usize = 64;

// ---------------------------------------------------------------------------
// MountEntry
// ---------------------------------------------------------------------------

/// A single mount point entry in the global mount table.
#[derive(Clone, Copy, Debug)]
pub struct MountEntry {
    /// Namespace this mount belongs to.
    pub ns_id: u32,
    /// Device identifier (arbitrary u32 tag, e.g. from a driver registry).
    pub dev_id: u32,
    /// Absolute path at which the device is mounted (NUL-terminated).
    pub mount_point: [u8; 64],
    /// Filesystem type string (e.g. b"ext4\0...").
    pub fs_type: [u8; 16],
    /// Mount flags (e.g. MS_RDONLY = 1, MS_NOEXEC = 8, MS_NOSUID = 2).
    pub flags: u32,
    /// Whether this slot is occupied.
    pub active: bool,
}

impl MountEntry {
    pub const fn empty() -> Self {
        MountEntry {
            ns_id: 0,
            dev_id: 0,
            mount_point: [0u8; 64],
            fs_type: [0u8; 16],
            flags: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// MountNamespace
// ---------------------------------------------------------------------------

/// Per-namespace metadata record.
#[derive(Clone, Copy, Debug)]
pub struct MountNamespace {
    /// Unique namespace ID (0 = init/root namespace).
    pub id: u32,
    /// Parent namespace ID.
    pub parent_id: u32,
    /// Whether this slot is occupied.
    pub active: bool,
    /// Number of active mount entries owned by this namespace.
    pub mount_count: u32,
}

impl MountNamespace {
    pub const fn empty() -> Self {
        MountNamespace {
            id: 0,
            parent_id: 0,
            active: false,
            mount_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

struct MntNsTable {
    slots: [MountNamespace; MAX_MNT_NAMESPACES],
    next_ns_id: u32,
}

impl MntNsTable {
    const fn new() -> Self {
        const EMPTY: MountNamespace = MountNamespace::empty();
        MntNsTable {
            slots: [EMPTY; MAX_MNT_NAMESPACES],
            next_ns_id: 1,
        }
    }
}

static MOUNT_NAMESPACES: Mutex<MntNsTable> = Mutex::new(MntNsTable::new());

const EMPTY_ENTRY: MountEntry = MountEntry::empty();
static MOUNT_TABLE: Mutex<[MountEntry; MAX_MOUNT_ENTRIES]> =
    Mutex::new([EMPTY_ENTRY; MAX_MOUNT_ENTRIES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_ns_slot(tbl: &MntNsTable, ns_id: u32) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if slot.active && slot.id == ns_id {
            return Some(i);
        }
    }
    None
}

fn find_free_ns_slot(tbl: &MntNsTable) -> Option<usize> {
    for (i, slot) in tbl.slots.iter().enumerate() {
        if !slot.active {
            return Some(i);
        }
    }
    None
}

/// Copy `src` bytes into a fixed-size buffer, zero-filling the remainder.
/// Copies at most `buf.len()` bytes.
fn copy_bytes(dst: &mut [u8], src: &[u8]) {
    let n = src.len().min(dst.len());
    let (head, tail) = dst.split_at_mut(n);
    head.copy_from_slice(&src[..n]);
    for b in tail.iter_mut() {
        *b = 0;
    }
}

/// Compare a byte slice against a fixed-size buffer (NUL-terminated style).
fn bytes_eq(buf: &[u8], src: &[u8]) -> bool {
    let n = src.len();
    if n > buf.len() {
        return false;
    }
    &buf[..n] == src && (n == buf.len() || buf[n] == 0)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the mount namespace subsystem.
///
/// Creates the root (init) namespace with id=0.
pub fn init() {
    let mut tbl = MOUNT_NAMESPACES.lock();
    tbl.slots[0] = MountNamespace {
        id: 0,
        parent_id: 0,
        active: true,
        mount_count: 0,
    };
    tbl.next_ns_id = 1;
    serial_println!("  MntNs: root namespace initialized (id=0)");
}

/// Create a new mount namespace as a child of `parent_id`.
///
/// Copies all of the parent's mount entries into the new namespace
/// (Linux clone semantics). Returns the new namespace ID on success,
/// or None if the table is full or the parent does not exist.
pub fn mnt_ns_create(parent_id: u32) -> Option<u32> {
    // --- phase 1: allocate namespace slot ---
    let new_id;
    let parent_mount_count;
    {
        let mut tbl = MOUNT_NAMESPACES.lock();

        let parent_idx = find_ns_slot(&tbl, parent_id)?;
        parent_mount_count = tbl.slots[parent_idx].mount_count;

        let free_idx = find_free_ns_slot(&tbl)?;

        new_id = tbl.next_ns_id;
        tbl.slots[free_idx] = MountNamespace {
            id: new_id,
            parent_id,
            active: true,
            mount_count: 0,
        };
        tbl.next_ns_id = tbl.next_ns_id.saturating_add(1);
    }

    // --- phase 2: clone parent mounts ---
    let mut cloned: u32 = 0;
    {
        let mut mounts = MOUNT_TABLE.lock();

        // Collect parent entries first (two-pass to avoid borrow conflict).
        // We process at most MAX_MOUNT_ENTRIES entries.
        let mut parent_entries: [Option<MountEntry>; MAX_MOUNT_ENTRIES] = [None; MAX_MOUNT_ENTRIES];
        let mut count = 0usize;
        for slot in mounts.iter() {
            if slot.active && slot.ns_id == parent_id {
                if count < MAX_MOUNT_ENTRIES {
                    parent_entries[count] = Some(*slot);
                    count = count.saturating_add(1);
                }
            }
        }

        // Insert clones.
        for i in 0..count {
            if let Some(mut entry) = parent_entries[i] {
                // Find a free slot in MOUNT_TABLE.
                let free = mounts.iter().position(|e| !e.active);
                if let Some(fi) = free {
                    entry.ns_id = new_id;
                    mounts[fi] = entry;
                    cloned = cloned.saturating_add(1);
                }
            }
        }
    }

    // Update mount_count for the new namespace.
    {
        let mut tbl = MOUNT_NAMESPACES.lock();
        if let Some(idx) = find_ns_slot(&tbl, new_id) {
            tbl.slots[idx].mount_count = cloned;
        }
    }

    serial_println!(
        "  MntNs: created ns_id={} parent={} cloned={} mounts",
        new_id,
        parent_id,
        cloned
    );
    Some(new_id)
}

/// Destroy a mount namespace and remove all its mount entries.
///
/// Never destroys the root namespace (id=0).
pub fn mnt_ns_destroy(ns_id: u32) {
    if ns_id == 0 {
        return;
    }

    // Remove all mount entries for this namespace.
    {
        let mut mounts = MOUNT_TABLE.lock();
        for slot in mounts.iter_mut() {
            if slot.active && slot.ns_id == ns_id {
                *slot = MountEntry::empty();
            }
        }
    }

    // Mark namespace slot inactive.
    {
        let mut tbl = MOUNT_NAMESPACES.lock();
        if let Some(idx) = find_ns_slot(&tbl, ns_id) {
            tbl.slots[idx] = MountNamespace::empty();
        }
    }

    serial_println!("  MntNs: destroyed ns_id={}", ns_id);
}

/// Mount a device into a namespace at the given path.
///
/// `point` and `fstype` are byte slices (not required to be NUL-terminated).
/// Returns true on success, false if the namespace does not exist or the
/// mount table is full.
pub fn mnt_ns_mount(ns_id: u32, dev_id: u32, point: &[u8], fstype: &[u8], flags: u32) -> bool {
    // Validate namespace exists.
    {
        let tbl = MOUNT_NAMESPACES.lock();
        if find_ns_slot(&tbl, ns_id).is_none() {
            return false;
        }
    }

    // Insert a new entry.
    let inserted;
    {
        let mut mounts = MOUNT_TABLE.lock();
        let free = mounts.iter().position(|e| !e.active);
        match free {
            None => {
                inserted = false;
            }
            Some(fi) => {
                let mut entry = MountEntry::empty();
                entry.ns_id = ns_id;
                entry.dev_id = dev_id;
                entry.flags = flags;
                entry.active = true;
                copy_bytes(&mut entry.mount_point, point);
                copy_bytes(&mut entry.fs_type, fstype);
                mounts[fi] = entry;
                inserted = true;
            }
        }
    }

    if inserted {
        let mut tbl = MOUNT_NAMESPACES.lock();
        if let Some(idx) = find_ns_slot(&tbl, ns_id) {
            tbl.slots[idx].mount_count = tbl.slots[idx].mount_count.saturating_add(1);
        }
    }

    inserted
}

/// Unmount the first matching mount point in a namespace.
///
/// Returns true if a matching entry was found and removed.
pub fn mnt_ns_umount(ns_id: u32, point: &[u8]) -> bool {
    let mut removed = false;
    let mut mounts = MOUNT_TABLE.lock();

    for slot in mounts.iter_mut() {
        if slot.active && slot.ns_id == ns_id && bytes_eq(&slot.mount_point, point) {
            *slot = MountEntry::empty();
            removed = true;
            break;
        }
    }

    if removed {
        drop(mounts); // release before taking tbl lock
        let mut tbl = MOUNT_NAMESPACES.lock();
        if let Some(idx) = find_ns_slot(&tbl, ns_id) {
            tbl.slots[idx].mount_count = tbl.slots[idx].mount_count.saturating_sub(1);
        }
    }

    removed
}

/// Look up the device ID mounted at `path` in namespace `ns_id`.
///
/// Returns Some(dev_id) if found, None otherwise.
pub fn mnt_ns_lookup(ns_id: u32, path: &[u8]) -> Option<u32> {
    let mounts = MOUNT_TABLE.lock();
    for slot in mounts.iter() {
        if slot.active && slot.ns_id == ns_id && bytes_eq(&slot.mount_point, path) {
            return Some(slot.dev_id);
        }
    }
    None
}

/// List mount entries for namespace `ns_id` into the provided output array.
///
/// Fills `out` with up to 64 entries and returns the number of entries written.
pub fn mnt_ns_list(ns_id: u32, out: &mut [MountEntry; MAX_LIST_ENTRIES]) -> u32 {
    let mounts = MOUNT_TABLE.lock();
    let mut count: u32 = 0;

    for slot in mounts.iter() {
        if slot.active && slot.ns_id == ns_id {
            if (count as usize) < MAX_LIST_ENTRIES {
                out[count as usize] = *slot;
                count = count.saturating_add(1);
            }
        }
    }

    count
}
