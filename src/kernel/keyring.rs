/// Kernel keyring and key management — Genesis AIOS
///
/// Provides a Linux-inspired kernel keyring subsystem for managing
/// cryptographic keys and other small secrets within the kernel.
///
/// Key concepts:
///   - Every key has a unique 32-bit ID assigned at allocation time.
///   - A keyring is a special key of type `KEY_TYPE_KEYRING` that links to
///     other keys.
///   - Keys are reference-counted in a simplified way: `key_put()` marks the
///     key as revoked; revoked keys are excluded from searches.
///   - Permissions are a 4-bit field: read | write | search | link.
///
/// Rules enforced:
///   - No heap (no Vec / Box / String / alloc::*)
///   - No float casts
///   - No panic (no unwrap / expect)
///   - Saturating arithmetic on all counters
///   - Wrapping arithmetic on sequence / ID numbers (NEXT_KEY_ID)
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of keys that can coexist in the kernel keyring.
pub const MAX_KEYS: usize = 128;

/// Maximum byte size of a key payload.
pub const MAX_KEY_PAYLOAD: usize = 512;

/// Maximum number of key links a single keyring entry can hold.
pub const MAX_KEYRING_LINKS: usize = 32;

// Key type identifiers.
pub const KEY_TYPE_USER: u8 = 0;
pub const KEY_TYPE_LOGON: u8 = 1;
pub const KEY_TYPE_KEYRING: u8 = 2;
pub const KEY_TYPE_ASYMMETRIC: u8 = 3;

// Permission bits.
pub const KEY_PERM_READ: u32 = 1;
pub const KEY_PERM_WRITE: u32 = 2;
pub const KEY_PERM_SEARCH: u32 = 4;
pub const KEY_PERM_LINK: u32 = 8;
pub const KEY_PERM_ALL: u32 = 0xF;

// Sentinel used to mark "no keyring" in KeyringLinks.
const NULL_KEY_ID: u32 = 0;

// ---------------------------------------------------------------------------
// Key record
// ---------------------------------------------------------------------------

/// A single kernel key entry.
#[derive(Clone, Copy)]
pub struct Key {
    /// Unique kernel-assigned identifier.
    pub id: u32,
    /// One of the `KEY_TYPE_*` constants.
    pub key_type: u8,
    /// Owner user ID.
    pub uid: u32,
    /// Owner group ID.
    pub gid: u32,
    /// Permission bitmask (see `KEY_PERM_*`).
    pub perm: u32,
    /// Human-readable description / name.
    pub description: [u8; 64],
    /// Valid bytes in `description`.
    pub desc_len: u8,
    /// Raw payload bytes.
    pub payload: [u8; MAX_KEY_PAYLOAD],
    /// Valid bytes in `payload`.
    pub payload_len: u16,
    /// Set by `key_revoke()`.  Revoked keys cannot be used.
    pub revoked: bool,
    /// `false` means the slot is free.
    pub active: bool,
}

