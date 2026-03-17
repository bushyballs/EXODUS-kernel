/// BPF Maps — Fixed-capacity key/value stores for the eBPF subsystem
///
/// Implements three map variants operating entirely without heap allocation:
///
///   * Hash map   — open-addressing, linear probe, FNV-1a hash, up to 16 instances
///   * Array map  — u32-indexed flat store, up to 16 instances
///   * Ring buffer — circular byte-stream with 8-byte record headers, up to 8 instances
///
/// All storage lives in three type-segregated static pools.  A fourth static,
/// MAP_DESCRIPTORS, maps file descriptors to (map_type, pool_index) tuples so
/// lookups need only one lock acquisition.
///
/// ## Kernel-critical invariants (violations crash the kernel)
///   - No heap: no Vec, Box, String, alloc::* — all arrays are fixed-size statics
///   - No float casts: no `as f64` / `as f32`
///   - No panics: no unwrap(), expect(), panic!()
///   - Saturating arithmetic for counters; wrapping_add for sequence numbers
///   - MMIO via read_volatile / write_volatile only
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Map type constants
// ---------------------------------------------------------------------------

pub const BPF_MAP_TYPE_UNSPEC: u32 = 0;
pub const BPF_MAP_TYPE_HASH: u32 = 1;
pub const BPF_MAP_TYPE_ARRAY: u32 = 2;
pub const BPF_MAP_TYPE_PROG_ARRAY: u32 = 3;
pub const BPF_MAP_TYPE_PERF_EVENT_ARRAY: u32 = 4;
pub const BPF_MAP_TYPE_PERCPU_HASH: u32 = 5;
pub const BPF_MAP_TYPE_PERCPU_ARRAY: u32 = 6;
pub const BPF_MAP_TYPE_STACK_TRACE: u32 = 7;
pub const BPF_MAP_TYPE_LRU_HASH: u32 = 9;
pub const BPF_MAP_TYPE_RINGBUF: u32 = 27;

// ---------------------------------------------------------------------------
// Map flags
// ---------------------------------------------------------------------------

pub const BPF_F_NO_PREALLOC: u32 = 1;
pub const BPF_F_RDONLY: u32 = 1 << 3;
pub const BPF_F_WRONLY: u32 = 1 << 4;
pub const BPF_F_RDONLY_PROG: u32 = 1 << 7;
pub const BPF_F_WRONLY_PROG: u32 = 1 << 8;

// ---------------------------------------------------------------------------
// Lookup / update flags
// ---------------------------------------------------------------------------

/// Any: insert or overwrite unconditionally
pub const BPF_ANY: u64 = 0;
/// No-exist: fail with EEXIST (-17) if the key is already present
pub const BPF_NOEXIST: u64 = 1;
/// Exist: fail with ENOENT (-2) if the key is not present
pub const BPF_EXIST: u64 = 2;

// ---------------------------------------------------------------------------
// BPF syscall command codes (used by sys_bpf)
// ---------------------------------------------------------------------------

pub const BPF_MAP_CREATE: u64 = 0;
pub const BPF_MAP_LOOKUP_ELEM: u64 = 1;
pub const BPF_MAP_UPDATE_ELEM: u64 = 2;
pub const BPF_MAP_DELETE_ELEM: u64 = 3;
pub const BPF_MAP_GET_NEXT_KEY: u64 = 4;

// ---------------------------------------------------------------------------
// Capacity constants
// ---------------------------------------------------------------------------

/// FD range: map FDs are BPF_MAP_FD_BASE .. BPF_MAP_FD_BASE + BPF_MAX_MAPS
pub const BPF_MAP_FD_BASE: i32 = 10_000;

/// Maximum number of simultaneously active map descriptors
pub const BPF_MAX_MAPS: usize = 64;

/// Maximum number of hash-map instances in the pool
pub const BPF_MAX_HASH_MAPS: usize = 16;

/// Maximum number of array-map instances in the pool
pub const BPF_MAX_ARRAY_MAPS: usize = 16;

/// Maximum number of ring-buffer instances in the pool
pub const BPF_MAX_RINGBUF_MAPS: usize = 8;

/// Open-addressing slot count for each hash map
pub const BPF_HASH_SLOTS: usize = 256;

/// Slot count for each array map
pub const BPF_ARRAY_SLOTS: usize = 1024;

/// Maximum key size in bytes accepted by any map
pub const BPF_KEY_SIZE: usize = 32;

/// Maximum value size in bytes accepted by any map
pub const BPF_VAL_SIZE: usize = 256;

/// Byte capacity of the data area in each ring buffer
pub const BPF_RINGBUF_SIZE: usize = 4096;

/// Size of the per-record header prepended to every ring-buffer write
pub const BPF_RINGBUF_HDR: usize = 8; // len: u32 (4) + pad: u32 (4)

// ---------------------------------------------------------------------------
// Hash map entry
// ---------------------------------------------------------------------------

/// A single occupied or empty slot in an open-addressing hash table.
#[derive(Copy, Clone)]
pub struct BpfHashEntry {
    /// Key bytes (left-aligned; bytes beyond key_len are undefined)
    pub key: [u8; BPF_KEY_SIZE],
    /// Value bytes (left-aligned; bytes beyond val_len are undefined)
    pub val: [u8; BPF_VAL_SIZE],
    /// Actual byte length of the stored key (0..=BPF_KEY_SIZE)
    pub key_len: u8,
    /// Actual byte length of the stored value (0..=BPF_VAL_SIZE as u16)
    pub val_len: u16,
    /// true → slot holds a valid entry; false → slot is empty
    pub occupied: bool,
}

