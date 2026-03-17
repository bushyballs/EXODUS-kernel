/// Peer Discovery for Genesis
///
/// Discovers peers on the local network and beyond via multiple
/// methods: multicast (mDNS-style), broadcast, Bluetooth/BLE,
/// manual registration, and DHT-based lookup.
///
/// All code is original. No external crates.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracked peers
const MAX_PEERS: usize = 512;

/// Maximum number of services per peer
const MAX_SERVICES_PER_PEER: usize = 16;

/// Maximum number of banned peers
const MAX_BANNED: usize = 128;

/// Maximum number of trusted peers
const MAX_TRUSTED: usize = 64;

/// Multicast group hash (simulated mDNS group 224.0.0.251 -> hash)
const MCAST_GROUP_HASH: u64 = 0x00E000000000_00FB;

/// Broadcast address sentinel
const BROADCAST_SENTINEL: u64 = 0xFFFF_FFFF_FFFF_FFFF;

/// Discovery interval in abstract ticks
const DISCOVERY_INTERVAL: u64 = 300;

/// Peer stale threshold (ticks without contact)
const STALE_THRESHOLD: u64 = 1800;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Method by which a peer was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMethod {
    /// Multicast / mDNS-style announcement
    Multicast,
    /// Subnet broadcast
    Broadcast,
    /// Bluetooth Low Energy
    Bluetooth,
    /// Manually added by user / config
    Manual,
    /// Found via DHT lookup
    Dht,
}

/// A discovered peer.
#[derive(Clone)]
pub struct Peer {
    pub id: u64,
    pub addr_hash: u64,
    pub name_hash: u64,
    pub services: Vec<u64>,
    pub discovered_via: DiscoveryMethod,
    pub last_seen: u64,
}

/// Discovery subsystem configuration.
struct DiscoveryConfig {
    multicast_enabled: bool,
    broadcast_enabled: bool,
    bluetooth_enabled: bool,
    dht_enabled: bool,
    announce_interval: u64,
    scan_interval: u64,
}

impl DiscoveryConfig {
    fn default_config() -> Self {
        DiscoveryConfig {
            multicast_enabled: true,
            broadcast_enabled: true,
            bluetooth_enabled: false,
            dht_enabled: true,
            announce_interval: DISCOVERY_INTERVAL,
            scan_interval: DISCOVERY_INTERVAL,
        }
    }
}

/// Stats snapshot for the discovery subsystem.
pub struct DiscoveryStats {
    pub total_peers: usize,
    pub active_peers: usize,
    pub banned_count: usize,
    pub trusted_count: usize,
    pub multicast_on: bool,
    pub broadcast_on: bool,
    pub bluetooth_on: bool,
    pub dht_on: bool,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PEERS: Mutex<Option<Vec<Peer>>> = Mutex::new(None);
static BANNED: Mutex<Option<Vec<u64>>> = Mutex::new(None);
static TRUSTED: Mutex<Option<Vec<u64>>> = Mutex::new(None);
static CONFIG: Mutex<Option<DiscoveryConfig>> = Mutex::new(None);
static DISCOVERY_RUNNING: Mutex<bool> = Mutex::new(false);
static LOCAL_PEER_ID: Mutex<Option<u64>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    {
        let mut peers = PEERS.lock();
        *peers = Some(Vec::new());
    }
    {
        let mut banned = BANNED.lock();
        *banned = Some(Vec::new());
    }
    {
        let mut trusted = TRUSTED.lock();
        *trusted = Some(Vec::new());
    }
    {
        let mut config = CONFIG.lock();
        *config = Some(DiscoveryConfig::default_config());
    }
    serial_println!("    peer_discovery: initialized (mDNS, broadcast, BLE, DHT)");
}

// ---------------------------------------------------------------------------
// Discovery control
// ---------------------------------------------------------------------------

