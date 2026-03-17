use crate::sync::Mutex;
/// I2P Garlic Routing for Genesis
///
/// Implements the Invisible Internet Project routing model where multiple
/// messages (cloves) are bundled into a single garlic message and routed
/// through unidirectional tunnels. Inbound and outbound tunnels are separate,
/// providing stronger anonymity than bidirectional circuits.
///
/// Key concepts:
///   - Destinations: cryptographic identifiers (not IP addresses)
///   - Tunnels: unidirectional chains of routers (inbound or outbound)
///   - Garlic messages: encrypted bundles of multiple cloves
///   - LeaseSets: published sets of inbound tunnel endpoints
///   - NetDB: distributed hash table of router/destination info
///
/// Tunnel types: Inbound, Outbound, Exploratory (for NetDB lookups)
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of hops per tunnel
const DEFAULT_TUNNEL_LENGTH: usize = 3;

/// Maximum cloves in a single garlic message
const MAX_CLOVES_PER_GARLIC: usize = 16;

/// Maximum tunnels in a tunnel pool
const MAX_POOL_SIZE: usize = 8;

/// Tunnel lifetime in ticks before rebuild
const TUNNEL_LIFETIME: u64 = 600;

/// Hash seed for garlic encryption
const GARLIC_KEY_SEED: u64 = 0x12ED4C6B8A0FE5D3;

/// Hash seed for tunnel layer encryption
const TUNNEL_LAYER_SEED: u64 = 0x7F3E5A1D9C2B4068;

/// Hash seed for destination lookups
const DESTINATION_SEED: u64 = 0x3B5A7C9D1E0F2A4C;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of I2P tunnel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelType {
    /// Receives traffic destined for our node
    Inbound,
    /// Sends traffic from our node toward the network
    Outbound,
    /// Used for NetDB lookups and tunnel building
    Exploratory,
}

/// Status of a tunnel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    /// Tunnel is being constructed
    Building,
    /// Tunnel is operational
    Ready,
    /// Tunnel has expired and needs rebuilding
    Expired,
    /// Tunnel construction or operation failed
    Failed,
}

/// A single clove within a garlic message
#[derive(Debug, Clone)]
pub struct GarlicClove {
    /// Hash identifying the delivery destination
    pub delivery_hash: u64,
    /// Hash of the clove payload data
    pub data_hash: u64,
    /// Clove delivery instructions
    delivery_type: CloveDeliveryType,
    /// Expiration tick for this clove
    expiry: u64,
    /// Payload bytes for this clove
    payload: Vec<u8>,
}

/// How a clove should be delivered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloveDeliveryType {
    /// Deliver to the local router
    Local,
    /// Deliver to a specific destination
    Destination,
    /// Deliver to a specific router
    Router,
    /// Deliver into a tunnel
    Tunnel,
}

/// A garlic message containing bundled encrypted cloves
#[derive(Debug, Clone)]
pub struct GarlicMessage {
    /// Individual cloves bundled in this message
    pub cloves: Vec<GarlicClove>,
    /// Whether the message has been encrypted
    pub encrypted: bool,
    /// Message identifier
    message_id: u64,
    /// Encryption key hash for the entire garlic bundle
    encryption_key_hash: u64,
    /// Total size of all clove payloads
    total_payload_size: usize,
}

/// An I2P tunnel (unidirectional chain of hops)
#[derive(Debug, Clone)]
pub struct I2pTunnel {
    /// Unique tunnel identifier
    pub id: u64,
    /// Hash of the tunnel destination
    pub destination_hash: u64,
    /// Type of tunnel (inbound, outbound, exploratory)
    pub tunnel_type: TunnelType,
    /// Ordered list of hop router hashes in the tunnel
    pub hops: Vec<u64>,
    /// Tick when the tunnel was created
    pub created: u64,
    /// Whether the tunnel is currently active
    pub active: bool,
    /// Current tunnel status
    status: TunnelStatus,
    /// Per-hop layer encryption keys
    layer_keys: Vec<u64>,
    /// Bytes routed through this tunnel
    bytes_routed: u64,
    /// Messages routed through this tunnel
    messages_routed: u64,
}

