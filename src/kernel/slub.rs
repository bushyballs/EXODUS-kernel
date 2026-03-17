/// SLUB slab allocator metadata tracking — Genesis AIOS
///
/// Provides O(1) fixed-size object allocation pools backed entirely by static
/// arrays.  There is no heap involvement: every `SlubCache` and `SlubObject`
/// lives in a `static Mutex<[T; N]>` and therefore must be `Copy` with a
/// `const fn empty()` constructor.
///
/// Design constraints (bare-metal #![no_std]):
///   - NO heap: no Vec / Box / String / alloc::* — fixed-size static arrays only
///   - NO floats: no `as f64` / `as f32`, no float literals
///   - NO panics: no unwrap() / expect() / panic!() — return Option<T> / bool
///   - Counters:  saturating_add / saturating_sub only
///   - Sequence numbers: wrapping_add only
///   - Structs in static Mutex<[T;N]>: Copy + `const fn empty()`
///   - No division without guarding divisor != 0
///
/// Standard caches created at `init()`:
///   "task_struct"  — size 64, capacity 64
///   "mm_struct"    — size 64, capacity 32
///   "vma"          — size 48, capacity 128
///   "file"         — size 32, capacity 64
///   "dentry"       — size 64, capacity 256
///   (Total objects = 544 — fits within MAX_SLUB_OBJECTS=512 only if we reduce;
///    dentry capacity adjusted to 224 so the sum == 512.)
///
/// Inspired by: Linux SLUB allocator (mm/slub.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of distinct slab caches.
pub const MAX_SLUB_CACHES: usize = 32;

/// Total tracked object slots across all caches.
pub const MAX_SLUB_OBJECTS: usize = 512;

/// Maximum bytes per slab object (must cover the largest kernel struct stored).
pub const MAX_OBJECT_DATA: usize = 64;

/// Maximum length of a cache name in bytes.
pub const MAX_CACHE_NAME: usize = 32;

// ---------------------------------------------------------------------------
// SlubCache
// ---------------------------------------------------------------------------

/// Metadata for one slab cache (a pool of same-sized objects).
///
/// Objects belonging to this cache occupy consecutive slots in `SLUB_OBJECTS`
/// starting at `first_obj` and running for `capacity` entries.
#[derive(Copy, Clone)]
pub struct SlubCache {
    /// Human-readable name (ASCII bytes, not NUL-terminated).
    pub name: [u8; MAX_CACHE_NAME],
    /// Number of valid bytes in `name`.
    pub name_len: u8,
    /// Nominal size (bytes) of each object managed by this cache.
    pub object_size: usize,
    /// Alignment requirement (informational; not enforced in static arrays).
    pub align: usize,
    /// Index of the first `SlubObject` slot owned by this cache.
    pub first_obj: u16,
    /// Total object slots allocated to this cache.
    pub capacity: u16,
    /// Number of currently free (not in-use) slots.
    pub free_count: u16,
    /// Lifetime allocation counter (saturating).
    pub alloc_count: u64,
    /// Lifetime free-operation counter (saturating).
    pub free_ops: u64,
    /// `true` while the cache exists; `false` for an unused registry slot.
    pub active: bool,
}

