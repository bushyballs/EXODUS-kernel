/// UEFI runtime variable storage — Genesis AIOS
///
/// Implements a persistent in-memory variable store modelled on the UEFI
/// Runtime Services `GetVariable` / `SetVariable` / `GetNextVariableName`
/// interface (UEFI Spec §8.2).
///
/// Because the kernel runs without a heap this module keeps all variables in
/// a fixed-size static array protected by a `Mutex`.  The maximum supported
/// storage is therefore bounded by the compile-time constants below.
///
/// Rules enforced:
///   - No heap (no Vec / Box / String / alloc::*)
///   - No float casts
///   - No panic (no unwrap / expect)
///   - Saturating arithmetic on all counters
///   - Wrapping arithmetic on sequence numbers
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of variables that can be stored simultaneously.
pub const MAX_EFI_VARS: usize = 64;

/// Maximum byte size of a single variable's data payload.
pub const MAX_VAR_DATA: usize = 512;

/// Maximum byte length of a variable name (UTF-8 / ASCII octets).
pub const MAX_VAR_NAME: usize = 64;

// UEFI variable attribute bits (UEFI Spec Table 36).
pub const EFI_VARIABLE_NON_VOLATILE: u32 = 1;
pub const EFI_VARIABLE_BOOTSERVICE_ACCESS: u32 = 2;
pub const EFI_VARIABLE_RUNTIME_ACCESS: u32 = 4;

// ---------------------------------------------------------------------------
// EFI GUID
// ---------------------------------------------------------------------------

/// A 128-bit EFI GUID as defined in the UEFI specification.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EfiGuid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl EfiGuid {
    /// Returns the all-zeros (nil) GUID.
    pub const fn zero() -> Self {
        EfiGuid {
            data1: 0,
            data2: 0,
            data3: 0,
            data4: [0u8; 8],
        }
    }

    /// Returns `true` if the two GUIDs are byte-for-byte identical.
    #[inline]
    fn equals(&self, other: &EfiGuid) -> bool {
        self.data1 == other.data1
            && self.data2 == other.data2
            && self.data3 == other.data3
            && self.data4 == other.data4
    }
}

// ---------------------------------------------------------------------------
// Well-known GUIDs
// ---------------------------------------------------------------------------

/// `EFI_GLOBAL_VARIABLE` GUID — {8BE4DF61-93CA-11D2-AA0D-00E098032B8C}
pub const EFI_GLOBAL_GUID: EfiGuid = EfiGuid {
    data1: 0x8BE4_DF61,
    data2: 0x93CA,
    data3: 0x11D2,
    data4: [0xAA, 0x0D, 0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C],
};

// ---------------------------------------------------------------------------
// EfiVar record
// ---------------------------------------------------------------------------

/// A single UEFI runtime variable entry.
#[derive(Clone, Copy)]
pub struct EfiVar {
    /// Variable name as raw bytes (not NUL-terminated).
    pub name: [u8; MAX_VAR_NAME],
    /// Number of valid bytes in `name`.
    pub name_len: u8,
    /// Namespace GUID for this variable.
    pub guid: EfiGuid,
    /// UEFI attribute bitmask.
    pub attributes: u32,
    /// Raw data payload.
    pub data: [u8; MAX_VAR_DATA],
    /// Number of valid bytes in `data`.
    pub data_len: u16,
    /// `false` means this slot is free/deleted.
    pub active: bool,
}