impl BpfHashEntry {
    pub const fn empty() -> Self {
        BpfHashEntry {
            key: [0u8; BPF_KEY_SIZE],
            val: [0u8; BPF_VAL_SIZE],
            key_len: 0,
            val_len: 0,
            occupied: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Array map entry
// ---------------------------------------------------------------------------

/// One element in an array map, indexed by u32 key.
#[derive(Copy, Clone)]
pub struct BpfArrayEntry {
    /// Value bytes (left-aligned; bytes beyond val_len are undefined)
    pub val: [u8; BPF_VAL_SIZE],
    /// Actual byte length of the stored value
    pub val_len: u16,
    /// false → element has never been written (reads return zero)
    pub initialized: bool,
}

impl BpfArrayEntry {
    pub const fn empty() -> Self {
        BpfArrayEntry {
            val: [0u8; BPF_VAL_SIZE],
            val_len: 0,
            initialized: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer state
// ---------------------------------------------------------------------------

/// Per-instance ring buffer state.
///
/// head = consumer position (bytes consumed from the start of the data window)
/// tail = producer position (bytes written past the start of the data window)
///
/// Both are monotonically increasing byte offsets; the actual byte index is
/// obtained by `pos % BPF_RINGBUF_SIZE`.  A ring is full when
/// `tail.wrapping_sub(head) == BPF_RINGBUF_SIZE as u32`.
#[derive(Copy, Clone)]
pub struct BpfRingBuf {
    pub data: [u8; BPF_RINGBUF_SIZE],
    /// Consumer offset (wrapping u32 byte position)
    pub head: u32,
    /// Producer offset (wrapping u32 byte position)
    pub tail: u32,
    /// Total records successfully enqueued
    pub sample_count: u64,
    /// Total records dropped because the buffer was full
    pub lost_count: u64,
}

impl BpfRingBuf {
    pub const fn empty() -> Self {
        BpfRingBuf {
            data: [0u8; BPF_RINGBUF_SIZE],
            head: 0,
            tail: 0,
            sample_count: 0,
            lost_count: 0,
        }
    }

    /// Returns the number of bytes currently available to read.
    #[inline]
    fn used(&self) -> u32 {
        self.tail.wrapping_sub(self.head)
    }

    /// Returns the number of bytes available for writing.
    #[inline]
    fn free(&self) -> u32 {
        (BPF_RINGBUF_SIZE as u32).saturating_sub(self.used())
    }
}

// ---------------------------------------------------------------------------
// Hash map pool entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct BpfHashMap {
    pub entries: [BpfHashEntry; BPF_HASH_SLOTS],
    pub count: u32,
    pub max_entries: u32,
    pub active: bool,
}

impl BpfHashMap {
    pub const fn empty() -> Self {
        BpfHashMap {
            entries: [BpfHashEntry::empty(); BPF_HASH_SLOTS],
            count: 0,
            max_entries: BPF_HASH_SLOTS as u32,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Array map pool entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct BpfArrayMap {
    pub entries: [BpfArrayEntry; BPF_ARRAY_SLOTS],
    pub max_entries: u32,
    pub active: bool,
}

impl BpfArrayMap {
    pub const fn empty() -> Self {
        BpfArrayMap {
            entries: [BpfArrayEntry::empty(); BPF_ARRAY_SLOTS],
            max_entries: BPF_ARRAY_SLOTS as u32,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer map pool entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct BpfRingbufMap {
    pub rb: BpfRingBuf,
    pub active: bool,
}

impl BpfRingbufMap {
    pub const fn empty() -> Self {
        BpfRingbufMap {
            rb: BpfRingBuf::empty(),
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Map descriptor
// ---------------------------------------------------------------------------

/// Lightweight descriptor that maps a synthetic fd to one pool entry.
#[derive(Copy, Clone)]
pub struct BpfMapDesc {
    /// Synthetic file descriptor (BPF_MAP_FD_BASE + descriptor_index)
    pub fd: i32,
    /// One of the BPF_MAP_TYPE_* constants
    pub map_type: u32,
    /// Index into the type-specific pool (HASH_MAPS / ARRAY_MAPS / RINGBUF_MAPS)
    pub type_idx: usize,
    /// Key size constraint declared at creation
    pub key_size: u32,
    /// Value size constraint declared at creation
    pub value_size: u32,
    /// Maximum number of entries (for array maps, clamped to BPF_ARRAY_SLOTS)
    pub max_entries: u32,
    /// Flags bitmask passed to bpf_map_create
    pub flags: u32,
    /// false → descriptor slot is free
    pub active: bool,
}

impl BpfMapDesc {
    pub const fn empty() -> Self {
        BpfMapDesc {
            fd: 0,
            map_type: BPF_MAP_TYPE_UNSPEC,
            type_idx: 0,
            key_size: 0,
            value_size: 0,
            max_entries: 0,
            flags: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static storage pools
// ---------------------------------------------------------------------------

// Each pool is an independent mutex so that hash-map and array-map operations
// never block each other.

static HASH_MAPS: Mutex<[BpfHashMap; BPF_MAX_HASH_MAPS]> =
    Mutex::new([BpfHashMap::empty(); BPF_MAX_HASH_MAPS]);

static ARRAY_MAPS: Mutex<[BpfArrayMap; BPF_MAX_ARRAY_MAPS]> =
    Mutex::new([BpfArrayMap::empty(); BPF_MAX_ARRAY_MAPS]);

static RINGBUF_MAPS: Mutex<[BpfRingbufMap; BPF_MAX_RINGBUF_MAPS]> =
    Mutex::new([BpfRingbufMap::empty(); BPF_MAX_RINGBUF_MAPS]);

static MAP_DESCRIPTORS: Mutex<[BpfMapDesc; BPF_MAX_MAPS]> =
    Mutex::new([BpfMapDesc::empty(); BPF_MAX_MAPS]);

// ---------------------------------------------------------------------------
// Internal: FNV-1a 32-bit hash (no alloc, no float, no division)
// ---------------------------------------------------------------------------

/// Compute the FNV-1a 32-bit hash of `data`.
///
/// Uses only wrapping multiplication and XOR — safe at opt-level 0.
#[inline]
fn fnv1a_hash(data: &[u8]) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    let mut i = 0usize;
    while i < data.len() {
        hash ^= data[i] as u32;
        hash = hash.wrapping_mul(16_777_619);
        i = i.saturating_add(1);
    }
    hash
}

/// Linear-probe slot index: (hash + attempt) mod max_slots.
#[inline]
fn probe_slot(hash: u32, attempt: u32, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    hash.wrapping_add(attempt) % max
}

// ---------------------------------------------------------------------------
// Internal: descriptor helpers
// ---------------------------------------------------------------------------

/// Allocate the next free descriptor slot; return its index or None if full.
fn alloc_desc_slot(descs: &mut [BpfMapDesc; BPF_MAX_MAPS]) -> Option<usize> {
    let mut i = 0usize;
    while i < BPF_MAX_MAPS {
        if !descs[i].active {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Find the descriptor slot for a given fd. Returns None if not active.
fn find_desc_slot(descs: &[BpfMapDesc; BPF_MAX_MAPS], fd: i32) -> Option<usize> {
    let mut i = 0usize;
    while i < BPF_MAX_MAPS {
        if descs[i].active && descs[i].fd == fd {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Internal: key equality helper
// ---------------------------------------------------------------------------

/// Return true if the first `len` bytes of `a` and `b` are identical.
#[inline]
fn key_eq(a: &[u8; BPF_KEY_SIZE], b: &[u8], len: usize) -> bool {
    if b.len() < len || len > BPF_KEY_SIZE {
        return false;
    }
    let mut i = 0usize;
    while i < len {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the BPF map subsystem.
///
/// Must be called once during kernel boot (from `kernel::init()`).
/// Zeroes all pool entries (they are already zero-initialized in BSS,
/// but this also confirms the spinlocks start in the unlocked state).
pub fn init() {
    // Acquiring and releasing each mutex verifies the spinlock round-trips
    // correctly on this platform.  The pools are const-initialized to empty
    // so no further work is needed.
    {
        let _h = HASH_MAPS.lock();
        let _a = ARRAY_MAPS.lock();
        let _r = RINGBUF_MAPS.lock();
        let _d = MAP_DESCRIPTORS.lock();
    }
    serial_println!(
        "  [bpf_map] subsystem ready ({} desc slots, {} hash, {} array, {} ringbuf pools)",
        BPF_MAX_MAPS,
        BPF_MAX_HASH_MAPS,
        BPF_MAX_ARRAY_MAPS,
        BPF_MAX_RINGBUF_MAPS
    );
}

// ---------------------------------------------------------------------------
// bpf_map_create
// ---------------------------------------------------------------------------

/// Create a new BPF map of the requested type.
///
/// Returns a synthetic file descriptor >= `BPF_MAP_FD_BASE` on success,
/// or a negative errno:
///   -22 (EINVAL)  — unsupported map_type, or sizes exceed limits
///   -12 (ENOMEM)  — descriptor table or type-pool is exhausted
pub fn bpf_map_create(
    map_type: u32,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    flags: u32,
) -> i32 {
    // Validate sizes
    if key_size == 0 || key_size as usize > BPF_KEY_SIZE {
        return -22; // EINVAL
    }
    if value_size == 0 || value_size as usize > BPF_VAL_SIZE {
        return -22; // EINVAL
    }

    // Validate that the map type is one we support
    match map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {}
        BPF_MAP_TYPE_ARRAY => {}
        BPF_MAP_TYPE_RINGBUF => {}
        _ => return -22, // EINVAL — unsupported type
    }

    // Allocate descriptor slot
    let desc_idx = {
        let mut descs = MAP_DESCRIPTORS.lock();
        match alloc_desc_slot(&mut *descs) {
            Some(idx) => idx,
            None => return -12, // ENOMEM
        }
    };

    let fd = BPF_MAP_FD_BASE.saturating_add(desc_idx as i32);

    // Allocate a type-specific pool slot and record the descriptor
    match map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let mut pool = HASH_MAPS.lock();
            let mut type_idx = BPF_MAX_HASH_MAPS; // sentinel = not found
            let mut i = 0usize;
            while i < BPF_MAX_HASH_MAPS {
                if !pool[i].active {
                    type_idx = i;
                    break;
                }
                i = i.saturating_add(1);
            }
            if type_idx == BPF_MAX_HASH_MAPS {
                return -12; // ENOMEM — pool exhausted
            }
            let effective_max = if max_entries == 0 || max_entries as usize > BPF_HASH_SLOTS {
                BPF_HASH_SLOTS as u32
            } else {
                max_entries
            };
            pool[type_idx].max_entries = effective_max;
            pool[type_idx].count = 0;
            pool[type_idx].active = true;
            // (entries array is already zeroed / unoccupied from const init)

            let mut descs = MAP_DESCRIPTORS.lock();
            descs[desc_idx] = BpfMapDesc {
                fd,
                map_type,
                type_idx,
                key_size,
                value_size,
                max_entries: effective_max,
                flags,
                active: true,
            };
        }
        BPF_MAP_TYPE_ARRAY => {
            let mut pool = ARRAY_MAPS.lock();
            let mut type_idx = BPF_MAX_ARRAY_MAPS;
            let mut i = 0usize;
            while i < BPF_MAX_ARRAY_MAPS {
                if !pool[i].active {
                    type_idx = i;
                    break;
                }
                i = i.saturating_add(1);
            }
            if type_idx == BPF_MAX_ARRAY_MAPS {
                return -12; // ENOMEM
            }
            let effective_max = if max_entries == 0 || max_entries as usize > BPF_ARRAY_SLOTS {
                BPF_ARRAY_SLOTS as u32
            } else {
                max_entries
            };
            pool[type_idx].max_entries = effective_max;
            pool[type_idx].active = true;

            let mut descs = MAP_DESCRIPTORS.lock();
            descs[desc_idx] = BpfMapDesc {
                fd,
                map_type,
                type_idx,
                key_size,
                value_size,
                max_entries: effective_max,
                flags,
                active: true,
            };
        }
        BPF_MAP_TYPE_RINGBUF => {
            let mut pool = RINGBUF_MAPS.lock();
            let mut type_idx = BPF_MAX_RINGBUF_MAPS;
            let mut i = 0usize;
            while i < BPF_MAX_RINGBUF_MAPS {
                if !pool[i].active {
                    type_idx = i;
                    break;
                }
                i = i.saturating_add(1);
            }
            if type_idx == BPF_MAX_RINGBUF_MAPS {
                return -12; // ENOMEM
            }
            pool[type_idx].rb = BpfRingBuf::empty();
            pool[type_idx].active = true;

            let mut descs = MAP_DESCRIPTORS.lock();
            descs[desc_idx] = BpfMapDesc {
                fd,
                map_type,
                type_idx,
                key_size,
                value_size,
                max_entries,
                flags,
                active: true,
            };
        }
        _ => return -22, // EINVAL (already filtered above, but exhaustive)
    }

    fd
}

// ---------------------------------------------------------------------------
// bpf_map_lookup
// ---------------------------------------------------------------------------

/// Look up a key in the map identified by `fd`.
///
/// Returns a copy of the value on success, or None if:
///   - `fd` is not a valid active map fd
///   - the key is not found
///   - the map type does not support lookup (RINGBUF)
///
/// The returned array is always `BPF_VAL_SIZE` bytes; only the first
/// `desc.value_size` bytes carry meaningful data.
pub fn bpf_map_lookup(fd: i32, key: &[u8]) -> Option<[u8; BPF_VAL_SIZE]> {
    let desc = bpf_map_get_info(fd)?;

    // Key length must match the declared key_size
    if key.len() != desc.key_size as usize {
        return None;
    }

    match desc.map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let pool = HASH_MAPS.lock();
            let hm = &pool[desc.type_idx];
            if !hm.active {
                return None;
            }
            let hash = fnv1a_hash(key);
            let slots = hm.max_entries;
            if slots == 0 {
                return None;
            }
            let mut attempt = 0u32;
            while attempt < slots {
                let idx = probe_slot(hash, attempt, slots) as usize;
                let entry = &hm.entries[idx];
                if !entry.occupied {
                    // Empty slot encountered — key is absent (linear probe invariant)
                    return None;
                }
                if entry.key_len as usize == key.len() && key_eq(&entry.key, key, key.len()) {
                    return Some(entry.val);
                }
                attempt = attempt.saturating_add(1);
            }
            None
        }
        BPF_MAP_TYPE_ARRAY => {
            if key.len() < 4 {
                return None;
            }
            let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
            let pool = ARRAY_MAPS.lock();
            let am = &pool[desc.type_idx];
            if !am.active || idx >= am.max_entries as usize {
                return None;
            }
            let entry = &am.entries[idx];
            Some(entry.val) // returns zero-filled val even if not initialized
        }
        BPF_MAP_TYPE_RINGBUF => {
            // Ring buffers do not support key-based lookup
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// bpf_map_update
// ---------------------------------------------------------------------------

/// Insert or update a key/value pair in the map.
///
/// Returns 0 on success, or a negative errno:
///   -22 (EINVAL)  — bad fd, key/value size mismatch, index out of range
///   -17 (EEXIST)  — BPF_NOEXIST and key already present
///   -2  (ENOENT)  — BPF_EXIST and key not present
///   -28 (ENOSPC)  — hash map is full
///   -95 (EOPNOTSUPP) — map type does not support update
pub fn bpf_map_update(fd: i32, key: &[u8], value: &[u8], flags: u64) -> i32 {
    let desc = match bpf_map_get_info(fd) {
        Some(d) => d,
        None => return -22, // EINVAL
    };

    if key.len() != desc.key_size as usize {
        return -22; // EINVAL
    }
    if value.len() != desc.value_size as usize {
        return -22; // EINVAL
    }

    match desc.map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let mut pool = HASH_MAPS.lock();
            let hm = &mut pool[desc.type_idx];
            if !hm.active {
                return -22; // EINVAL
            }

            let hash = fnv1a_hash(key);
            let slots = hm.max_entries;
            if slots == 0 {
                return -22; // EINVAL
            }

            // First pass: probe for an existing entry with this key
            let mut existing_slot: Option<usize> = None;
            let mut first_empty: Option<usize> = None;
            let mut attempt = 0u32;
            while attempt < slots {
                let idx = probe_slot(hash, attempt, slots) as usize;
                let entry = &hm.entries[idx];
                if entry.occupied {
                    if entry.key_len as usize == key.len() && key_eq(&entry.key, key, key.len()) {
                        existing_slot = Some(idx);
                        break;
                    }
                } else {
                    if first_empty.is_none() {
                        first_empty = Some(idx);
                    }
                    // Cannot stop early in linear probe — a later slot might
                    // hold the key if a prior collision occurred.  We must
                    // continue until we either find the key or hit an empty
                    // slot that was never displaced.
                    break;
                }
                attempt = attempt.saturating_add(1);
            }

            // Apply flag semantics
            match flags {
                BPF_NOEXIST => {
                    if existing_slot.is_some() {
                        return -17; // EEXIST
                    }
                }
                BPF_EXIST => {
                    if existing_slot.is_none() {
                        return -2; // ENOENT
                    }
                }
                _ => {} // BPF_ANY — upsert, no restriction
            }

            if let Some(slot) = existing_slot {
                // Overwrite in place
                let entry = &mut hm.entries[slot];
                let vlen = value.len();
                let mut vi = 0usize;
                while vi < vlen {
                    entry.val[vi] = value[vi];
                    vi = vi.saturating_add(1);
                }
                entry.val_len = vlen as u16;
                return 0;
            }

            // Insert into first_empty slot
            if hm.count >= hm.max_entries {
                return -28; // ENOSPC
            }

            // Perform a fresh probe to find the insertion slot
            // (first_empty may have been found before completing a full scan)
            let insert_slot = {
                let mut found: Option<usize> = None;
                let mut att2 = 0u32;
                while att2 < slots {
                    let idx = probe_slot(hash, att2, slots) as usize;
                    if !hm.entries[idx].occupied {
                        found = Some(idx);
                        break;
                    }
                    att2 = att2.saturating_add(1);
                }
                match found {
                    Some(s) => s,
                    None => return -28, // ENOSPC — table fully occupied
                }
            };

            let entry = &mut hm.entries[insert_slot];
            let klen = key.len();
            let vlen = value.len();
            let mut ki = 0usize;
            while ki < klen {
                entry.key[ki] = key[ki];
                ki = ki.saturating_add(1);
            }
            let mut vi = 0usize;
            while vi < vlen {
                entry.val[vi] = value[vi];
                vi = vi.saturating_add(1);
            }
            entry.key_len = klen as u8;
            entry.val_len = vlen as u16;
            entry.occupied = true;
            hm.count = hm.count.saturating_add(1);
            0
        }

        BPF_MAP_TYPE_ARRAY => {
            if key.len() < 4 {
                return -22; // EINVAL
            }
            let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
            let mut pool = ARRAY_MAPS.lock();
            let am = &mut pool[desc.type_idx];
            if !am.active || idx >= am.max_entries as usize {
                return -22; // EINVAL
            }

            // BPF_NOEXIST / BPF_EXIST semantics for array maps
            match flags {
                BPF_NOEXIST => {
                    if am.entries[idx].initialized {
                        return -17; // EEXIST
                    }
                }
                BPF_EXIST => {
                    if !am.entries[idx].initialized {
                        return -2; // ENOENT
                    }
                }
                _ => {}
            }

            let entry = &mut am.entries[idx];
            let vlen = value.len();
            let mut vi = 0usize;
            while vi < vlen {
                entry.val[vi] = value[vi];
                vi = vi.saturating_add(1);
            }
            entry.val_len = vlen as u16;
            entry.initialized = true;
            0
        }

        BPF_MAP_TYPE_RINGBUF => -95, // EOPNOTSUPP — use bpf_ringbuf_output instead

        _ => -22, // EINVAL
    }
}

// ---------------------------------------------------------------------------
// bpf_map_delete
// ---------------------------------------------------------------------------

/// Delete a key from the map.
///
/// Returns 0 on success, -2 (ENOENT) if the key is not found,
/// -22 (EINVAL) for invalid fd / size mismatch.
///
/// Hash map deletion uses a tombstone-free approach: after removing the
/// target entry the probe chain is repaired by rehashing all entries that
/// follow in the same cluster (Robin Hood deletion).
pub fn bpf_map_delete(fd: i32, key: &[u8]) -> i32 {
    let desc = match bpf_map_get_info(fd) {
        Some(d) => d,
        None => return -22,
    };

    if key.len() != desc.key_size as usize {
        return -22;
    }

    match desc.map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let mut pool = HASH_MAPS.lock();
            let hm = &mut pool[desc.type_idx];
            if !hm.active {
                return -22;
            }

            let hash = fnv1a_hash(key);
            let slots = hm.max_entries;
            if slots == 0 {
                return -2;
            }

            // Find the target slot
            let mut target: Option<usize> = None;
            let mut attempt = 0u32;
            while attempt < slots {
                let idx = probe_slot(hash, attempt, slots) as usize;
                let entry = &hm.entries[idx];
                if !entry.occupied {
                    return -2; // ENOENT — hit empty slot before finding key
                }
                if entry.key_len as usize == key.len() && key_eq(&entry.key, key, key.len()) {
                    target = Some(idx);
                    break;
                }
                attempt = attempt.saturating_add(1);
            }

            let del_slot = match target {
                Some(s) => s,
                None => return -2,
            };

            // Repair probe chain: walk forward from del_slot, re-inserting any
            // entry whose natural slot would otherwise skip over the hole.
            hm.entries[del_slot].occupied = false;
            hm.count = hm.count.saturating_sub(1);

            let mut current = del_slot;
            let mut next = (current.saturating_add(1)) % (slots as usize);
            while hm.entries[next].occupied && next != del_slot {
                let entry_key_len = hm.entries[next].key_len as usize;
                let natural = fnv1a_hash(&hm.entries[next].key[..entry_key_len]) % slots;
                let natural_idx = natural as usize;
                // If the natural slot for next is at or before the hole, move it
                let should_move = if current < next {
                    natural_idx <= current || natural_idx > next
                } else {
                    natural_idx <= current && natural_idx > next
                };
                if should_move {
                    hm.entries[current] = hm.entries[next];
                    hm.entries[next].occupied = false;
                    current = next;
                }
                next = (next.saturating_add(1)) % (slots as usize);
            }
            0
        }

        BPF_MAP_TYPE_ARRAY => {
            if key.len() < 4 {
                return -22;
            }
            let idx = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]) as usize;
            let mut pool = ARRAY_MAPS.lock();
            let am = &mut pool[desc.type_idx];
            if !am.active || idx >= am.max_entries as usize {
                return -22;
            }
            if !am.entries[idx].initialized {
                return -2; // ENOENT
            }
            am.entries[idx].initialized = false;
            am.entries[idx].val_len = 0;
            0
        }

        _ => -22, // EINVAL / EOPNOTSUPP
    }
}

// ---------------------------------------------------------------------------
// bpf_map_get_next_key
// ---------------------------------------------------------------------------

/// Iterate the map: given the current `key`, return the next key in iteration
/// order.  If `key` is `None`, return the very first key.
///
/// Returns:
///   0   — success; `next` is populated
///  -2   — no more keys (ENOENT)
/// -22   — invalid fd or key (EINVAL)
///
/// Only HASH and ARRAY maps support iteration.  RINGBUF returns -22.
pub fn bpf_map_get_next_key(fd: i32, key: Option<&[u8]>, next: &mut [u8; BPF_KEY_SIZE]) -> i32 {
    let desc = match bpf_map_get_info(fd) {
        Some(d) => d,
        None => return -22,
    };

    match desc.map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let pool = HASH_MAPS.lock();
            let hm = &pool[desc.type_idx];
            if !hm.active {
                return -22;
            }

            let slots = hm.max_entries as usize;
            if slots == 0 {
                return -2;
            }

            match key {
                None => {
                    // Return the first occupied slot
                    let mut i = 0usize;
                    while i < slots {
                        if hm.entries[i].occupied {
                            let klen = hm.entries[i].key_len as usize;
                            let mut ki = 0usize;
                            while ki < klen {
                                next[ki] = hm.entries[i].key[ki];
                                ki = ki.saturating_add(1);
                            }
                            return 0;
                        }
                        i = i.saturating_add(1);
                    }
                    -2 // no entries at all
                }
                Some(k) => {
                    if k.len() != desc.key_size as usize {
                        return -22;
                    }
                    // Find current key's slot, then find the next occupied slot after it
                    let hash = fnv1a_hash(k);
                    let slots_u32 = hm.max_entries;
                    let mut found_slot: Option<usize> = None;
                    let mut attempt = 0u32;
                    while attempt < slots_u32 {
                        let idx = probe_slot(hash, attempt, slots_u32) as usize;
                        let entry = &hm.entries[idx];
                        if !entry.occupied {
                            break;
                        }
                        if entry.key_len as usize == k.len() && key_eq(&entry.key, k, k.len()) {
                            found_slot = Some(idx);
                            break;
                        }
                        attempt = attempt.saturating_add(1);
                    }

                    let start = match found_slot {
                        Some(s) => s.saturating_add(1),
                        None => return -2, // key not found; return first entry instead
                    };

                    let mut i = start;
                    while i < slots {
                        if hm.entries[i].occupied {
                            let klen = hm.entries[i].key_len as usize;
                            let mut ki = 0usize;
                            while ki < klen {
                                next[ki] = hm.entries[i].key[ki];
                                ki = ki.saturating_add(1);
                            }
                            return 0;
                        }
                        i = i.saturating_add(1);
                    }
                    -2 // no more keys
                }
            }
        }

        BPF_MAP_TYPE_ARRAY => {
            let pool = ARRAY_MAPS.lock();
            let am = &pool[desc.type_idx];
            if !am.active {
                return -22;
            }

            let max = am.max_entries as usize;

            // Determine starting index
            let start = match key {
                None => 0usize,
                Some(k) => {
                    if k.len() < 4 {
                        return -22;
                    }
                    let cur_idx = u32::from_ne_bytes([k[0], k[1], k[2], k[3]]) as usize;
                    cur_idx.saturating_add(1)
                }
            };

            // Find the next initialized entry at or after `start`
            let mut i = start;
            while i < max {
                if am.entries[i].initialized {
                    let idx_bytes = (i as u32).to_ne_bytes();
                    next[0] = idx_bytes[0];
                    next[1] = idx_bytes[1];
                    next[2] = idx_bytes[2];
                    next[3] = idx_bytes[3];
                    return 0;
                }
                i = i.saturating_add(1);
            }
            -2 // no more keys
        }

        _ => -22, // EINVAL
    }
}

// ---------------------------------------------------------------------------
// bpf_ringbuf_output
// ---------------------------------------------------------------------------

/// Write a variable-length record to the ring buffer identified by `fd`.
///
/// Each record is prefixed with an 8-byte header:
///   bytes 0-3: payload length as little-endian u32
///   bytes 4-7: padding / reserved (zeroed)
///
/// Returns:
///    0   — success
///  -22   — invalid fd or data too large to fit (EINVAL)
///  -11   — ring buffer is full, record dropped (EAGAIN)
pub fn bpf_ringbuf_output(fd: i32, data: &[u8]) -> i32 {
    let desc = match bpf_map_get_info(fd) {
        Some(d) => d,
        None => return -22,
    };

    if desc.map_type != BPF_MAP_TYPE_RINGBUF {
        return -22;
    }

    let record_len = BPF_RINGBUF_HDR.saturating_add(data.len());
    if record_len > BPF_RINGBUF_SIZE {
        return -22; // EINVAL — record exceeds entire buffer
    }

    let mut pool = RINGBUF_MAPS.lock();
    let rm = &mut pool[desc.type_idx];
    if !rm.active {
        return -22;
    }
    let rb = &mut rm.rb;

    if (rb.free() as usize) < record_len {
        rb.lost_count = rb.lost_count.saturating_add(1);
        return -11; // EAGAIN — full
    }

    // Write 8-byte header: length (u32 LE) + pad (u32 = 0)
    let len_bytes = (data.len() as u32).to_le_bytes();
    let mut hi = 0usize;
    while hi < 4 {
        let pos = (rb.tail.wrapping_add(hi as u32)) % (BPF_RINGBUF_SIZE as u32);
        rb.data[pos as usize] = len_bytes[hi];
        hi = hi.saturating_add(1);
    }
    // pad bytes (already 0 in the static, but write explicitly for correctness)
    while hi < 8 {
        let pos = (rb.tail.wrapping_add(hi as u32)) % (BPF_RINGBUF_SIZE as u32);
        rb.data[pos as usize] = 0;
        hi = hi.saturating_add(1);
    }

    // Write payload
    let mut di = 0usize;
    while di < data.len() {
        let pos = (rb
            .tail
            .wrapping_add((BPF_RINGBUF_HDR as u32).saturating_add(di as u32)))
            % (BPF_RINGBUF_SIZE as u32);
        rb.data[pos as usize] = data[di];
        di = di.saturating_add(1);
    }

    rb.tail = rb.tail.wrapping_add(record_len as u32);
    rb.sample_count = rb.sample_count.saturating_add(1);
    0
}

// ---------------------------------------------------------------------------
// bpf_ringbuf_read
// ---------------------------------------------------------------------------

/// Read the oldest record from the ring buffer.
///
/// The 8-byte header is consumed but not copied into `out`.
/// Returns the number of payload bytes copied (>= 0) on success,
/// or -11 (EAGAIN) if the buffer is empty, -22 (EINVAL) for bad fd.
pub fn bpf_ringbuf_read(fd: i32, out: &mut [u8; BPF_RINGBUF_SIZE]) -> isize {
    let desc = match bpf_map_get_info(fd) {
        Some(d) => d,
        None => return -22,
    };

    if desc.map_type != BPF_MAP_TYPE_RINGBUF {
        return -22;
    }

    let mut pool = RINGBUF_MAPS.lock();
    let rm = &mut pool[desc.type_idx];
    if !rm.active {
        return -22;
    }
    let rb = &mut rm.rb;

    // Need at least the header
    if rb.used() < BPF_RINGBUF_HDR as u32 {
        return -11; // EAGAIN
    }

    // Read payload length from header (little-endian u32)
    let mut hdr_bytes = [0u8; 4];
    let mut hi = 0usize;
    while hi < 4 {
        let pos = (rb.head.wrapping_add(hi as u32)) % (BPF_RINGBUF_SIZE as u32);
        hdr_bytes[hi] = rb.data[pos as usize];
        hi = hi.saturating_add(1);
    }
    let payload_len = u32::from_le_bytes(hdr_bytes) as usize;

    let record_len = BPF_RINGBUF_HDR.saturating_add(payload_len);
    if rb.used() < record_len as u32 {
        return -11; // EAGAIN — header was written but payload not yet
    }
    if payload_len > BPF_RINGBUF_SIZE {
        // Corrupt header — skip the entire (corrupted) entry width and fail
        rb.head = rb.head.wrapping_add(BPF_RINGBUF_HDR as u32);
        return -22; // EINVAL
    }

    // Copy payload into `out`
    let copy_len = if payload_len <= BPF_RINGBUF_SIZE {
        payload_len
    } else {
        BPF_RINGBUF_SIZE
    };
    let mut di = 0usize;
    while di < copy_len {
        let pos = (rb
            .head
            .wrapping_add((BPF_RINGBUF_HDR as u32).saturating_add(di as u32)))
            % (BPF_RINGBUF_SIZE as u32);
        out[di] = rb.data[pos as usize];
        di = di.saturating_add(1);
    }

    // Advance consumer pointer
    rb.head = rb.head.wrapping_add(record_len as u32);
    copy_len as isize
}

// ---------------------------------------------------------------------------
// bpf_map_is_fd / bpf_map_get_info / bpf_map_close
// ---------------------------------------------------------------------------

/// Return true if `fd` is in the BPF map fd range and corresponds to an
/// active map descriptor.
pub fn bpf_map_is_fd(fd: i32) -> bool {
    if fd < BPF_MAP_FD_BASE || fd >= BPF_MAP_FD_BASE.saturating_add(BPF_MAX_MAPS as i32) {
        return false;
    }
    let descs = MAP_DESCRIPTORS.lock();
    let idx = (fd - BPF_MAP_FD_BASE) as usize;
    if idx >= BPF_MAX_MAPS {
        return false;
    }
    descs[idx].active && descs[idx].fd == fd
}

/// Return a copy of the map descriptor for `fd`, or None if not active.
pub fn bpf_map_get_info(fd: i32) -> Option<BpfMapDesc> {
    if fd < BPF_MAP_FD_BASE {
        return None;
    }
    let descs = MAP_DESCRIPTORS.lock();
    let slot = find_desc_slot(&*descs, fd)?;
    Some(descs[slot])
}

/// Release a map: mark its descriptor and type-pool slot as inactive.
///
/// After this call the fd is invalid.  Calling code must not use the fd again.
pub fn bpf_map_close(fd: i32) {
    let descs_snapshot = {
        let mut descs = MAP_DESCRIPTORS.lock();
        match find_desc_slot(&*descs, fd) {
            None => return,
            Some(slot) => {
                let snap = descs[slot];
                descs[slot] = BpfMapDesc::empty();
                snap
            }
        }
    };

    // Now release the type-specific pool slot
    match descs_snapshot.map_type {
        BPF_MAP_TYPE_HASH | BPF_MAP_TYPE_LRU_HASH => {
            let mut pool = HASH_MAPS.lock();
            let idx = descs_snapshot.type_idx;
            if idx < BPF_MAX_HASH_MAPS {
                pool[idx] = BpfHashMap::empty();
            }
        }
        BPF_MAP_TYPE_ARRAY => {
            let mut pool = ARRAY_MAPS.lock();
            let idx = descs_snapshot.type_idx;
            if idx < BPF_MAX_ARRAY_MAPS {
                pool[idx] = BpfArrayMap::empty();
            }
        }
        BPF_MAP_TYPE_RINGBUF => {
            let mut pool = RINGBUF_MAPS.lock();
            let idx = descs_snapshot.type_idx;
            if idx < BPF_MAX_RINGBUF_MAPS {
                pool[idx] = BpfRingbufMap::empty();
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// sys_bpf — syscall entry point
// ---------------------------------------------------------------------------

/// Entry point for the `bpf(2)` system call (SYS_BPF = 331).
///
/// Calling convention (matches `syscall_dispatch`):
///   cmd  = arg1 (u64): BPF_MAP_CREATE | BPF_MAP_LOOKUP_ELEM | ...
///   arg2 = fd or map_type depending on command
///   arg3..arg6 = command-specific arguments
///
/// All user-pointer arguments must have already been validated by the
/// caller (`check_user_ptr!` in `syscall_dispatch`) before reaching here.
/// This function operates entirely on kernel-resident copies.
///
/// Returns 0 or a positive fd on success; a wrapped negative errno
/// (as u64 two's-complement) on failure.
pub fn sys_bpf(cmd: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64, _arg6: u64) -> u64 {
    match cmd {
        // BPF_MAP_CREATE:
        //   arg2 = map_type (u32)
        //   arg3 = key_size (u32)
        //   arg4 = value_size (u32)
        //   arg5 = max_entries (u32)
        //   flags passed as upper 32 bits of arg5 for simplicity, or 0
        BPF_MAP_CREATE => {
            let map_type = arg2 as u32;
            let key_size = arg3 as u32;
            let value_size = arg4 as u32;
            let max_entries = (arg5 & 0xFFFF_FFFF) as u32;
            let flags = (arg5 >> 32) as u32;
            let fd = bpf_map_create(map_type, key_size, value_size, max_entries, flags);
            fd as u64
        }

        // BPF_MAP_LOOKUP_ELEM:
        //   arg2 = fd
        //   arg3 = key_ptr  (pointer to key_size bytes in kernel address space)
        //   arg4 = out_ptr  (pointer to BPF_VAL_SIZE-byte output buffer)
        BPF_MAP_LOOKUP_ELEM => {
            let fd = arg2 as i32;
            let key_ptr = arg3 as *const u8;
            let out_ptr = arg4 as *mut u8;
            if key_ptr.is_null() || out_ptr.is_null() {
                return 0xFFFF_FFFF_FFFF_FFEA; // -22 EINVAL
            }
            let desc = match bpf_map_get_info(fd) {
                Some(d) => d,
                None => return 0xFFFF_FFFF_FFFF_FFEA,
            };
            let key_size = desc.key_size as usize;
            if key_size > BPF_KEY_SIZE {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let key_slice = unsafe { core::slice::from_raw_parts(key_ptr, key_size) };
            match bpf_map_lookup(fd, key_slice) {
                None => 0xFFFF_FFFF_FFFF_FFFE, // -2 ENOENT
                Some(val) => {
                    let val_size = desc.value_size as usize;
                    let out_slice = unsafe { core::slice::from_raw_parts_mut(out_ptr, val_size) };
                    let mut vi = 0usize;
                    while vi < val_size {
                        out_slice[vi] = val[vi];
                        vi = vi.saturating_add(1);
                    }
                    0
                }
            }
        }

        // BPF_MAP_UPDATE_ELEM:
        //   arg2 = fd
        //   arg3 = key_ptr
        //   arg4 = value_ptr
        //   arg5 = flags (BPF_ANY / BPF_NOEXIST / BPF_EXIST)
        BPF_MAP_UPDATE_ELEM => {
            let fd = arg2 as i32;
            let key_ptr = arg3 as *const u8;
            let value_ptr = arg4 as *const u8;
            let flags_val = arg5;
            if key_ptr.is_null() || value_ptr.is_null() {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let desc = match bpf_map_get_info(fd) {
                Some(d) => d,
                None => return 0xFFFF_FFFF_FFFF_FFEA,
            };
            let key_size = desc.key_size as usize;
            let val_size = desc.value_size as usize;
            if key_size > BPF_KEY_SIZE || val_size > BPF_VAL_SIZE {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let key_slice = unsafe { core::slice::from_raw_parts(key_ptr, key_size) };
            let val_slice = unsafe { core::slice::from_raw_parts(value_ptr, val_size) };
            let ret = bpf_map_update(fd, key_slice, val_slice, flags_val);
            ret as u64
        }

        // BPF_MAP_DELETE_ELEM:
        //   arg2 = fd
        //   arg3 = key_ptr
        BPF_MAP_DELETE_ELEM => {
            let fd = arg2 as i32;
            let key_ptr = arg3 as *const u8;
            if key_ptr.is_null() {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let desc = match bpf_map_get_info(fd) {
                Some(d) => d,
                None => return 0xFFFF_FFFF_FFFF_FFEA,
            };
            let key_size = desc.key_size as usize;
            if key_size > BPF_KEY_SIZE {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let key_slice = unsafe { core::slice::from_raw_parts(key_ptr, key_size) };
            let ret = bpf_map_delete(fd, key_slice);
            ret as u64
        }

        // BPF_MAP_GET_NEXT_KEY:
        //   arg2 = fd
        //   arg3 = key_ptr  (may be 0 = None, meaning "first key")
        //   arg4 = next_key_ptr (output, BPF_KEY_SIZE bytes)
        BPF_MAP_GET_NEXT_KEY => {
            let fd = arg2 as i32;
            let key_ptr = arg3 as *const u8;
            let next_key_ptr = arg4 as *mut u8;
            if next_key_ptr.is_null() {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }
            let desc = match bpf_map_get_info(fd) {
                Some(d) => d,
                None => return 0xFFFF_FFFF_FFFF_FFEA,
            };
            let key_size = desc.key_size as usize;
            if key_size > BPF_KEY_SIZE {
                return 0xFFFF_FFFF_FFFF_FFEA;
            }

            let mut next_buf = [0u8; BPF_KEY_SIZE];
            let ret = if key_ptr.is_null() {
                bpf_map_get_next_key(fd, None, &mut next_buf)
            } else {
                let key_slice = unsafe { core::slice::from_raw_parts(key_ptr, key_size) };
                bpf_map_get_next_key(fd, Some(key_slice), &mut next_buf)
            };

            if ret == 0 {
                let out_slice = unsafe { core::slice::from_raw_parts_mut(next_key_ptr, key_size) };
                let mut ki = 0usize;
                while ki < key_size {
                    out_slice[ki] = next_buf[ki];
                    ki = ki.saturating_add(1);
                }
            }
            ret as u64
        }

        _ => 0xFFFF_FFFF_FFFF_FFEA, // -22 EINVAL — unknown BPF command
    }
}