/// Start the discovery subsystem with the given local peer id.
pub fn start_discovery(local_id: u64) -> bool {
    {
        let running = DISCOVERY_RUNNING.lock();
        if *running {
            return false; // already running
        }
    }

    {
        let mut local = LOCAL_PEER_ID.lock();
        *local = Some(local_id);
    }
    {
        let mut running = DISCOVERY_RUNNING.lock();
        *running = true;
    }

    // Perform initial announcement on all enabled methods
    announce_on_all(local_id);
    true
}

/// Stop the discovery subsystem.
pub fn stop_discovery() {
    let mut running = DISCOVERY_RUNNING.lock();
    *running = false;
}

/// Check if discovery is currently running.
pub fn is_running() -> bool {
    let running = DISCOVERY_RUNNING.lock();
    *running
}

// ---------------------------------------------------------------------------
// Announcement
// ---------------------------------------------------------------------------

/// Announce our presence on all enabled discovery methods.
fn announce_on_all(local_id: u64) {
    let config = CONFIG.lock();
    if let Some(ref cfg) = *config {
        if cfg.multicast_enabled {
            announce_multicast(local_id);
        }
        if cfg.broadcast_enabled {
            announce_broadcast(local_id);
        }
        if cfg.bluetooth_enabled {
            announce_bluetooth(local_id);
        }
    }
}

/// Announce presence via multicast (mDNS-style).
fn announce_multicast(local_id: u64) {
    // In a real implementation, this would send an mDNS packet to
    // the multicast group. Here we log the intent.
    let _group = MCAST_GROUP_HASH;
    let _id = local_id;
    // Packet would contain: local_id, services, name_hash
}

/// Announce presence via subnet broadcast.
fn announce_broadcast(local_id: u64) {
    let _bcast = BROADCAST_SENTINEL;
    let _id = local_id;
    // Broadcast UDP packet with peer info
}

/// Announce presence via Bluetooth Low Energy advertisement.
fn announce_bluetooth(local_id: u64) {
    let _id = local_id;
    // BLE advertisement with service UUID containing peer id
}

/// Announce our presence explicitly. Can be called periodically.
pub fn announce() {
    let local_id;
    {
        let local = LOCAL_PEER_ID.lock();
        local_id = match *local {
            Some(id) => id,
            None => return,
        };
    }
    announce_on_all(local_id);
}

// ---------------------------------------------------------------------------
// Peer registration
// ---------------------------------------------------------------------------

