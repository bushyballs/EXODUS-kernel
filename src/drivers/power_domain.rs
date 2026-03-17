use crate::sync::Mutex;
/// Power domain management driver for Genesis — no-heap, fixed-size arrays
///
/// Groups hardware blocks into power domains that can be powered on or off
/// together. Inspired by PSCI (Power State Coordination Interface) and
/// the Linux Generic Power Domain (genpd) framework. All code is original.
///
/// Domain topology:
///   - Domains form a tree rooted at domain id 0 (the "always-on" rail).
///   - `parent_id == 0` means the domain is a direct child of the root.
///   - Powering on a domain automatically powers on its parent first.
///
/// Reference counting:
///   - `ref_count` tracks how many consumers (devices or child domains)
///     have requested the domain to be on.
///   - `pd_power_on`  increments the count; transitions Off → On at 1.
///   - `pd_power_off` decrements the count; transitions On → Off at 0.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers (none used here)
///   - No division without guard
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of power domains
pub const MAX_POWER_DOMAINS: usize = 32;

/// Maximum number of devices that can be attached to a single domain
pub const MAX_DOMAIN_DEVICES: usize = 16;

// ---------------------------------------------------------------------------
// PSCI-inspired power state constants
// ---------------------------------------------------------------------------

/// Domain is fully powered on
pub const POWER_STATE_ON: u8 = 0;
/// Retention — powered but state preserved at reduced power
pub const POWER_STATE_RET: u8 = 1;
/// Domain is fully powered off
pub const POWER_STATE_OFF: u8 = 3;

// ---------------------------------------------------------------------------
// DomainState enum
// ---------------------------------------------------------------------------

/// Lifecycle state of a power domain
#[derive(Copy, Clone, PartialEq)]
pub enum DomainState {
    /// Domain is completely off
    Off,
    /// Transition to retention has been requested but not completed
    RetentionPending,
    /// Domain is in low-power retention (state preserved)
    Retention,
    /// Domain is fully powered on
    On,
    /// Power-on sequence is in progress
    TurningOn,
    /// Power-off sequence is in progress
    TurningOff,
}

// ---------------------------------------------------------------------------
// PowerDomainDevice
// ---------------------------------------------------------------------------

/// A device attached to a power domain
#[derive(Copy, Clone)]
pub struct PowerDomainDevice {
    /// Unique device identifier
    pub dev_id: u32,
    /// Human-readable device name (null-padded ASCII, up to 16 bytes)
    pub name: [u8; 16],
    /// Length of the valid name bytes
    pub name_len: u8,
    /// If true, this device prevents the domain from turning off while active
    pub required: bool,
    /// True when this device is currently active / consuming power
    pub active: bool,
}

