/// IEEE 802.1D Ethernet Bridge
///
/// Provides L2 switching with MAC address learning, FDB aging, and STP port states.
/// Single bridge instance with up to 16 ports and 256 MAC addresses in FDB.
/// All data structures are fixed-size, no heap allocation.
///
/// Inspired by: Linux bridge, IEEE 802.1D specification.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Port State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgePortState {
    Disabled,
    Blocking,
    Listening,
    Learning,
    Forwarding,
}

impl BridgePortState {
    pub fn can_learn(&self) -> bool {
        matches!(
            self,
            BridgePortState::Learning | BridgePortState::Forwarding
        )
    }

    pub fn can_forward(&self) -> bool {
        matches!(self, BridgePortState::Forwarding)
    }
}

// ---------------------------------------------------------------------------
// Bridge Port
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BridgePort {
    pub port_ifindex: u8,
    pub state: BridgePortState,
    pub port_id: u16,
    pub path_cost: u32,
    pub active: bool,
}

impl BridgePort {
    pub const fn empty() -> Self {
        BridgePort {
            port_ifindex: 0,
            state: BridgePortState::Disabled,
            port_id: 0,
            path_cost: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// FDB Entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BridgeFdbEntry {
    pub mac: [u8; 6],
    pub port_ifindex: u8,
    pub is_local: bool,
    pub last_seen_ms: u64,
    pub active: bool,
}

impl BridgeFdbEntry {
    pub const fn empty() -> Self {
        BridgeFdbEntry {
            mac: [0; 6],
            port_ifindex: 0,
            is_local: false,
            last_seen_ms: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct BridgeState {
    pub bridge_id: [u8; 8],
    pub root_id: [u8; 8],
    pub root_port: u8,
    pub root_path_cost: u32,
    pub is_root: bool,
    pub hello_time_ms: u64,
    pub nports: u8,
    pub ports: [BridgePort; 16],
}

impl BridgeState {
    pub const fn empty() -> Self {
        BridgeState {
            bridge_id: [0; 8],
            root_id: [0; 8],
            root_port: 0,
            root_path_cost: 0,
            is_root: false,
            hello_time_ms: 0,
            nports: 0,
            ports: [BridgePort::empty(); 16],
        }
    }
}

// ---------------------------------------------------------------------------
// Global Bridge State
// ---------------------------------------------------------------------------

static BRIDGE: Mutex<BridgeState> = Mutex::new(BridgeState::empty());
static BRIDGE_FDB: Mutex<[BridgeFdbEntry; 256]> = Mutex::new([BridgeFdbEntry::empty(); 256]);
static LAST_HELLO_MS: AtomicU64 = AtomicU64::new(0);

const FDB_AGING_MS: u64 = 300_000; // 5 minutes

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a port to the bridge.
/// Returns true if successful, false if bridge is full or port already exists.
pub fn bridge_port_add(ifindex: u8) -> bool {
    let mut bridge = BRIDGE.lock();

    // Check if port already exists
    for i in 0..bridge.nports as usize {
        if bridge.ports[i].port_ifindex == ifindex {
            return false; // Already exists
        }
    }

    if bridge.nports >= 16 {
        return false; // Bridge is full
    }

    let idx = bridge.nports as usize;
    bridge.ports[idx] = BridgePort {
        port_ifindex: ifindex,
        state: BridgePortState::Blocking,
        port_id: ifindex as u16,
        path_cost: 19_200, // Default cost for 100Mbps
        active: true,
    };
    bridge.nports = bridge.nports.saturating_add(1);
    true
}

/// Remove a port from the bridge.
/// Returns true if port was removed, false if not found.
pub fn bridge_port_remove(ifindex: u8) -> bool {
    let mut bridge = BRIDGE.lock();

    let mut found_idx = None;
    for i in 0..bridge.nports as usize {
        if bridge.ports[i].port_ifindex == ifindex {
            found_idx = Some(i);
            break;
        }
    }

    let idx = match found_idx {
        Some(i) => i,
        None => return false,
    };

    // Shift remaining ports
    let n = bridge.nports as usize;
    if idx < n - 1 {
        for i in idx..n - 1 {
            bridge.ports[i] = bridge.ports[i + 1];
        }
    }
    bridge.nports = bridge.nports.saturating_sub(1);
    true
}

/// Set the state of a port.
/// Returns true if port found and state updated.
pub fn bridge_port_set_state(ifindex: u8, state: BridgePortState) -> bool {
    let mut bridge = BRIDGE.lock();

    for i in 0..bridge.nports as usize {
        if bridge.ports[i].port_ifindex == ifindex {
            bridge.ports[i].state = state;
            return true;
        }
    }
    false
}

/// Learn or update a MAC address in the FDB.
pub fn bridge_fdb_learn(mac: &[u8; 6], ifindex: u8, current_ms: u64) {
    let mut fdb = BRIDGE_FDB.lock();

    // Look for existing entry
    for i in 0..256 {
        if fdb[i].active && fdb[i].mac == *mac {
            fdb[i].last_seen_ms = current_ms;
            fdb[i].port_ifindex = ifindex;
            return;
        }
    }

    // Find empty slot
    for i in 0..256 {
        if !fdb[i].active {
            fdb[i] = BridgeFdbEntry {
                mac: *mac,
                port_ifindex: ifindex,
                is_local: false,
                last_seen_ms: current_ms,
                active: true,
            };
            return;
        }
    }
}

/// Look up a MAC address in the FDB.
/// Returns the port ifindex if found, None otherwise (flood).
pub fn bridge_fdb_lookup(mac: &[u8; 6]) -> Option<u8> {
    let fdb = BRIDGE_FDB.lock();

    for i in 0..256 {
        if fdb[i].active && fdb[i].mac == *mac {
            return Some(fdb[i].port_ifindex);
        }
    }
    None
}

/// Age out and remove stale FDB entries.
pub fn bridge_fdb_expire(current_ms: u64, age_ms: u64) {
    let mut fdb = BRIDGE_FDB.lock();

    for i in 0..256 {
        if fdb[i].active && !fdb[i].is_local {
            let age = current_ms.saturating_sub(fdb[i].last_seen_ms);
            if age > age_ms {
                fdb[i].active = false;
            }
        }
    }
}

/// Process an incoming frame: learn source MAC, look up destination MAC.
/// frame: raw Ethernet frame data (including source/dest MAC)
/// len: frame length (must be >= 14 for Ethernet header)
/// in_ifindex: incoming port interface index
/// current_ms: current time in milliseconds
pub fn bridge_rx(frame: &[u8], len: usize, in_ifindex: u8, current_ms: u64) {
    if len < 14 {
        return; // Frame too short
    }

    // Parse source and destination MACs from Ethernet header
    let src_mac = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];
    let dst_mac = [frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]];

    // Learn source MAC
    bridge_fdb_learn(&src_mac, in_ifindex, current_ms);

    // Look up destination MAC
    if let Some(out_ifindex) = bridge_fdb_lookup(&dst_mac) {
        // Unicast: forward to specific port
        // (In a real implementation, we'd queue the frame for transmission on out_ifindex)
    } else {
        // Broadcast/unknown: flood to all ports except incoming
        // (In a real implementation, we'd queue frame for all other active ports)
    }
}

/// Periodic bridge maintenance: hello timer and FDB aging.
/// Call this every ~1000ms.
pub fn bridge_tick(current_ms: u64) {
    // Check if we should send hello BPDUs (STP)
    let last_hello = LAST_HELLO_MS.load(Ordering::Relaxed);
    if current_ms.saturating_sub(last_hello) >= 1000 {
        // Time to send hello (not implemented in stub)
        LAST_HELLO_MS.store(current_ms, Ordering::Relaxed);
    }

    // Age out stale FDB entries every 5000ms
    if current_ms.saturating_sub(last_hello) % 5000 == 0 {
        bridge_fdb_expire(current_ms, FDB_AGING_MS);
    }
}

/// Initialize the bridge subsystem.
pub fn init() {
    let mut bridge = BRIDGE.lock();
    bridge.bridge_id = [0x80, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    bridge.root_id = bridge.bridge_id;
    bridge.is_root = true;
    bridge.hello_time_ms = 2000;
    bridge.nports = 0;
    drop(bridge);

    LAST_HELLO_MS.store(0, Ordering::Relaxed);

    serial_println!("[bridge] IEEE 802.1D Ethernet bridge initialized");
}