impl Key {
    /// Construct an empty / unused key slot.  `const fn` for static init.
    pub const fn empty() -> Self {
        Key {
            id: 0,
            key_type: KEY_TYPE_USER,
            uid: 0,
            gid: 0,
            perm: 0,
            description: [0u8; 64],
            desc_len: 0,
            payload: [0u8; MAX_KEY_PAYLOAD],
            payload_len: 0,
            revoked: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// KeyringLinks — the set of keys linked into a keyring
// ---------------------------------------------------------------------------

/// Tracks which keys belong to a particular keyring key.
#[derive(Clone, Copy)]
pub struct KeyringLinks {
    /// ID of the keyring key that owns these links.
    pub keyring_id: u32,
    /// IDs of linked keys (0 = empty slot).
    pub key_ids: [u32; MAX_KEYRING_LINKS],
    /// Number of active links.
    pub nkeys: u8,
    /// `false` means this slot is free.
    pub active: bool,
}

impl KeyringLinks {
    /// Construct an empty / unused keyring-links slot.
    pub const fn empty() -> Self {
        KeyringLinks {
            keyring_id: 0,
            key_ids: [0u32; MAX_KEYRING_LINKS],
            nkeys: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

/// The global key table.
static KEYS: Mutex<[Key; MAX_KEYS]> = Mutex::new([Key::empty(); MAX_KEYS]);

/// Per-keyring link tables (one entry per keyring key, max 16 keyrings).
static KEYRING_LINKS: Mutex<[KeyringLinks; 16]> = Mutex::new([KeyringLinks::empty(); 16]);

/// Monotonically increasing key ID counter.  Uses wrapping increment so it
/// never overflows.  Starts at 1; 0 is reserved as "no key".
static NEXT_KEY_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy at most `MAX_VAR=64` bytes from `src` into a `[u8; 64]` buffer.
/// Returns the number of bytes actually copied.
#[inline]
fn copy_desc(dst: &mut [u8; 64], src: &[u8]) -> u8 {
    let len = if src.len() > 64 { 64 } else { src.len() };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

/// Compare `needle` against a key's description field.
#[inline]
fn desc_matches(key: &Key, needle: &[u8]) -> bool {
    if needle.len() > 64 {
        return false;
    }
    if key.desc_len as usize != needle.len() {
        return false;
    }
    let len = key.desc_len as usize;
    let mut i = 0usize;
    while i < len {
        if key.description[i] != needle[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public key API
// ---------------------------------------------------------------------------

/// Allocate a new key slot and return its ID.
///
/// The key starts with an empty payload; call `key_instantiate()` to set it.
///
/// Returns `None` if the key table is full.
pub fn key_alloc(key_type: u8, description: &[u8], uid: u32, gid: u32, perm: u32) -> Option<u32> {
    let id = NEXT_KEY_ID.fetch_add(1, Ordering::Relaxed).wrapping_add(0);
    // Ensure we never hand out ID 0.
    let id = if id == 0 {
        NEXT_KEY_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    };

    let mut keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        if !keys[i].active {
            let slot = &mut keys[i];
            slot.id = id;
            slot.key_type = key_type;
            slot.uid = uid;
            slot.gid = gid;
            slot.perm = perm;
            slot.desc_len = copy_desc(&mut slot.description, description);
            // payload stays zeroed / empty until key_instantiate() is called.
            slot.payload_len = 0;
            slot.revoked = false;
            slot.active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Simplified `key_put`: mark the key as revoked (decrement-refcount stub).
///
/// The slot remains allocated but is inaccessible via search / read until
/// `key_revoke()` frees it.  This is intentionally simplified — a full
/// implementation would track reference counts.
pub fn key_put(id: u32) {
    let mut keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        if keys[i].active && keys[i].id == id {
            keys[i].revoked = true;
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Instantiate a key by setting its initial payload.
///
/// Returns `false` if:
/// - `id` is not found or the key is revoked
/// - `payload.len() > MAX_KEY_PAYLOAD`
pub fn key_instantiate(id: u32, payload: &[u8]) -> bool {
    if payload.len() > MAX_KEY_PAYLOAD {
        return false;
    }
    let mut keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        let k = &mut keys[i];
        if k.active && !k.revoked && k.id == id {
            let plen = payload.len();
            let mut j = 0usize;
            while j < plen {
                k.payload[j] = payload[j];
                j = j.saturating_add(1);
            }
            k.payload_len = plen as u16;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Update the payload of an existing key.
///
/// The key must have `KEY_PERM_WRITE` in its permission mask.
///
/// Returns `false` if:
/// - `id` is not found or the key is revoked
/// - The key lacks write permission
/// - `payload.len() > MAX_KEY_PAYLOAD`
pub fn key_update(id: u32, payload: &[u8]) -> bool {
    if payload.len() > MAX_KEY_PAYLOAD {
        return false;
    }
    let mut keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        let k = &mut keys[i];
        if k.active && !k.revoked && k.id == id {
            if k.perm & KEY_PERM_WRITE == 0 {
                return false;
            }
            let plen = payload.len();
            let mut j = 0usize;
            while j < plen {
                k.payload[j] = payload[j];
                j = j.saturating_add(1);
            }
            k.payload_len = plen as u16;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Read the payload of a key into `out`.
///
/// On success writes the payload bytes into `out` and sets `*out_len`.
///
/// Returns `false` if the key is not found, is revoked, or lacks
/// `KEY_PERM_READ`.
pub fn key_read(id: u32, out: &mut [u8; MAX_KEY_PAYLOAD], out_len: &mut u16) -> bool {
    let keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        let k = &keys[i];
        if k.active && !k.revoked && k.id == id {
            if k.perm & KEY_PERM_READ == 0 {
                return false;
            }
            let plen = k.payload_len as usize;
            let mut j = 0usize;
            while j < plen {
                out[j] = k.payload[j];
                j = j.saturating_add(1);
            }
            *out_len = k.payload_len;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Revoke a key.  The slot is marked `revoked=true`; subsequent operations
/// that require a non-revoked key will fail.
///
/// Returns `false` if the key is not found.
pub fn key_revoke(id: u32) -> bool {
    let mut keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        let k = &mut keys[i];
        if k.active && k.id == id {
            k.revoked = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Search for a key by its description.
///
/// Returns the ID of the first non-revoked key whose description matches
/// `description`, or `None` if no match is found.
///
/// Only keys with `KEY_PERM_SEARCH` set are considered.
pub fn key_search(description: &[u8]) -> Option<u32> {
    if description.is_empty() || description.len() > 64 {
        return None;
    }
    let keys = KEYS.lock();
    let mut i = 0usize;
    while i < MAX_KEYS {
        let k = &keys[i];
        if k.active && !k.revoked && k.perm & KEY_PERM_SEARCH != 0 {
            if desc_matches(k, description) {
                return Some(k.id);
            }
        }
        i = i.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Public keyring link API
// ---------------------------------------------------------------------------

/// Link `key_id` into the keyring identified by `keyring_id`.
///
/// Returns `false` if:
/// - `keyring_id` does not correspond to an active `KEY_TYPE_KEYRING` key
/// - The keyring already has `MAX_KEYRING_LINKS` links
/// - No `KeyringLinks` slot is available for a new keyring
pub fn keyring_link(keyring_id: u32, key_id: u32) -> bool {
    // Verify that keyring_id is a real KEYRING-type key.
    {
        let keys = KEYS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_KEYS {
            let k = &keys[i];
            if k.active && !k.revoked && k.id == keyring_id && k.key_type == KEY_TYPE_KEYRING {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return false;
        }
    }

    let mut links = KEYRING_LINKS.lock();

    // Find an existing KeyringLinks entry for this keyring, or a free slot.
    let mut existing_idx = 16usize; // sentinel
    let mut free_idx = 16usize; // sentinel

    let mut i = 0usize;
    while i < 16 {
        if links[i].active && links[i].keyring_id == keyring_id {
            existing_idx = i;
            break;
        }
        if !links[i].active && free_idx == 16 {
            free_idx = i;
        }
        i = i.saturating_add(1);
    }

    let slot_idx = if existing_idx < 16 {
        existing_idx
    } else if free_idx < 16 {
        // Initialise a fresh slot.
        links[free_idx].keyring_id = keyring_id;
        links[free_idx].nkeys = 0;
        links[free_idx].active = true;
        // Zero out key_ids (they should already be zero but be explicit).
        let mut j = 0usize;
        while j < MAX_KEYRING_LINKS {
            links[free_idx].key_ids[j] = NULL_KEY_ID;
            j = j.saturating_add(1);
        }
        free_idx
    } else {
        return false; // No slot available.
    };

    let entry = &mut links[slot_idx];

    // Check for duplicate link.
    let mut j = 0usize;
    while j < MAX_KEYRING_LINKS {
        if entry.key_ids[j] == key_id {
            return true; // Already linked — idempotent success.
        }
        j = j.saturating_add(1);
    }

    // Find a free slot in the key_ids array.
    let mut j = 0usize;
    while j < MAX_KEYRING_LINKS {
        if entry.key_ids[j] == NULL_KEY_ID {
            entry.key_ids[j] = key_id;
            entry.nkeys = entry.nkeys.saturating_add(1);
            return true;
        }
        j = j.saturating_add(1);
    }

    false // Keyring is full.
}

/// Unlink `key_id` from the keyring identified by `keyring_id`.
///
/// Returns `false` if the keyring or the link is not found.
pub fn keyring_unlink(keyring_id: u32, key_id: u32) -> bool {
    let mut links = KEYRING_LINKS.lock();
    let mut i = 0usize;
    while i < 16 {
        let entry = &mut links[i];
        if entry.active && entry.keyring_id == keyring_id {
            let mut j = 0usize;
            while j < MAX_KEYRING_LINKS {
                if entry.key_ids[j] == key_id {
                    entry.key_ids[j] = NULL_KEY_ID;
                    entry.nkeys = entry.nkeys.saturating_sub(1);
                    return true;
                }
                j = j.saturating_add(1);
            }
            return false; // keyring found but key not linked.
        }
        i = i.saturating_add(1);
    }
    false
}

/// Create a new keyring key and a corresponding `KeyringLinks` entry.
///
/// Returns the new keyring's key ID, or `None` if either the key table or
/// the `KeyringLinks` table is full.
pub fn keyring_create(description: &[u8], uid: u32) -> Option<u32> {
    // Allocate the keyring key.
    let id = key_alloc(
        KEY_TYPE_KEYRING,
        description,
        uid,
        uid, // gid == uid for simplicity
        KEY_PERM_ALL,
    )?;

    // Allocate a KeyringLinks entry for this new keyring.
    let mut links = KEYRING_LINKS.lock();
    let mut i = 0usize;
    while i < 16 {
        if !links[i].active {
            links[i].keyring_id = id;
            links[i].nkeys = 0;
            links[i].active = true;
            let mut j = 0usize;
            while j < MAX_KEYRING_LINKS {
                links[i].key_ids[j] = NULL_KEY_ID;
                j = j.saturating_add(1);
            }
            return Some(id);
        }
        i = i.saturating_add(1);
    }

    // No KeyringLinks slot available: revoke the key we just allocated and
    // report failure.
    drop(links);
    key_revoke(id);
    None
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the kernel keyring subsystem.
///
/// Creates the default `.system_keyring` keyring that is available to all
/// kernel subsystems.
pub fn init() {
    if keyring_create(b".system_keyring", 0).is_none() {
        // Log but do not panic — the kernel can still function without the
        // system keyring in degraded mode.
        serial_println!("[keyring] WARNING: failed to create .system_keyring");
    }
    serial_println!("[keyring] key management initialized");
}
