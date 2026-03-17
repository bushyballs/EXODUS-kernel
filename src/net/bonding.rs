use crate::sync::Mutex;
/// Network bonding / link aggregation (no-heap)
///
/// Supports four bonding modes:
///   0 — Round-robin  (transmit in order across all link-up slaves)
///   1 — Active-backup (one active slave; others stand by for failover)
///   3 — Broadcast    (transmit on every link-up slave simultaneously)
///   4 — 802.3ad LACP (simplified: treated as round-robin for TX)
///
/// All state is held in fixed-size static arrays — no Vec, no String, no alloc.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

pub const MAX_BONDS: usize = 4;
pub const MAX_SLAVES_PER_BOND: usize = 8;

// ---------------------------------------------------------------------------
// Bonding mode constants
// ---------------------------------------------------------------------------

pub const BOND_MODE_ROUNDROBIN: u8 = 0;
pub const BOND_MODE_ACTIVEBACKUP: u8 = 1;
pub const BOND_MODE_BROADCAST: u8 = 3;
pub const BOND_MODE_8023AD: u8 = 4;

// ---------------------------------------------------------------------------
// BondSlave
// ---------------------------------------------------------------------------

/// One physical (or virtual) interface enslaved to a bond.
#[derive(Clone, Copy)]
pub struct BondSlave {
    /// Index into the netdev device table identifying this interface.
    pub iface_idx: u32,
    /// Whether this slave is administratively active (used in active-backup).
    pub active: bool,
    /// Physical link state.
    pub link_up: bool,
    /// Total bytes transmitted through this slave.
    pub tx_bytes: u64,
    /// Total bytes received through this slave.
    pub rx_bytes: u64,
}