/// Register a newly discovered peer. Returns true if added.
pub fn register_peer(
    id: u64,
    addr_hash: u64,
    name_hash: u64,
    services: Vec<u64>,
    method: DiscoveryMethod,
    timestamp: u64,
) -> bool {
    // Check ban list
    {
        let banned = BANNED.lock();
        if let Some(ref list) = *banned {
            if list.contains(&id) {
                return false;
            }
        }
    }

    // Check local id (don't register self)
    {
        let local = LOCAL_PEER_ID.lock();
        if let Some(lid) = *local {
            if lid == id {
                return false;
            }
        }
    }

    let mut peers = PEERS.lock();
    if let Some(ref mut list) = *peers {
        // Update existing peer
        for peer in list.iter_mut() {
            if peer.id == id {
                peer.addr_hash = addr_hash;
                peer.name_hash = name_hash;
                peer.last_seen = timestamp;
                peer.discovered_via = method;
                // Merge services
                for svc in &services {
                    if !peer.services.contains(svc) && peer.services.len() < MAX_SERVICES_PER_PEER {
                        peer.services.push(*svc);
                    }
                }
                return true;
            }
        }

        // New peer — check capacity
        if list.len() >= MAX_PEERS {
            // Evict the oldest peer that is not trusted
            let trusted_ids: Vec<u64>;
            {
                let trusted = TRUSTED.lock();
                trusted_ids = match *trusted {
                    Some(ref t) => t.clone(),
                    None => Vec::new(),
                };
            }

            let mut oldest_idx: Option<usize> = None;
            let mut oldest_ts: u64 = u64::MAX;
            for (i, peer) in list.iter().enumerate() {
                if !trusted_ids.contains(&peer.id) && peer.last_seen < oldest_ts {
                    oldest_ts = peer.last_seen;
                    oldest_idx = Some(i);
                }
            }

            if let Some(idx) = oldest_idx {
                list.remove(idx);
            } else {
                return false; // all peers are trusted, cannot evict
            }
        }

        let truncated_services = if services.len() > MAX_SERVICES_PER_PEER {
            services[..MAX_SERVICES_PER_PEER].to_vec()
        } else {
            services
        };

        list.push(Peer {
            id,
            addr_hash,
            name_hash,
            services: truncated_services,
            discovered_via: method,
            last_seen: timestamp,
        });
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Peer queries
// ---------------------------------------------------------------------------

/// Get all known peers.
pub fn get_peers() -> Vec<Peer> {
    let peers = PEERS.lock();
    match *peers {
        Some(ref list) => list.clone(),
        None => Vec::new(),
    }
}

/// Get a specific peer by ID.
pub fn get_peer(id: u64) -> Option<Peer> {
    let peers = PEERS.lock();
    if let Some(ref list) = *peers {
        for peer in list.iter() {
            if peer.id == id {
                return Some(peer.clone());
            }
        }
    }
    None
}

/// Filter peers by a specific service hash.
pub fn filter_by_service(service_hash: u64) -> Vec<Peer> {
    let peers = PEERS.lock();
    match *peers {
        Some(ref list) => list
            .iter()
            .filter(|p| p.services.contains(&service_hash))
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Filter peers by discovery method.
pub fn filter_by_method(method: DiscoveryMethod) -> Vec<Peer> {
    let peers = PEERS.lock();
    match *peers {
        Some(ref list) => list
            .iter()
            .filter(|p| p.discovered_via == method)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Get peers that have been seen since the given timestamp.
pub fn get_active_peers(since: u64) -> Vec<Peer> {
    let peers = PEERS.lock();
    match *peers {
        Some(ref list) => list
            .iter()
            .filter(|p| p.last_seen >= since)
            .cloned()
            .collect(),
        None => Vec::new(),
    }
}

/// Number of known peers.
pub fn peer_count() -> usize {
    let peers = PEERS.lock();
    match *peers {
        Some(ref list) => list.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Trust / ban management
// ---------------------------------------------------------------------------

/// Ban a peer. Removes from peer list and prevents future registration.
pub fn ban_peer(id: u64) -> bool {
    // Remove from peers
    {
        let mut peers = PEERS.lock();
        if let Some(ref mut list) = *peers {
            list.retain(|p| p.id != id);
        }
    }

    // Remove from trusted
    {
        let mut trusted = TRUSTED.lock();
        if let Some(ref mut list) = *trusted {
            list.retain(|&tid| tid != id);
        }
    }

    // Add to banned
    let mut banned = BANNED.lock();
    if let Some(ref mut list) = *banned {
        if list.contains(&id) {
            return false; // already banned
        }
        if list.len() >= MAX_BANNED {
            list.remove(0); // evict oldest ban
        }
        list.push(id);
        true
    } else {
        false
    }
}

/// Unban a peer.
pub fn unban_peer(id: u64) -> bool {
    let mut banned = BANNED.lock();
    if let Some(ref mut list) = *banned {
        let before = list.len();
        list.retain(|&bid| bid != id);
        list.len() < before
    } else {
        false
    }
}

/// Mark a peer as trusted. Trusted peers are not evicted during capacity management.
pub fn trust_peer(id: u64) -> bool {
    // Must exist in peer list
    {
        let peers = PEERS.lock();
        let exists = match *peers {
            Some(ref list) => list.iter().any(|p| p.id == id),
            None => false,
        };
        if !exists {
            return false;
        }
    }

    let mut trusted = TRUSTED.lock();
    if let Some(ref mut list) = *trusted {
        if list.contains(&id) {
            return false; // already trusted
        }
        if list.len() >= MAX_TRUSTED {
            return false; // trust list full
        }
        list.push(id);
        true
    } else {
        false
    }
}

/// Remove trust status from a peer.
pub fn untrust_peer(id: u64) -> bool {
    let mut trusted = TRUSTED.lock();
    if let Some(ref mut list) = *trusted {
        let before = list.len();
        list.retain(|&tid| tid != id);
        list.len() < before
    } else {
        false
    }
}

/// Check if a peer is banned.
pub fn is_banned(id: u64) -> bool {
    let banned = BANNED.lock();
    match *banned {
        Some(ref list) => list.contains(&id),
        None => false,
    }
}

/// Check if a peer is trusted.
pub fn is_trusted(id: u64) -> bool {
    let trusted = TRUSTED.lock();
    match *trusted {
        Some(ref list) => list.contains(&id),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

/// Remove peers not seen since the cutoff timestamp.
/// Trusted peers are exempt from pruning.
pub fn prune_stale(cutoff: u64) -> usize {
    let trusted_ids: Vec<u64>;
    {
        let trusted = TRUSTED.lock();
        trusted_ids = match *trusted {
            Some(ref list) => list.clone(),
            None => Vec::new(),
        };
    }

    let mut peers = PEERS.lock();
    if let Some(ref mut list) = *peers {
        let before = list.len();
        list.retain(|p| p.last_seen >= cutoff || trusted_ids.contains(&p.id));
        before - list.len()
    } else {
        0
    }
}

/// Get a statistics snapshot.
pub fn get_stats(now: u64) -> DiscoveryStats {
    let total;
    let active;
    {
        let peers = PEERS.lock();
        match *peers {
            Some(ref list) => {
                total = list.len();
                let threshold = if now > STALE_THRESHOLD {
                    now - STALE_THRESHOLD
                } else {
                    0
                };
                active = list.iter().filter(|p| p.last_seen >= threshold).count();
            }
            None => {
                total = 0;
                active = 0;
            }
        }
    }

    let banned_count;
    {
        let banned = BANNED.lock();
        banned_count = match *banned {
            Some(ref list) => list.len(),
            None => 0,
        };
    }

    let trusted_count;
    {
        let trusted = TRUSTED.lock();
        trusted_count = match *trusted {
            Some(ref list) => list.len(),
            None => 0,
        };
    }

    let (mcast, bcast, bt, dht_on);
    {
        let config = CONFIG.lock();
        match *config {
            Some(ref cfg) => {
                mcast = cfg.multicast_enabled;
                bcast = cfg.broadcast_enabled;
                bt = cfg.bluetooth_enabled;
                dht_on = cfg.dht_enabled;
            }
            None => {
                mcast = false;
                bcast = false;
                bt = false;
                dht_on = false;
            }
        }
    }

    DiscoveryStats {
        total_peers: total,
        active_peers: active,
        banned_count,
        trusted_count,
        multicast_on: mcast,
        broadcast_on: bcast,
        bluetooth_on: bt,
        dht_on,
    }
}

/// Enable or disable a specific discovery method.
pub fn set_method_enabled(method: DiscoveryMethod, enabled: bool) {
    let mut config = CONFIG.lock();
    if let Some(ref mut cfg) = *config {
        match method {
            DiscoveryMethod::Multicast => cfg.multicast_enabled = enabled,
            DiscoveryMethod::Broadcast => cfg.broadcast_enabled = enabled,
            DiscoveryMethod::Bluetooth => cfg.bluetooth_enabled = enabled,
            DiscoveryMethod::Dht => cfg.dht_enabled = enabled,
            DiscoveryMethod::Manual => {} // Manual is always available
        }
    }
}