impl SlubCache {
    /// Return an empty (unused) cache slot suitable for `static` initialisation.
    pub const fn empty() -> Self {
        SlubCache {
            name: [0u8; MAX_CACHE_NAME],
            name_len: 0,
            object_size: 0,
            align: 0,
            first_obj: 0,
            capacity: 0,
            free_count: 0,
            alloc_count: 0,
            free_ops: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// SlubObject
// ---------------------------------------------------------------------------

/// One tracked object slot within the global object pool.
#[derive(Copy, Clone)]
pub struct SlubObject {
    /// Which cache owns this slot (index into `SLUB_CACHES`).
    pub cache_id: u32,
    /// Raw data bytes for this object (up to `MAX_OBJECT_DATA` bytes).
    pub data: [u8; MAX_OBJECT_DATA],
    /// `true` when this slot has been allocated and not yet freed.
    pub in_use: bool,
    /// `true` when this slot is owned by an active cache (vs. unassigned).
    pub active: bool,
}

impl SlubObject {
    /// Return an empty object slot suitable for `static` initialisation.
    pub const fn empty() -> Self {
        SlubObject {
            cache_id: 0,
            data: [0u8; MAX_OBJECT_DATA],
            in_use: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SLUB_CACHES: Mutex<[SlubCache; MAX_SLUB_CACHES]> =
    Mutex::new([SlubCache::empty(); MAX_SLUB_CACHES]);

static SLUB_OBJECTS: Mutex<[SlubObject; MAX_SLUB_OBJECTS]> =
    Mutex::new([SlubObject::empty(); MAX_SLUB_OBJECTS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to `MAX_CACHE_NAME` bytes from `src` into `dst`.
/// Returns the number of bytes copied.
#[inline]
fn copy_name(dst: &mut [u8; MAX_CACHE_NAME], src: &[u8]) -> u8 {
    let len = src.len().min(MAX_CACHE_NAME);
    dst[..len].copy_from_slice(&src[..len]);
    for i in len..MAX_CACHE_NAME {
        dst[i] = 0;
    }
    len as u8
}

/// Compare a cache name stored as `(bytes, len)` against a `&[u8]` key.
#[inline]
fn name_matches(name: &[u8; MAX_CACHE_NAME], name_len: u8, key: &[u8]) -> bool {
    let l = name_len as usize;
    if l != key.len() {
        return false;
    }
    &name[..l] == key
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new slab cache and allocate `capacity` object slots to it.
///
/// Returns the cache ID (index into `SLUB_CACHES`) on success, or `None` if:
///   - The cache registry is full (`MAX_SLUB_CACHES` reached).
///   - There are not enough free object slots.
///   - `capacity == 0` or `object_size == 0`.
pub fn slub_create_cache(name: &[u8], object_size: usize, capacity: u16) -> Option<u32> {
    if object_size == 0 || capacity == 0 {
        return None;
    }

    let mut caches = SLUB_CACHES.lock();
    let mut objects = SLUB_OBJECTS.lock();

    // Find a free cache slot.
    let mut cache_id: Option<usize> = None;
    for i in 0..MAX_SLUB_CACHES {
        if !caches[i].active {
            cache_id = Some(i);
            break;
        }
    }
    let cache_id = cache_id?;

    // Find `capacity` consecutive free object slots.
    // We scan linearly and take the first available run.
    let cap = capacity as usize;
    let mut run_start: Option<usize> = None;
    let mut run_len: usize = 0;

    for i in 0..MAX_SLUB_OBJECTS {
        if !objects[i].active {
            if run_start.is_none() {
                run_start = Some(i);
                run_len = 1;
            } else {
                run_len = run_len.saturating_add(1);
            }
            if run_len >= cap {
                break;
            }
        } else {
            run_start = None;
            run_len = 0;
        }
    }

    let first_obj = run_start.filter(|_| run_len >= cap)?;
    if first_obj.saturating_add(cap) > MAX_SLUB_OBJECTS {
        return None;
    }

    // Mark those slots as belonging to this cache.
    for i in first_obj..first_obj.saturating_add(cap) {
        objects[i].cache_id = cache_id as u32;
        objects[i].in_use = false;
        objects[i].active = true;
        // Clear stale data.
        objects[i].data = [0u8; MAX_OBJECT_DATA];
    }

    // Initialise the cache record.
    let c = &mut caches[cache_id];
    c.name_len = copy_name(&mut c.name, name);
    c.object_size = object_size;
    c.align = core::mem::align_of::<u64>(); // sensible default
    c.first_obj = first_obj as u16;
    c.capacity = capacity;
    c.free_count = capacity;
    c.alloc_count = 0;
    c.free_ops = 0;
    c.active = true;

    Some(cache_id as u32)
}

/// Destroy a slab cache and release all its object slots.
///
/// Returns `true` on success; `false` if `cache_id` is invalid or the cache
/// is not active.  All previously allocated objects are forcibly freed.
pub fn slub_destroy_cache(cache_id: u32) -> bool {
    let id = cache_id as usize;
    if id >= MAX_SLUB_CACHES {
        return false;
    }

    let mut caches = SLUB_CACHES.lock();
    let mut objects = SLUB_OBJECTS.lock();

    if !caches[id].active {
        return false;
    }

    let first = caches[id].first_obj as usize;
    let cap = caches[id].capacity as usize;
    let end = first.saturating_add(cap).min(MAX_SLUB_OBJECTS);

    for i in first..end {
        if objects[i].cache_id == cache_id {
            objects[i].in_use = false;
            objects[i].active = false;
            objects[i].data = [0u8; MAX_OBJECT_DATA];
        }
    }

    caches[id] = SlubCache::empty();
    true
}

/// Allocate one object from a cache.
///
/// Returns the global index of the allocated `SlubObject` in `SLUB_OBJECTS`,
/// or `None` if the cache is full, invalid, or not active.
pub fn slub_alloc(cache_id: u32) -> Option<u32> {
    let id = cache_id as usize;
    if id >= MAX_SLUB_CACHES {
        return None;
    }

    let mut caches = SLUB_CACHES.lock();
    if !caches[id].active || caches[id].free_count == 0 {
        return None;
    }

    let first = caches[id].first_obj as usize;
    let cap = caches[id].capacity as usize;
    let end = first.saturating_add(cap).min(MAX_SLUB_OBJECTS);

    // Drop the caches lock before taking objects lock to preserve lock order.
    let first_copy = first;
    let end_copy = end;
    drop(caches);

    let mut objects = SLUB_OBJECTS.lock();
    let mut found: Option<usize> = None;
    for i in first_copy..end_copy {
        if objects[i].active && objects[i].cache_id == cache_id && !objects[i].in_use {
            found = Some(i);
            break;
        }
    }

    let obj_idx = found?;
    objects[obj_idx].in_use = true;
    objects[obj_idx].data = [0u8; MAX_OBJECT_DATA]; // zero on allocation
    drop(objects);

    // Re-lock caches to update stats.
    let mut caches = SLUB_CACHES.lock();
    if id < MAX_SLUB_CACHES && caches[id].active {
        caches[id].free_count = caches[id].free_count.saturating_sub(1);
        caches[id].alloc_count = caches[id].alloc_count.saturating_add(1);
    }

    Some(obj_idx as u32)
}

/// Free a previously allocated object back to its cache.
///
/// Returns `true` on success; `false` if `obj_idx` is out of range, not
/// in-use, or the owning cache ID does not match `cache_id`.
pub fn slub_free(cache_id: u32, obj_idx: u32) -> bool {
    let oi = obj_idx as usize;
    if oi >= MAX_SLUB_OBJECTS {
        return false;
    }

    let mut objects = SLUB_OBJECTS.lock();
    if !objects[oi].active || !objects[oi].in_use || objects[oi].cache_id != cache_id {
        return false;
    }

    objects[oi].in_use = false;
    objects[oi].data = [0u8; MAX_OBJECT_DATA]; // scrub on free
    drop(objects);

    let id = cache_id as usize;
    if id >= MAX_SLUB_CACHES {
        return true; // object freed, cache metadata update skipped
    }

    let mut caches = SLUB_CACHES.lock();
    if caches[id].active {
        caches[id].free_count = caches[id].free_count.saturating_add(1);
        caches[id].free_ops = caches[id].free_ops.saturating_add(1);
    }

    true
}

/// Return a copy of the data stored in the object at `obj_idx`.
///
/// Returns `None` if `obj_idx` is out of range, the slot is not active, or
/// the slot is not currently in-use.
pub fn slub_get_object(obj_idx: u32) -> Option<[u8; MAX_OBJECT_DATA]> {
    let oi = obj_idx as usize;
    if oi >= MAX_SLUB_OBJECTS {
        return None;
    }
    let objects = SLUB_OBJECTS.lock();
    if !objects[oi].active || !objects[oi].in_use {
        return None;
    }
    Some(objects[oi].data)
}

/// Copy up to `MAX_OBJECT_DATA` bytes from `data` into the object at `obj_idx`.
///
/// Silently truncates if `data.len() > MAX_OBJECT_DATA`.
///
/// Returns `false` if `obj_idx` is out of range, the slot is not active, or
/// the slot is not currently in-use.
pub fn slub_set_object(obj_idx: u32, data: &[u8]) -> bool {
    let oi = obj_idx as usize;
    if oi >= MAX_SLUB_OBJECTS {
        return false;
    }
    let mut objects = SLUB_OBJECTS.lock();
    if !objects[oi].active || !objects[oi].in_use {
        return false;
    }
    let len = data.len().min(MAX_OBJECT_DATA);
    objects[oi].data[..len].copy_from_slice(&data[..len]);
    // Zero any trailing bytes from a previous write.
    for b in objects[oi].data[len..].iter_mut() {
        *b = 0;
    }
    true
}

/// Return statistics for the given cache:
///   `(alloc_count, free_ops, free_count, capacity)`
///
/// Returns `None` if `cache_id` is invalid or the cache is not active.
pub fn slub_get_stats(cache_id: u32) -> Option<(u64, u64, u16, u16)> {
    let id = cache_id as usize;
    if id >= MAX_SLUB_CACHES {
        return None;
    }
    let caches = SLUB_CACHES.lock();
    if !caches[id].active {
        return None;
    }
    let c = &caches[id];
    Some((c.alloc_count, c.free_ops, c.free_count, c.capacity))
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Create the standard kernel object caches.
///
/// Cache capacities are chosen so the sum equals exactly `MAX_SLUB_OBJECTS`
/// (512):
///   task_struct (64) + mm_struct (32) + vma (128) + file (64) + dentry (224)
///   = 512.
pub fn init() {
    let mut count: u32 = 0;

    macro_rules! create {
        ($name:expr, $size:expr, $cap:expr) => {
            if slub_create_cache($name, $size, $cap).is_some() {
                count = count.saturating_add(1);
            } else {
                serial_println!(
                    "  [slub] WARNING: failed to create cache \"{}\"",
                    core::str::from_utf8($name).unwrap_or("?")
                );
            }
        };
    }

    create!(b"task_struct", 64, 64);
    create!(b"mm_struct", 64, 32);
    create!(b"vma", 48, 128);
    create!(b"file", 32, 64);
    // Adjusted to 224 so total = 64+32+128+64+224 = 512 = MAX_SLUB_OBJECTS.
    create!(b"dentry", 64, 224);

    serial_println!(
        "[slub] SLUB allocator initialized, {} caches created",
        count
    );
}