impl BondSlave {
    pub const fn empty() -> Self {
        BondSlave {
            iface_idx: 0,
            active: false,
            link_up: false,
            tx_bytes: 0,
            rx_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// BondDevice
// ---------------------------------------------------------------------------

/// A bonded logical interface aggregating up to MAX_SLAVES_PER_BOND slaves.
#[derive(Clone, Copy)]
pub struct BondDevice {
    /// Unique numeric identifier assigned at creation time.
    pub id: u32,
    /// Bonding mode: one of the BOND_MODE_* constants.
    pub mode: u8,
    /// Human-readable name (NUL-padded ASCII, up to 15 characters + NUL).
    pub name: [u8; 16],
    /// Logical MAC address presented to the network layer.
    pub mac: [u8; 6],
    /// Slave array (valid entries are [0..nslaves)).
    pub slaves: [BondSlave; MAX_SLAVES_PER_BOND],
    /// Number of valid entries in `slaves`.
    pub nslaves: u8,
    /// Index within `slaves` of the currently active slave (active-backup).
    pub active_slave: u8,
    /// Round-robin cursor; wraps via wrapping_add.
    pub rr_cursor: u8,
    /// MII link-monitoring interval in milliseconds.
    pub mii_interval_ms: u32,
    /// Whether this bond device is enabled.
    pub enabled: bool,
}

impl BondDevice {
    pub const fn empty() -> Self {
        BondDevice {
            id: 0,
            mode: BOND_MODE_ACTIVEBACKUP,
            name: [0u8; 16],
            mac: [0u8; 6],
            slaves: [const { BondSlave::empty() }; MAX_SLAVES_PER_BOND],
            nslaves: 0,
            active_slave: 0,
            rr_cursor: 0,
            mii_interval_ms: 100,
            enabled: false,
        }
    }

    // -----------------------------------------------------------------------
    // Internal helpers (operate only on self — no lock held here)
    // -----------------------------------------------------------------------

    /// Write up to 15 bytes of `src` into `self.name` (NUL-terminates).
    fn set_name(&mut self, src: &[u8]) {
        let len = src.len().min(15);
        self.name[..len].copy_from_slice(&src[..len]);
        self.name[len] = 0;
    }

    /// Find a slave by iface_idx; returns its position in `slaves`, or None.
    fn find_slave(&self, iface_idx: u32) -> Option<usize> {
        let n = self.nslaves as usize;
        for i in 0..n {
            if self.slaves[i].iface_idx == iface_idx {
                return Some(i);
            }
        }
        None
    }

    /// Find the first slave whose link is up; returns its index in `slaves`.
    fn first_link_up_slave(&self) -> Option<usize> {
        let n = self.nslaves as usize;
        for i in 0..n {
            if self.slaves[i].link_up {
                return Some(i);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static BOND_DEVICES: Mutex<[BondDevice; MAX_BONDS]> =
    Mutex::new([const { BondDevice::empty() }; MAX_BONDS]);

// ---------------------------------------------------------------------------
// Internal utility: next unused bond slot
// ---------------------------------------------------------------------------

fn find_free_slot(devices: &[BondDevice; MAX_BONDS]) -> Option<usize> {
    for i in 0..MAX_BONDS {
        if !devices[i].enabled {
            return Some(i);
        }
    }
    None
}

fn find_bond_slot(devices: &[BondDevice; MAX_BONDS], id: u32) -> Option<usize> {
    for i in 0..MAX_BONDS {
        if devices[i].enabled && devices[i].id == id {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new bond with the given name slice and mode.
///
/// The default MAC is `[0x02, 0xBB, 0xBB, id as u8, 0x00, 0x00]`.
/// Returns the bond `id` on success, or `None` if the table is full.
pub fn bond_create(name: &[u8], mode: u8) -> Option<u32> {
    let mut devices = BOND_DEVICES.lock();

    let slot = find_free_slot(&devices)?;

    // Derive a stable id from the slot index (1-based so id=0 is never valid).
    let id = (slot as u32).saturating_add(1);

    let dev = &mut devices[slot];
    *dev = BondDevice::empty();
    dev.id = id;
    dev.mode = mode;
    dev.enabled = true;
    dev.set_name(name);
    dev.mac = [0x02, 0xBB, 0xBB, id as u8, 0x00, 0x00];

    serial_println!("[bonding] created bond id={} mode={}", id, mode);
    Some(id)
}

/// Destroy an existing bond.  Returns `true` if found and removed.
pub fn bond_destroy(id: u32) -> bool {
    let mut devices = BOND_DEVICES.lock();
    if let Some(slot) = find_bond_slot(&devices, id) {
        devices[slot] = BondDevice::empty();
        serial_println!("[bonding] destroyed bond id={}", id);
        true
    } else {
        false
    }
}

/// Attach `iface_idx` as a slave to the bond identified by `bond_id`.
///
/// Returns `false` if the bond is not found, the slave is already present,
/// or the slave table is full.
pub fn bond_add_slave(bond_id: u32, iface_idx: u32) -> bool {
    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return false,
    };
    let dev = &mut devices[slot];

    let n = dev.nslaves as usize;
    if n >= MAX_SLAVES_PER_BOND {
        return false;
    }
    // Duplicate check
    if dev.find_slave(iface_idx).is_some() {
        return false;
    }

    dev.slaves[n] = BondSlave {
        iface_idx,
        active: false,
        link_up: false,
        tx_bytes: 0,
        rx_bytes: 0,
    };
    dev.nslaves = dev.nslaves.saturating_add(1);

    serial_println!("[bonding] bond {} added slave iface={}", bond_id, iface_idx);
    true
}

/// Detach `iface_idx` from the bond.  Compacts the slave array.
///
/// Returns `false` if the bond or slave is not found.
pub fn bond_remove_slave(bond_id: u32, iface_idx: u32) -> bool {
    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return false,
    };
    let dev = &mut devices[slot];

    let pos = match dev.find_slave(iface_idx) {
        Some(p) => p,
        None => return false,
    };

    let n = dev.nslaves as usize;

    // Shift everything after `pos` left by one slot.
    let mut i = pos;
    while i.saturating_add(1) < n {
        dev.slaves[i] = dev.slaves[i.saturating_add(1)];
        i = i.saturating_add(1);
    }
    dev.slaves[n.saturating_sub(1)] = BondSlave::empty();
    dev.nslaves = dev.nslaves.saturating_sub(1);

    // Fix active_slave index if it pointed at or past the removed slot.
    let active = dev.active_slave as usize;
    if active == pos {
        // The active slave was just removed; promote the first link-up slave.
        dev.active_slave = dev.first_link_up_slave().unwrap_or(0) as u8;
    } else if active > pos && active > 0 {
        dev.active_slave = dev.active_slave.saturating_sub(1);
    }

    serial_println!(
        "[bonding] bond {} removed slave iface={}",
        bond_id,
        iface_idx
    );
    true
}

/// Manually select `iface_idx` as the active slave for an active-backup bond.
///
/// Only meaningful when `mode == BOND_MODE_ACTIVEBACKUP`.
/// Returns `false` if the bond or slave is not found.
pub fn bond_set_active_slave(bond_id: u32, iface_idx: u32) -> bool {
    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return false,
    };
    let dev = &mut devices[slot];

    let pos = match dev.find_slave(iface_idx) {
        Some(p) => p,
        None => return false,
    };

    dev.active_slave = pos as u8;
    serial_println!(
        "[bonding] bond {} active slave -> iface={}",
        bond_id,
        iface_idx
    );
    true
}

/// Mark the slave identified by `iface_idx` as link-up.
///
/// In active-backup mode, if there is currently no valid active slave
/// (active_slave index is out of range or that slave's link is down),
/// this slave is promoted to active.
pub fn bond_slave_link_up(bond_id: u32, iface_idx: u32) {
    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return,
    };
    let dev = &mut devices[slot];

    let pos = match dev.find_slave(iface_idx) {
        Some(p) => p,
        None => return,
    };
    dev.slaves[pos].link_up = true;

    // In active-backup: promote this slave if no active slave is currently up.
    if dev.mode == BOND_MODE_ACTIVEBACKUP {
        let active = dev.active_slave as usize;
        let n = dev.nslaves as usize;
        let need_promotion = active >= n || !dev.slaves[active].link_up;
        if need_promotion {
            dev.active_slave = pos as u8;
            serial_println!(
                "[bonding] bond {} promoted slave iface={} to active",
                bond_id,
                iface_idx
            );
        }
    }
}

/// Mark the slave identified by `iface_idx` as link-down.
///
/// In active-backup mode, if this was the active slave, scan for the first
/// remaining link-up slave and promote it.
pub fn bond_slave_link_down(bond_id: u32, iface_idx: u32) {
    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return,
    };
    let dev = &mut devices[slot];

    let pos = match dev.find_slave(iface_idx) {
        Some(p) => p,
        None => return,
    };
    dev.slaves[pos].link_up = false;

    // In active-backup: failover if this was the active slave.
    if dev.mode == BOND_MODE_ACTIVEBACKUP {
        let active = dev.active_slave as usize;
        if active == pos {
            // Find first other slave that is link-up.
            let n = dev.nslaves as usize;
            let mut found = false;
            for i in 0..n {
                if i != pos && dev.slaves[i].link_up {
                    dev.active_slave = i as u8;
                    serial_println!(
                        "[bonding] bond {} failover: slave iface={} is now active",
                        bond_id,
                        dev.slaves[i].iface_idx
                    );
                    found = true;
                    break;
                }
            }
            if !found {
                serial_println!(
                    "[bonding] bond {} failover: no link-up slave available",
                    bond_id
                );
            }
        }
    }
}

/// Transmit `frame[..len]` through the bond.
///
/// Dispatch rules:
///   ROUNDROBIN / 8023AD — advance `rr_cursor` (wrapping), pick
///     `slaves[rr_cursor % nslaves]` skipping non-link-up slaves.
///     Falls back to the next link-up slave by linear scan if needed.
///   ACTIVEBACKUP — send only on `slaves[active_slave]` if it is link-up.
///   BROADCAST    — send on every link-up slave.
///
/// Calls `crate::net::netdev::driver_send_by_idx(iface_idx, frame, len)`
/// for each slave that is selected.  Updates `tx_bytes` with saturating_add.
///
/// Returns `true` if at least one slave successfully transmitted.
pub fn bond_transmit(bond_id: u32, frame: &[u8; 1514], len: usize) -> bool {
    // Clamp len to the frame buffer size.
    let len = len.min(1514);

    let mut devices = BOND_DEVICES.lock();
    let slot = match find_bond_slot(&devices, bond_id) {
        Some(s) => s,
        None => return false,
    };
    let dev = &mut devices[slot];

    if !dev.enabled || dev.nslaves == 0 {
        return false;
    }

    let n = dev.nslaves as usize;
    let mut sent_any = false;

    match dev.mode {
        BOND_MODE_ACTIVEBACKUP => {
            let active = dev.active_slave as usize;
            if active < n && dev.slaves[active].link_up {
                let iface = dev.slaves[active].iface_idx;
                let ok = crate::net::netdev::driver_send_by_idx(iface, frame, len);
                if ok {
                    dev.slaves[active].tx_bytes =
                        dev.slaves[active].tx_bytes.saturating_add(len as u64);
                    sent_any = true;
                }
            }
        }

        BOND_MODE_BROADCAST => {
            for i in 0..n {
                if dev.slaves[i].link_up {
                    let iface = dev.slaves[i].iface_idx;
                    let ok = crate::net::netdev::driver_send_by_idx(iface, frame, len);
                    if ok {
                        dev.slaves[i].tx_bytes = dev.slaves[i].tx_bytes.saturating_add(len as u64);
                        sent_any = true;
                    }
                }
            }
        }

        // ROUNDROBIN (0) and 8023AD (4) — round-robin over link-up slaves.
        _ => {
            // Advance the cursor (wrapping).
            dev.rr_cursor = dev.rr_cursor.wrapping_add(1);

            // Try up to `n` candidates starting from the cursor position,
            // skipping slaves that are link-down.
            let mut chosen: Option<usize> = None;
            for attempt in 0..n {
                let candidate = (dev.rr_cursor as usize).wrapping_add(attempt) % n;
                if dev.slaves[candidate].link_up {
                    chosen = Some(candidate);
                    break;
                }
            }

            if let Some(i) = chosen {
                let iface = dev.slaves[i].iface_idx;
                let ok = crate::net::netdev::driver_send_by_idx(iface, frame, len);
                if ok {
                    dev.slaves[i].tx_bytes = dev.slaves[i].tx_bytes.saturating_add(len as u64);
                    sent_any = true;
                }
            }
        }
    }

    sent_any
}

/// Receive a frame that arrived on `iface_idx`.
///
/// Finds which bond this interface belongs to, updates `rx_bytes`,
/// and delivers the frame to `crate::net::process_frame`.
///
/// Returns `true` if the interface is a member of a bond (and the frame
/// was forwarded), `false` otherwise.
pub fn bond_receive(iface_idx: u32, frame: &[u8; 1514], len: usize) -> bool {
    let len = len.min(1514);

    let mut devices = BOND_DEVICES.lock();

    // Find the bond that owns this slave.
    for slot in 0..MAX_BONDS {
        if !devices[slot].enabled {
            continue;
        }
        let n = devices[slot].nslaves as usize;
        for i in 0..n {
            if devices[slot].slaves[i].iface_idx == iface_idx {
                devices[slot].slaves[i].rx_bytes =
                    devices[slot].slaves[i].rx_bytes.saturating_add(len as u64);
                // Release the lock before calling process_frame to avoid
                // potential deadlocks inside the network stack.
                drop(devices);
                crate::net::process_frame(&frame[..len]);
                return true;
            }
        }
    }

    false
}

/// Return aggregate (total_tx_bytes, total_rx_bytes) for all slaves in a bond.
///
/// Returns `None` if the bond is not found.
pub fn bond_get_stats(bond_id: u32) -> Option<(u64, u64)> {
    let devices = BOND_DEVICES.lock();
    let slot = find_bond_slot(&devices, bond_id)?;

    let dev = &devices[slot];
    let n = dev.nslaves as usize;

    let mut total_tx: u64 = 0;
    let mut total_rx: u64 = 0;
    for i in 0..n {
        total_tx = total_tx.saturating_add(dev.slaves[i].tx_bytes);
        total_rx = total_rx.saturating_add(dev.slaves[i].rx_bytes);
    }
    Some((total_tx, total_rx))
}

/// Periodic MII link-monitoring tick.
///
/// Called with the current monotonic timestamp in milliseconds.  Iterates
/// over all active bonds and ensures `active_slave` is valid in active-backup
/// mode (promotes the first link-up slave if the current active is link-down).
pub fn bond_mii_tick(current_ms: u64) {
    let _ = current_ms; // timestamp available for future interval gating

    let mut devices = BOND_DEVICES.lock();
    for slot in 0..MAX_BONDS {
        if !devices[slot].enabled {
            continue;
        }
        let dev = &mut devices[slot];
        if dev.mode != BOND_MODE_ACTIVEBACKUP {
            continue;
        }

        let active = dev.active_slave as usize;
        let n = dev.nslaves as usize;
        if n == 0 {
            continue;
        }

        // If the active slave is missing or link-down, find a replacement.
        let needs_failover = active >= n || !dev.slaves[active].link_up;
        if needs_failover {
            for i in 0..n {
                if dev.slaves[i].link_up {
                    dev.active_slave = i as u8;
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the network bonding subsystem.
pub fn init() {
    // The static is already zero-initialized through `BondDevice::empty()`.
    // Just announce readiness.
    serial_println!("[bonding] network bonding initialized");
}
