/// Live kernel patching without reboot.
///
/// Part of the AIOS kernel.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Describes a single function replacement in a live patch.
pub struct PatchEntry {
    pub target_symbol: String,
    pub original_addr: usize,
    pub replacement_addr: usize,
}

/// Number of bytes saved per patched site (enough for a 64-bit indirect JMP).
const PATCH_SAVE_LEN: usize = 14;

/// A live patch bundle containing one or more function replacements.
pub struct LivePatch {
    /// Unique patch identifier assigned at registration time.
    pub id: u32,
    pub name: String,
    pub entries: Vec<PatchEntry>,
    pub applied: bool,
    /// Original instruction bytes saved before patching (parallel to entries).
    saved_bytes: Vec<[u8; PATCH_SAVE_LEN]>,
}

impl LivePatch {
    pub fn new(name: &str) -> Self {
        LivePatch {
            id: 0,
            name: String::from(name),
            entries: Vec::new(),
            applied: false,
            saved_bytes: Vec::new(),
        }
    }

    /// Apply the patch by writing a JMP trampoline at each target address.
    ///
    /// Encoding: FF 25 00 00 00 00 <8-byte-abs-addr> (RIP-relative indirect JMP, 14 bytes).
    /// The caller must ensure target addresses are mapped and writable
    /// (e.g. CR0.WP cleared, direct physical mapping used).
    pub fn apply(&mut self) -> Result<(), &'static str> {
        if self.applied {
            return Err("patch already applied");
        }
        self.saved_bytes.clear();
        for entry in &self.entries {
            let target = entry.original_addr;
            let replacement = entry.replacement_addr;
            // Safety: caller guarantees addresses are valid writable kernel code pages.
            unsafe {
                let ptr = target as *mut u8;
                // Save original bytes before overwriting.
                let mut saved = [0u8; PATCH_SAVE_LEN];
                for i in 0..PATCH_SAVE_LEN {
                    saved[i] = ptr.add(i).read_volatile();
                }
                self.saved_bytes.push(saved);
                // Write JMP QWORD PTR [RIP+0]  (FF 25 00 00 00 00) + 8-byte target.
                ptr.add(0).write_volatile(0xFF);
                ptr.add(1).write_volatile(0x25);
                ptr.add(2).write_volatile(0x00);
                ptr.add(3).write_volatile(0x00);
                ptr.add(4).write_volatile(0x00);
                ptr.add(5).write_volatile(0x00);
                let addr_bytes = (replacement as u64).to_le_bytes();
                for (i, b) in addr_bytes.iter().enumerate() {
                    ptr.add(6 + i).write_volatile(*b);
                }
            }
        }
        self.applied = true;
        Ok(())
    }

    /// Revert the patch by restoring saved original bytes.
    pub fn revert(&mut self) -> Result<(), &'static str> {
        if !self.applied {
            return Err("patch not applied");
        }
        if self.saved_bytes.len() != self.entries.len() {
            return Err("saved_bytes length mismatch — cannot revert safely");
        }
        for (entry, saved) in self.entries.iter().zip(self.saved_bytes.iter()) {
            // Safety: same guarantees as apply().
            unsafe {
                let ptr = entry.original_addr as *mut u8;
                for (i, b) in saved.iter().enumerate() {
                    ptr.add(i).write_volatile(*b);
                }
            }
        }
        self.applied = false;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Static patch registry
// ---------------------------------------------------------------------------

/// Maximum number of live patches that can be registered simultaneously.
const MAX_PATCHES: usize = 32;

/// Inner state of the patch registry.
struct PatchRegistry {
    /// Fixed-size slot array; `None` = empty slot.
    slots: [Option<LivePatch>; MAX_PATCHES],
    /// Monotonically increasing ID counter for new patches.
    next_id: u32,
}

// We cannot derive Default for arrays of non-Copy Option<LivePatch>, so we
// implement a const constructor using a workaround: initialise via a const
// transmute of a zeroed region.  Instead, we use `unsafe` init in
// `PatchRegistry::new()` which is called only once from `PATCHES` initialiser.
//
// Since `Option<LivePatch>` is NOT `Copy` we cannot put it directly in a
// `const` array literal.  We therefore wrap the registry in a `MaybeUninit`
// and initialise it at `init()` time.  Until `init()` is called the registry
// must not be used for registration.