impl PowerDomainDevice {
    /// Return a zeroed, inactive device slot
    pub const fn empty() -> Self {
        PowerDomainDevice {
            dev_id: 0,
            name: [0u8; 16],
            name_len: 0,
            required: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PowerDomain
// ---------------------------------------------------------------------------

/// A power domain grouping hardware blocks that share a power rail
#[derive(Copy, Clone)]
pub struct PowerDomain {
    /// Unique domain identifier
    pub id: u32,
    /// Human-readable domain name (null-padded ASCII, up to 32 bytes)
    pub name: [u8; 32],
    /// Length of the valid name bytes
    pub name_len: u8,
    /// Parent domain id; 0 means this domain is a root / always-on child
    pub parent_id: u32,
    /// Current power state
    pub state: DomainState,
    /// Attached devices; entries 0..ndevices are valid
    pub devices: [PowerDomainDevice; MAX_DOMAIN_DEVICES],
    /// Number of valid device entries
    pub ndevices: u8,
    /// Number of active consumers (devices + child domains)
    pub ref_count: u32,
    /// Simulated delay before the domain actually turns off (ms, informational)
    pub off_delay_ms: u32,
    /// True when this domain slot is in use
    pub active: bool,
}

impl PowerDomain {
    /// Return a zeroed, inactive domain slot
    pub const fn empty() -> Self {
        PowerDomain {
            id: 0,
            name: [0u8; 32],
            name_len: 0,
            parent_id: 0,
            state: DomainState::Off,
            devices: [PowerDomainDevice::empty(); MAX_DOMAIN_DEVICES],
            ndevices: 0,
            ref_count: 0,
            off_delay_ms: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static POWER_DOMAINS: Mutex<[PowerDomain; MAX_POWER_DOMAINS]> =
    Mutex::new([PowerDomain::empty(); MAX_POWER_DOMAINS]);

/// Monotonic domain-id counter (1-based so that 0 is the "root" sentinel)
static NEXT_DOMAIN_ID: Mutex<u32> = Mutex::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to `dst.len()` bytes from `src` into `dst`, null-padding the rest.
/// Returns the number of meaningful bytes copied (capped at dst.len() - 1 to
/// leave room for a null terminator).
fn copy_name_32(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let max = 31usize; // leave one byte for safety null-padding
    let len = if src.len() < max { src.len() } else { max };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 32 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
    len as u8
}

/// Copy up to 15 bytes from `src` into a 16-byte device name array.
fn copy_name_16(dst: &mut [u8; 16], src: &[u8]) -> u8 {
    let max = 15usize;
    let len = if src.len() < max { src.len() } else { max };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
    len as u8
}

/// Find the array index of a domain by its id. Returns MAX_POWER_DOMAINS if
/// not found.
fn find_domain_idx(domains: &[PowerDomain; MAX_POWER_DOMAINS], id: u32) -> usize {
    let mut i = 0usize;
    while i < MAX_POWER_DOMAINS {
        if domains[i].active && domains[i].id == id {
            return i;
        }
        i = i.saturating_add(1);
    }
    MAX_POWER_DOMAINS // sentinel: not found
}

/// Return true if any required device in `domain` is currently active.
fn has_required_active(domain: &PowerDomain) -> bool {
    let mut i = 0usize;
    while i < domain.ndevices as usize {
        if i >= MAX_DOMAIN_DEVICES {
            break;
        }
        if domain.devices[i].required && domain.devices[i].active {
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new power domain.
///
/// `name`      — ASCII name slice (up to 31 bytes used).
/// `parent_id` — id of the parent domain; 0 means no parent (root child).
///
/// Returns the new domain's id on success, or `None` if the table is full.
pub fn pd_register(name: &[u8], parent_id: u32) -> Option<u32> {
    let new_id = {
        let mut id_guard = NEXT_DOMAIN_ID.lock();
        let id = *id_guard;
        *id_guard = id_guard.saturating_add(1);
        id
    };

    let mut domains = POWER_DOMAINS.lock();
    for d in domains.iter_mut() {
        if !d.active {
            *d = PowerDomain::empty();
            d.id = new_id;
            d.name_len = copy_name_32(&mut d.name, name);
            d.parent_id = parent_id;
            d.state = DomainState::Off;
            d.active = true;
            return Some(new_id);
        }
    }
    None // table full
}

/// Unregister a power domain by id.
///
/// Returns `true` on success, `false` if the domain was not found.
pub fn pd_unregister(domain_id: u32) -> bool {
    let mut domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return false;
    }
    domains[idx] = PowerDomain::empty();
    true
}

/// Request power-on for a domain.
///
/// Increments `ref_count`. When the count transitions from 0 to 1 the
/// domain moves Off → TurningOn → On. If the domain has a parent
/// (`parent_id != 0`), that parent is powered on recursively first.
///
/// Returns `true` on success, `false` if the domain was not found.
pub fn pd_power_on(domain_id: u32) -> bool {
    // First, recursively power on the parent (if any) before acquiring the
    // main lock again, to avoid nested lock usage.
    let parent_id = {
        let domains = POWER_DOMAINS.lock();
        let idx = find_domain_idx(&domains, domain_id);
        if idx >= MAX_POWER_DOMAINS {
            return false;
        }
        domains[idx].parent_id
    };

    if parent_id != 0 {
        // Power on parent first; ignore return value (best-effort).
        let _ = pd_power_on(parent_id);
    }

    // Now power on this domain.
    let mut domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return false;
    }

    let d = &mut domains[idx];
    let prev_ref = d.ref_count;
    d.ref_count = d.ref_count.saturating_add(1);

    if prev_ref == 0 {
        // First consumer — start the On transition.
        d.state = DomainState::TurningOn;
        // Simulated instant hardware bring-up.
        d.state = DomainState::On;
    }
    true
}

/// Request power-off for a domain.
///
/// Decrements `ref_count`. When the count reaches zero the domain moves
/// On → TurningOff → Off, provided no required devices are still active.
///
/// Returns `false` if:
///   - The domain was not found.
///   - A required device is still active (power-off is blocked).
pub fn pd_power_off(domain_id: u32) -> bool {
    let mut domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return false;
    }

    let d = &mut domains[idx];

    // Check for blocking required devices.
    if has_required_active(d) {
        return false;
    }

    if d.ref_count > 0 {
        d.ref_count = d.ref_count.saturating_sub(1);
    }

    if d.ref_count == 0 {
        d.state = DomainState::TurningOff;
        // Simulated instant hardware tear-down.
        d.state = DomainState::Off;
    }
    true
}

/// Return the current power state of a domain, or `None` if not found.
pub fn pd_get_state(domain_id: u32) -> Option<DomainState> {
    let domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return None;
    }
    Some(domains[idx].state)
}

/// Return the reference count of a domain (0 if not found).
pub fn pd_get_ref_count(domain_id: u32) -> u32 {
    let domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return 0;
    }
    domains[idx].ref_count
}

/// Attach a device to a power domain.
///
/// `domain_id` — target domain.
/// `dev_id`    — unique device identifier.
/// `name`      — human-readable device name (up to 15 bytes used).
/// `required`  — if true, this device blocks domain power-off while active.
///
/// Returns `true` on success, `false` if the domain is full or not found.
pub fn pd_device_attach(domain_id: u32, dev_id: u32, name: &[u8], required: bool) -> bool {
    let mut domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return false;
    }

    let d = &mut domains[idx];
    let ndev = d.ndevices as usize;
    if ndev >= MAX_DOMAIN_DEVICES {
        return false;
    }

    let dev = &mut d.devices[ndev];
    dev.dev_id = dev_id;
    dev.name_len = copy_name_16(&mut dev.name, name);
    dev.required = required;
    dev.active = false;
    d.ndevices = d.ndevices.saturating_add(1);
    true
}

/// Detach a device from a power domain by device id.
///
/// Shifts remaining device entries to fill the gap.
/// Returns `true` on success, `false` if not found.
pub fn pd_device_detach(domain_id: u32, dev_id: u32) -> bool {
    let mut domains = POWER_DOMAINS.lock();
    let idx = find_domain_idx(&domains, domain_id);
    if idx >= MAX_POWER_DOMAINS {
        return false;
    }

    let d = &mut domains[idx];
    let ndev = d.ndevices as usize;
    let mut found_at = MAX_DOMAIN_DEVICES; // sentinel

    let mut i = 0usize;
    while i < ndev {
        if i >= MAX_DOMAIN_DEVICES {
            break;
        }
        if d.devices[i].dev_id == dev_id {
            found_at = i;
            break;
        }
        i = i.saturating_add(1);
    }

    if found_at >= MAX_DOMAIN_DEVICES {
        return false; // device not in this domain
    }

    // Shift entries left to fill the gap.
    let mut j = found_at;
    while j.saturating_add(1) < ndev && j.saturating_add(1) < MAX_DOMAIN_DEVICES {
        d.devices[j] = d.devices[j.saturating_add(1)];
        j = j.saturating_add(1);
    }
    // Zero the now-vacant last slot.
    if ndev > 0 && ndev.saturating_sub(1) < MAX_DOMAIN_DEVICES {
        d.devices[ndev.saturating_sub(1)] = PowerDomainDevice::empty();
    }
    if d.ndevices > 0 {
        d.ndevices = d.ndevices.saturating_sub(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the power domain manager.
///
/// Registers five standard platform domains and powers on the ones that are
/// required at boot time (cpu, storage, usb). The "always-on" domain starts
/// in On state because it represents the main power rail.
pub fn init() {
    // Register "always-on" (id=1, parent=0): main power rail — always On
    if let Some(id) = pd_register(b"always-on", 0) {
        // Force the always-on domain into On state regardless of ref_count.
        let mut domains = POWER_DOMAINS.lock();
        let idx = find_domain_idx(&domains, id);
        if idx < MAX_POWER_DOMAINS {
            domains[idx].state = DomainState::On;
            domains[idx].ref_count = 1; // permanently held
        }
        drop(domains);
        serial_println!("  [power_domain] 'always-on' domain id={}", id);
    }

    // Register "cpu" (parent = always-on id=1)
    if let Some(id) = pd_register(b"cpu", 1) {
        pd_power_on(id);
        serial_println!("  [power_domain] 'cpu' domain id={}", id);
    }

    // Register "gpu" (parent = always-on id=1) — off by default
    if let Some(id) = pd_register(b"gpu", 1) {
        serial_println!("  [power_domain] 'gpu' domain id={} (off)", id);
    }

    // Register "storage" (parent = always-on id=1)
    if let Some(id) = pd_register(b"storage", 1) {
        pd_power_on(id);
        serial_println!("  [power_domain] 'storage' domain id={}", id);
    }

    // Register "usb" (parent = always-on id=1)
    if let Some(id) = pd_register(b"usb", 1) {
        pd_power_on(id);
        serial_println!("  [power_domain] 'usb' domain id={}", id);
    }

    super::register("power-domain", super::DeviceType::Other);
    serial_println!("[power_domain] power domain manager initialized");
}