impl EfiVar {
    /// Construct an empty / unused slot.  `const fn` so it is usable in
    /// the static initialiser.
    pub const fn empty() -> Self {
        EfiVar {
            name: [0u8; MAX_VAR_NAME],
            name_len: 0,
            guid: EfiGuid::zero(),
            attributes: 0,
            data: [0u8; MAX_VAR_DATA],
            data_len: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static variable store
// ---------------------------------------------------------------------------

/// The global in-memory UEFI variable store.
///
/// The inner array is `[EfiVar; MAX_EFI_VARS]`.  `EfiVar` is `Copy` and
/// `EfiVar::empty()` is `const fn`, satisfying the requirements of
/// `Mutex::new()` in a bare-metal `static`.
static EFI_VARS: Mutex<[EfiVar; MAX_EFI_VARS]> = Mutex::new([EfiVar::empty(); MAX_EFI_VARS]);

// ---------------------------------------------------------------------------
// Helper — name comparison (byte slice vs fixed array)
// ---------------------------------------------------------------------------

/// Compare `name` slice against the `name` field of a slot.  The comparison
/// is byte-exact and length-exact; neither `name` nor the stored name may
/// exceed `MAX_VAR_NAME`.
#[inline]
fn name_matches(slot: &EfiVar, name: &[u8]) -> bool {
    if name.len() > MAX_VAR_NAME {
        return false;
    }
    if slot.name_len as usize != name.len() {
        return false;
    }
    let len = slot.name_len as usize;
    // Manual loop — no iterators that could panic on index-out-of-bounds; the
    // bound checks above guarantee safety.
    let mut i = 0usize;
    while i < len {
        if slot.name[i] != name[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Retrieve the value of a UEFI variable identified by `(name, guid)`.
///
/// On success the payload is written into `data_out`, `*data_len` is set to
/// the number of bytes written, and `true` is returned.
///
/// Returns `false` if:
/// - `name` is empty or longer than `MAX_VAR_NAME`
/// - No matching active variable is found
pub fn efi_get_variable(
    name: &[u8],
    guid: &EfiGuid,
    data_out: &mut [u8; MAX_VAR_DATA],
    data_len: &mut u16,
) -> bool {
    if name.is_empty() || name.len() > MAX_VAR_NAME {
        return false;
    }
    let store = EFI_VARS.lock();
    let mut i = 0usize;
    while i < MAX_EFI_VARS {
        let v = &store[i];
        if v.active && v.guid.equals(guid) && name_matches(v, name) {
            let len = v.data_len as usize;
            // Copy payload — both sides are exactly MAX_VAR_DATA bytes.
            let mut j = 0usize;
            while j < len {
                data_out[j] = v.data[j];
                j = j.saturating_add(1);
            }
            *data_len = v.data_len;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Create or update a UEFI variable identified by `(name, guid)`.
///
/// If a variable with the same `(name, guid)` already exists its payload is
/// replaced in-place.  Otherwise a free slot is used.
///
/// Returns `false` if:
/// - `name` is empty or longer than `MAX_VAR_NAME`
/// - `data.len() > MAX_VAR_DATA`
/// - The store is full and no existing variable matches
pub fn efi_set_variable(name: &[u8], guid: &EfiGuid, attributes: u32, data: &[u8]) -> bool {
    if name.is_empty() || name.len() > MAX_VAR_NAME {
        return false;
    }
    if data.len() > MAX_VAR_DATA {
        return false;
    }

    let mut store = EFI_VARS.lock();

    // First pass: update an existing variable if found.
    let mut i = 0usize;
    while i < MAX_EFI_VARS {
        let v = &mut store[i];
        if v.active && v.guid.equals(guid) && name_matches(v, name) {
            // Update payload in place.
            let dlen = data.len();
            let mut j = 0usize;
            while j < dlen {
                v.data[j] = data[j];
                j = j.saturating_add(1);
            }
            v.data_len = dlen as u16;
            v.attributes = attributes;
            return true;
        }
        i = i.saturating_add(1);
    }

    // Second pass: find a free slot.
    let mut i = 0usize;
    while i < MAX_EFI_VARS {
        let v = &mut store[i];
        if !v.active {
            // Write name.
            let nlen = name.len();
            let mut j = 0usize;
            while j < nlen {
                v.name[j] = name[j];
                j = j.saturating_add(1);
            }
            v.name_len = nlen as u8;
            v.guid = *guid;
            v.attributes = attributes;

            // Write data.
            let dlen = data.len();
            let mut j = 0usize;
            while j < dlen {
                v.data[j] = data[j];
                j = j.saturating_add(1);
            }
            v.data_len = dlen as u16;
            v.active = true;
            return true;
        }
        i = i.saturating_add(1);
    }

    // Store is full.
    false
}

/// Mark a variable as deleted.  The slot is freed for future use.
///
/// Returns `false` if the variable is not found.
pub fn efi_delete_variable(name: &[u8], guid: &EfiGuid) -> bool {
    if name.is_empty() || name.len() > MAX_VAR_NAME {
        return false;
    }
    let mut store = EFI_VARS.lock();
    let mut i = 0usize;
    while i < MAX_EFI_VARS {
        let v = &mut store[i];
        if v.active && v.guid.equals(guid) && name_matches(v, name) {
            // Zero out the slot so the name/data are not retained in memory.
            *v = EfiVar::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Enumerate variables: given `prev_name` (or `None` to start), write the
/// *next* active variable's name into `name_out` and `*name_len_out`.
///
/// The enumeration order is the internal slot order.  Deleted variables are
/// skipped automatically.
///
/// Returns `false` when there are no more variables.
pub fn efi_get_next_variable(
    prev_name: Option<&[u8]>,
    name_out: &mut [u8; MAX_VAR_NAME],
    name_len_out: &mut u8,
) -> bool {
    let store = EFI_VARS.lock();

    // Find the slot *after* the one matching prev_name.
    let start_slot = match prev_name {
        None => 0usize,
        Some(pn) => {
            if pn.is_empty() || pn.len() > MAX_VAR_NAME {
                return false;
            }
            // Locate prev_name in the store.
            let mut found = MAX_EFI_VARS; // sentinel: "not found"
            let mut i = 0usize;
            while i < MAX_EFI_VARS {
                let v = &store[i];
                if v.active && name_matches(v, pn) {
                    found = i;
                    break;
                }
                i = i.saturating_add(1);
            }
            if found == MAX_EFI_VARS {
                // prev_name not found; restart from the beginning.
                0usize
            } else {
                found.saturating_add(1)
            }
        }
    };

    // Find the next active slot from `start_slot`.
    let mut i = start_slot;
    while i < MAX_EFI_VARS {
        let v = &store[i];
        if v.active {
            let nlen = v.name_len as usize;
            let mut j = 0usize;
            while j < nlen {
                name_out[j] = v.name[j];
                j = j.saturating_add(1);
            }
            *name_len_out = v.name_len;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Query the total and remaining storage capacity of the variable store.
///
/// Returns `(max_storage_bytes, remaining_storage_bytes, max_variable_size)`.
///
/// `max_storage_bytes`    — total byte capacity of all variable payloads
/// `remaining_storage_bytes` — bytes still available for new variable data
/// `max_variable_size`    — maximum payload size of a single variable
pub fn efi_query_variable_info() -> (u64, u64, u64) {
    let max_storage = (MAX_EFI_VARS * MAX_VAR_DATA) as u64;
    let max_size = MAX_VAR_DATA as u64;

    let store = EFI_VARS.lock();
    let mut used: u64 = 0;
    let mut i = 0usize;
    while i < MAX_EFI_VARS {
        if store[i].active {
            used = used.saturating_add(store[i].data_len as u64);
        }
        i = i.saturating_add(1);
    }

    let remaining = max_storage.saturating_sub(used);
    (max_storage, remaining, max_size)
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the EFI variable store.
///
/// Pre-populates the standard `BootOrder` global variable so early boot
/// code finds a valid value.
pub fn init() {
    // Pre-populate BootOrder with 8 zero bytes.
    let boot_order_data = [0u8; 8];
    efi_set_variable(
        b"BootOrder",
        &EFI_GLOBAL_GUID,
        EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS,
        &boot_order_data,
    );

    serial_println!("[efi_vars] runtime variable store initialized");
}