impl PatchRegistry {
    /// Construct a registry with all slots empty.
    ///
    /// Called from the `Mutex::new()` initialiser — must be `const` but
    /// `Option::None` for non-Copy types is allowed in const context.
    const fn new_empty() -> Self {
        // Build the array one element at a time.  Rust allows this in a
        // regular (non-const) function, which is fine because `PATCHES` is
        // initialised lazily behind a Mutex (the Mutex::new itself is const,
        // but the inner value just needs to be constructible at runtime).
        //
        // We use a helper to avoid repeating None 32 times.
        const NONE_PATCH: Option<LivePatch> = None;
        PatchRegistry {
            slots: [
                NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH,
                NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH,
                NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH,
                NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH,
                NONE_PATCH, NONE_PATCH, NONE_PATCH, NONE_PATCH,
            ],
            next_id: 1,
        }
    }

    /// Find the first empty slot index, or `None` if the registry is full.
    fn find_free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    /// Find the slot index occupied by the patch with the given `id`.
    fn find_by_id(&self, id: u32) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| s.as_ref().map(|p| p.id == id).unwrap_or(false))
    }
}

/// Global static patch registry protected by a spinlock Mutex.
///
/// Using a function-local static with `spin::Once` would also work, but the
/// project already uses `crate::sync::Mutex` everywhere so we follow that
/// pattern.  The `Mutex::new` call requires a `const`-constructible value;
/// we satisfy that by providing a `const`-compatible initialiser.
static PATCHES: Mutex<PatchRegistry> = Mutex::new(PatchRegistry::new_empty());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a patch in the static registry and apply it immediately.
///
/// Assigns a monotonically increasing `id` to the patch (overwriting any
/// pre-existing `id` field).  Returns the assigned ID on success, or 0 on
/// failure (registry full or apply error).
pub fn register_patch(mut patch: LivePatch) -> u32 {
    let mut reg = PATCHES.lock();

    let slot = match reg.find_free_slot() {
        Some(s) => s,
        None => {
            crate::serial_println!(
                "[livepatch] registry full ({} slots) — cannot register '{}'",
                MAX_PATCHES,
                patch.name
            );
            return 0;
        }
    };

    // Assign unique ID.
    let id = reg.next_id;
    reg.next_id = reg.next_id.wrapping_add(1);
    patch.id = id;

    // Apply the patch immediately.
    match patch.apply() {
        Ok(()) => {
            crate::serial_println!(
                "[livepatch] patch '{}' (id={}) applied — {} entry/entries patched",
                patch.name,
                id,
                patch.entries.len()
            );
            reg.slots[slot] = Some(patch);
            id
        }
        Err(e) => {
            crate::serial_println!(
                "[livepatch] patch '{}' apply failed: {} — not registered",
                patch.name,
                e
            );
            0
        }
    }
}

/// Revert a previously registered patch identified by `id`.
///
/// Restores the original bytes at each patched address via `write_volatile`
/// and removes the patch from the registry.  Returns `true` on success.
pub fn revert_patch(id: u32) -> bool {
    let mut reg = PATCHES.lock();

    let slot = match reg.find_by_id(id) {
        Some(s) => s,
        None => {
            crate::serial_println!("[livepatch] revert: patch id={} not found", id);
            return false;
        }
    };

    // Take ownership of the patch out of the slot so we can call revert().
    let mut patch = reg.slots[slot].take().unwrap();
    // Drop the lock before performing MMIO writes to avoid holding it
    // across potentially slow memory operations.
    drop(reg);

    let name = patch.name.clone();
    match patch.revert() {
        Ok(()) => {
            crate::serial_println!(
                "[livepatch] patch '{}' (id={}) reverted — original bytes restored",
                name,
                id
            );
            true
        }
        Err(e) => {
            crate::serial_println!(
                "[livepatch] patch '{}' (id={}) revert failed: {}",
                name,
                id,
                e
            );
            // Put the (still-applied) patch back into the registry slot.
            let mut reg2 = PATCHES.lock();
            reg2.slots[slot] = Some(patch);
            false
        }
    }
}

/// Return the count of currently active (applied) patches and log each one.
pub fn list_patches() -> usize {
    let reg = PATCHES.lock();
    let mut count = 0usize;

    for slot in reg.slots.iter() {
        if let Some(patch) = slot {
            if patch.applied {
                crate::serial_println!(
                    "[livepatch]   id={:>3}  applied=true  entries={}  name='{}'",
                    patch.id,
                    patch.entries.len(),
                    patch.name
                );
                count += 1;
            }
        }
    }

    crate::serial_println!("[livepatch] {} active patch(es)", count);
    count
}

/// Initialize the live patching subsystem.
pub fn init() {
    // The static registry is already set up by `Mutex::new(PatchRegistry::new_empty())`.
    // Nothing more needed at boot time.
    crate::serial_println!(
        "[livepatch] initialized — static registry with {} slots ready",
        MAX_PATCHES
    );
}