/// A LeaseSet advertising inbound tunnel endpoints
#[derive(Debug, Clone)]
struct LeaseSet {
    /// Destination hash this LeaseSet belongs to
    destination_hash: u64,
    /// Tunnel IDs that can reach this destination
    tunnel_ids: Vec<u64>,
    /// Expiration tick
    expiry: u64,
    /// Signature hash for verification
    signature_hash: u64,
}

/// A tunnel pool managing a set of related tunnels
#[derive(Debug, Clone)]
struct TunnelPool {
    /// Pool identifier
    id: u64,
    /// Type of tunnels in this pool
    tunnel_type: TunnelType,
    /// Tunnel IDs in this pool
    tunnel_ids: Vec<u64>,
    /// Desired pool size
    target_size: usize,
}

/// Entry in the NetDB (distributed hash table)
#[derive(Debug, Clone)]
struct NetDbEntry {
    /// Router or destination hash
    key_hash: u64,
    /// Whether this is a router info or lease set
    is_router: bool,
    /// Data hash for the entry content
    data_hash: u64,
    /// When this entry was last updated
    updated: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Active tunnels
static TUNNELS: Mutex<Vec<I2pTunnel>> = Mutex::new(Vec::new());

/// Published lease sets
static LEASE_SETS: Mutex<Vec<LeaseSet>> = Mutex::new(Vec::new());

/// Tunnel pools
static TUNNEL_POOLS: Mutex<Vec<TunnelPool>> = Mutex::new(Vec::new());

/// Local NetDB cache
static NET_DB: Mutex<Vec<NetDbEntry>> = Mutex::new(Vec::new());

/// Next tunnel ID counter
static NEXT_TUNNEL_ID: Mutex<u64> = Mutex::new(1);

/// Next message ID counter
static NEXT_MESSAGE_ID: Mutex<u64> = Mutex::new(1);

/// Simulated tick
static TICK: Mutex<u64> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Hash function for I2P key derivation and lookups
fn i2p_hash(data: &[u8], seed: u64) -> u64 {
    let mut h: u64 = seed;
    for &b in data {
        h = h.wrapping_mul(0x6C62272E07BB0142).wrapping_add(b as u64);
        h ^= h >> 23;
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    h
}

/// Derive a tunnel layer key for a specific hop
fn derive_layer_key(tunnel_id: u64, hop_index: usize, router_hash: u64) -> u64 {
    let mut buf = vec![0u8; 24];
    buf[0..8].copy_from_slice(&tunnel_id.to_le_bytes());
    buf[8..16].copy_from_slice(&(hop_index as u64).to_le_bytes());
    buf[16..24].copy_from_slice(&router_hash.to_le_bytes());
    i2p_hash(&buf, TUNNEL_LAYER_SEED)
}

/// Encrypt a garlic message payload
fn garlic_encrypt(data: &mut [u8], key_hash: u64) {
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] ^= key_bytes[i % 8];
        data[i] = data[i].wrapping_add(key_bytes[(i + 3) % 8]);
    }
}

/// Decrypt a garlic message payload
fn garlic_decrypt(data: &mut [u8], key_hash: u64) {
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] = data[i].wrapping_sub(key_bytes[(i + 3) % 8]);
        data[i] ^= key_bytes[i % 8];
    }
}

/// Get current tick value
fn current_tick() -> u64 {
    let t = TICK.lock();
    *t
}

/// Generate simulated router hashes for tunnel hops
fn generate_hop_hashes(count: usize, seed: u64) -> Vec<u64> {
    let mut hops = Vec::new();
    for i in 0..count {
        let mut buf = vec![0u8; 16];
        buf[0..8].copy_from_slice(&seed.to_le_bytes());
        buf[8..16].copy_from_slice(&(i as u64).to_le_bytes());
        hops.push(i2p_hash(&buf, DESTINATION_SEED));
    }
    hops
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new I2P tunnel of the specified type.
/// Returns the tunnel ID on success.
pub fn create_tunnel(tunnel_type: TunnelType) -> Option<u64> {
    let mut id_counter = NEXT_TUNNEL_ID.lock();
    let tunnel_id = *id_counter;
    *id_counter = id_counter.wrapping_add(1);
    drop(id_counter);

    let now = current_tick();
    let hops = generate_hop_hashes(DEFAULT_TUNNEL_LENGTH, tunnel_id);

    // Derive layer keys for each hop
    let mut layer_keys = Vec::new();
    for (i, hop_hash) in hops.iter().enumerate() {
        let key = derive_layer_key(tunnel_id, i, *hop_hash);
        layer_keys.push(key);
    }

    let destination_hash = i2p_hash(&tunnel_id.to_le_bytes(), DESTINATION_SEED);

    let tunnel = I2pTunnel {
        id: tunnel_id,
        destination_hash,
        tunnel_type,
        hops: hops.clone(),
        created: now,
        active: true,
        status: TunnelStatus::Ready,
        layer_keys,
        bytes_routed: 0,
        messages_routed: 0,
    };

    let type_str = match tunnel_type {
        TunnelType::Inbound => "inbound",
        TunnelType::Outbound => "outbound",
        TunnelType::Exploratory => "exploratory",
    };

    let mut tunnels = TUNNELS.lock();
    tunnels.push(tunnel);

    serial_println!(
        "  I2P: {} tunnel {} created ({} hops, dest_hash={:#018X})",
        type_str,
        tunnel_id,
        hops.len(),
        destination_hash
    );

    Some(tunnel_id)
}

/// Send a garlic message through an outbound tunnel.
/// Bundles the provided cloves, encrypts, and routes through tunnel hops.
/// Returns the message ID on success.
pub fn send_garlic(tunnel_id: u64, cloves: Vec<GarlicClove>) -> Option<u64> {
    if cloves.is_empty() || cloves.len() > MAX_CLOVES_PER_GARLIC {
        serial_println!(
            "  I2P: invalid clove count ({}, max={})",
            cloves.len(),
            MAX_CLOVES_PER_GARLIC
        );
        return None;
    }

    let mut tunnels = TUNNELS.lock();
    let tunnel = match tunnels.iter_mut().find(|t| t.id == tunnel_id) {
        Some(t) => t,
        None => return None,
    };

    if !tunnel.active || tunnel.status != TunnelStatus::Ready {
        return None;
    }

    if tunnel.tunnel_type != TunnelType::Outbound && tunnel.tunnel_type != TunnelType::Exploratory {
        serial_println!("  I2P: cannot send on inbound tunnel {}", tunnel_id);
        return None;
    }

    let mut msg_id_counter = NEXT_MESSAGE_ID.lock();
    let message_id = *msg_id_counter;
    *msg_id_counter = msg_id_counter.wrapping_add(1);
    drop(msg_id_counter);

    // Calculate total payload size
    let total_size: usize = cloves.iter().map(|c| c.payload.len()).sum();
    let encryption_key = i2p_hash(&message_id.to_le_bytes(), GARLIC_KEY_SEED);

    let mut message = GarlicMessage {
        cloves,
        encrypted: false,
        message_id,
        encryption_key_hash: encryption_key,
        total_payload_size: total_size,
    };

    // Encrypt each clove payload
    for clove in message.cloves.iter_mut() {
        garlic_encrypt(&mut clove.payload, encryption_key);
    }
    message.encrypted = true;

    // Apply tunnel layer encryption (each hop adds a layer)
    // For outbound: innermost hop key first, outermost last
    let num_layers = tunnel.layer_keys.len();
    for clove in message.cloves.iter_mut() {
        for layer_idx in 0..num_layers {
            garlic_encrypt(&mut clove.payload, tunnel.layer_keys[layer_idx]);
        }
    }

    tunnel.bytes_routed = tunnel.bytes_routed.wrapping_add(total_size as u64);
    tunnel.messages_routed = tunnel.messages_routed.wrapping_add(1);

    serial_println!(
        "  I2P: garlic msg {} sent via tunnel {} ({} cloves, {} bytes)",
        message_id,
        tunnel_id,
        message.cloves.len(),
        total_size
    );

    Some(message_id)
}

/// Receive and process a garlic message arriving on an inbound tunnel.
/// Decrypts tunnel layers and garlic encryption, returning the cloves.
pub fn receive(tunnel_id: u64, mut encrypted_data: Vec<u8>) -> Vec<GarlicClove> {
    let tunnels = TUNNELS.lock();
    let tunnel = match tunnels.iter().find(|t| t.id == tunnel_id) {
        Some(t) => t,
        None => return Vec::new(),
    };

    if tunnel.tunnel_type != TunnelType::Inbound {
        serial_println!("  I2P: cannot receive on non-inbound tunnel {}", tunnel_id);
        return Vec::new();
    }

    // Strip tunnel layer encryption (reverse order for inbound)
    for layer_key in tunnel.layer_keys.iter().rev() {
        garlic_decrypt(&mut encrypted_data, *layer_key);
    }

    // Decrypt garlic layer
    let garlic_key = i2p_hash(&tunnel_id.to_le_bytes(), GARLIC_KEY_SEED);
    garlic_decrypt(&mut encrypted_data, garlic_key);

    // Parse cloves from decrypted data (simplified: treat entire buffer as one clove)
    let clove = GarlicClove {
        delivery_hash: i2p_hash(&encrypted_data, DESTINATION_SEED),
        data_hash: i2p_hash(&encrypted_data, GARLIC_KEY_SEED),
        delivery_type: CloveDeliveryType::Local,
        expiry: current_tick().wrapping_add(TUNNEL_LIFETIME),
        payload: encrypted_data,
    };

    serial_println!(
        "  I2P: received garlic on tunnel {} (delivery_hash={:#018X})",
        tunnel_id,
        clove.delivery_hash
    );

    vec![clove]
}

/// Publish a LeaseSet advertising our inbound tunnel endpoints.
/// This allows other I2P nodes to find and reach us.
pub fn publish_leaseset(destination_hash: u64) -> bool {
    let tunnels = TUNNELS.lock();
    let inbound_ids: Vec<u64> = tunnels
        .iter()
        .filter(|t| t.tunnel_type == TunnelType::Inbound && t.active)
        .map(|t| t.id)
        .collect();

    if inbound_ids.is_empty() {
        serial_println!("  I2P: no active inbound tunnels for LeaseSet");
        return false;
    }

    let now = current_tick();
    let sig_data: Vec<u8> = inbound_ids.iter().flat_map(|id| id.to_le_bytes()).collect();
    let signature_hash = i2p_hash(&sig_data, destination_hash);

    let leaseset = LeaseSet {
        destination_hash,
        tunnel_ids: inbound_ids.clone(),
        expiry: now.wrapping_add(TUNNEL_LIFETIME),
        signature_hash,
    };

    drop(tunnels);
    let mut leasesets = LEASE_SETS.lock();
    // Remove old LeaseSet for this destination
    leasesets.retain(|ls| ls.destination_hash != destination_hash);
    leasesets.push(leaseset);

    serial_println!(
        "  I2P: LeaseSet published for {:#018X} ({} tunnels, sig={:#018X})",
        destination_hash,
        inbound_ids.len(),
        signature_hash
    );

    true
}

/// Look up a destination in the NetDB to find its LeaseSet.
/// Returns the tunnel IDs that can reach the destination.
pub fn lookup_destination(destination_hash: u64) -> Vec<u64> {
    // First check local LeaseSet cache
    let leasesets = LEASE_SETS.lock();
    if let Some(ls) = leasesets
        .iter()
        .find(|ls| ls.destination_hash == destination_hash)
    {
        let now = current_tick();
        if ls.expiry > now {
            serial_println!(
                "  I2P: destination {:#018X} found in LeaseSet cache ({} tunnels)",
                destination_hash,
                ls.tunnel_ids.len()
            );
            return ls.tunnel_ids.clone();
        }
    }
    drop(leasesets);

    // Check NetDB
    let netdb = NET_DB.lock();
    if let Some(entry) = netdb
        .iter()
        .find(|e| e.key_hash == destination_hash && !e.is_router)
    {
        serial_println!(
            "  I2P: destination {:#018X} found in NetDB (data_hash={:#018X})",
            destination_hash,
            entry.data_hash
        );
        // In a real implementation, we would fetch the LeaseSet from the NetDB
        return Vec::new();
    }

    serial_println!("  I2P: destination {:#018X} not found", destination_hash);
    Vec::new()
}

/// Build a pool of tunnels of the specified type.
/// Maintains the target number of ready tunnels.
pub fn build_tunnel_pool(tunnel_type: TunnelType, target_size: usize) -> u64 {
    let capped_size = if target_size > MAX_POOL_SIZE {
        MAX_POOL_SIZE
    } else {
        target_size
    };

    let mut pool_id_buf = vec![0u8; 9];
    pool_id_buf[0] = tunnel_type as u8;
    pool_id_buf[1..9].copy_from_slice(&(capped_size as u64).to_le_bytes());
    let pool_id = i2p_hash(&pool_id_buf, DESTINATION_SEED);

    let mut tunnel_ids = Vec::new();
    for _ in 0..capped_size {
        if let Some(tid) = create_tunnel(tunnel_type) {
            tunnel_ids.push(tid);
        }
    }

    let pool = TunnelPool {
        id: pool_id,
        tunnel_type,
        tunnel_ids: tunnel_ids.clone(),
        target_size: capped_size,
    };

    let mut pools = TUNNEL_POOLS.lock();
    pools.push(pool);

    let type_str = match tunnel_type {
        TunnelType::Inbound => "inbound",
        TunnelType::Outbound => "outbound",
        TunnelType::Exploratory => "exploratory",
    };

    serial_println!(
        "  I2P: {} tunnel pool created (pool_id={:#018X}, {} tunnels)",
        type_str,
        pool_id,
        tunnel_ids.len()
    );

    pool_id
}

/// Create a GarlicClove with the given destination and data
pub fn make_clove(delivery_hash: u64, data: &[u8]) -> GarlicClove {
    let data_hash = i2p_hash(data, GARLIC_KEY_SEED);
    GarlicClove {
        delivery_hash,
        data_hash,
        delivery_type: CloveDeliveryType::Destination,
        expiry: current_tick().wrapping_add(TUNNEL_LIFETIME),
        payload: Vec::from(data),
    }
}

/// Get tunnel statistics
pub fn get_tunnel_stats() -> Vec<(u64, TunnelType, TunnelStatus, u64, u64)> {
    let tunnels = TUNNELS.lock();
    tunnels
        .iter()
        .map(|t| {
            (
                t.id,
                t.tunnel_type,
                t.status,
                t.bytes_routed,
                t.messages_routed,
            )
        })
        .collect()
}

/// Expire old tunnels and remove them
pub fn expire_tunnels() {
    let now = current_tick();
    let mut tunnels = TUNNELS.lock();
    let before = tunnels.len();
    for tunnel in tunnels.iter_mut() {
        if tunnel.active && now.wrapping_sub(tunnel.created) > TUNNEL_LIFETIME {
            tunnel.active = false;
            tunnel.status = TunnelStatus::Expired;
        }
    }
    tunnels.retain(|t| t.status != TunnelStatus::Expired);
    let removed = before - tunnels.len();
    if removed > 0 {
        serial_println!("  I2P: expired {} tunnels", removed);
    }
}

/// Seed the NetDB with bootstrap router entries
fn seed_netdb() {
    let mut netdb = NET_DB.lock();
    let bootstrap_routers: Vec<(u64, u64)> = vec![
        (0xA1B2C3D4E5F6A7B8, 0x1A2B3C4D5E6F7A8B),
        (0x2C3D4E5F6A7B8C9D, 0xB1C2D3E4F5A6B7C8),
        (0x3D4E5F6A7B8C9DAE, 0xC1D2E3F4A5B6C7D8),
        (0x4E5F6A7B8C9DAEBF, 0xD1E2F3A4B5C6D7E8),
        (0x5F6A7B8C9DAEBF01, 0xE1F2A3B4C5D6E7F8),
    ];

    let now = current_tick();
    for (key, data) in bootstrap_routers {
        netdb.push(NetDbEntry {
            key_hash: key,
            is_router: true,
            data_hash: data,
            updated: now,
        });
    }
}

/// Initialize the I2P subsystem
pub fn init() {
    seed_netdb();

    let netdb_count = {
        let netdb = NET_DB.lock();
        netdb.len()
    };

    serial_println!(
        "  I2P: garlic router initialized ({} NetDB entries)",
        netdb_count
    );
}
