use crate::sync::Mutex;
/// Tor-like Onion Routing Client for Genesis
///
/// Implements a multi-hop onion routing protocol with layered encryption.
/// Each relay in a circuit peels one layer of encryption, forwarding the
/// inner payload to the next hop. The exit node delivers traffic to the
/// destination. Circuits are built incrementally via extend operations.
///
/// Cell types: Create, Created, Relay, Destroy, Padding
/// Stream multiplexing over circuits for concurrent connections.
/// DNS resolution tunneled through exit nodes to prevent leakage.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of hops in a standard circuit
const DEFAULT_HOP_COUNT: usize = 3;

/// Maximum streams per circuit
const MAX_STREAMS_PER_CIRCUIT: usize = 64;

/// Cell payload size in bytes
const CELL_PAYLOAD_SIZE: usize = 509;

/// Circuit idle timeout in ticks (simulated)
const CIRCUIT_IDLE_TIMEOUT: u64 = 600;

/// Hash seed for onion layer key derivation
const ONION_KEY_SEED: u64 = 0xA1B2C3D4E5F60718;

/// Relay flag bits
const FLAG_GUARD: u32 = 0x0001;
const FLAG_EXIT: u32 = 0x0002;
const FLAG_STABLE: u32 = 0x0004;
const FLAG_FAST: u32 = 0x0008;
const FLAG_RUNNING: u32 = 0x0010;
const FLAG_VALID: u32 = 0x0020;
const FLAG_HSDIR: u32 = 0x0040;
const FLAG_AUTHORITY: u32 = 0x0080;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Status of a Tor circuit through its lifecycle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitStatus {
    /// Circuit is being constructed (extending hops)
    Building,
    /// Circuit is fully built and ready for traffic
    Open,
    /// Circuit has been torn down gracefully
    Closed,
    /// Circuit encountered an error and is unusable
    Failed,
}

/// Type of cell transmitted along a circuit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellType {
    /// Initiate circuit creation at first hop
    Create,
    /// Acknowledgement of circuit creation
    Created,
    /// Relay cell carrying stream data (encrypted per hop)
    Relay,
    /// Tear down the circuit
    Destroy,
    /// Padding cell for traffic analysis resistance
    Padding,
}

/// A relay node in the Tor network directory
#[derive(Debug, Clone)]
pub struct RelayNode {
    /// Unique identifier for this relay
    pub id: u64,
    /// Hash of the relay network address
    pub addr_hash: u64,
    /// Hash of the relay public key (used for onion encryption)
    pub public_key_hash: u64,
    /// Advertised bandwidth in KB/s
    pub bandwidth: u32,
    /// Relay capability flags (guard, exit, stable, etc.)
    pub flags: u32,
}

/// A cell transmitted along a circuit
#[derive(Debug, Clone)]
pub struct TorCell {
    /// Circuit this cell belongs to
    pub circuit_id: u64,
    /// Type of cell
    pub cell_type: CellType,
    /// Stream ID within the circuit (0 for circuit-level cells)
    pub stream_id: u16,
    /// Payload data (up to CELL_PAYLOAD_SIZE bytes)
    pub payload: Vec<u8>,
    /// Integrity digest of the payload
    pub digest: u64,
}

/// A stream multiplexed within a circuit
#[derive(Debug, Clone)]
pub struct TorStream {
    /// Stream identifier within the circuit
    pub stream_id: u16,
    /// Hash of the destination address
    pub dest_hash: u64,
    /// Destination port
    pub dest_port: u16,
    /// Whether the stream is active
    pub active: bool,
    /// Bytes sent on this stream
    pub bytes_sent: u64,
    /// Bytes received on this stream
    pub bytes_received: u64,
}

/// An onion routing circuit through multiple relay nodes
#[derive(Debug, Clone)]
pub struct TorCircuit {
    /// Unique circuit identifier
    pub id: u64,
    /// Ordered list of relay nodes in the circuit path
    pub nodes: Vec<RelayNode>,
    /// Tick count when the circuit was created
    pub created: u64,
    /// Total bytes sent through this circuit
    pub bytes_sent: u64,
    /// Current circuit status
    pub status: CircuitStatus,
    /// Per-hop session key hashes (one per node)
    session_keys: Vec<u64>,
    /// Streams multiplexed over this circuit
    streams: Vec<TorStream>,
    /// Next available stream ID
    next_stream_id: u16,
    /// Last activity tick for idle detection
    last_activity: u64,
}

/// Exit policy entry — what destinations an exit node allows
#[derive(Debug, Clone)]
pub struct ExitPolicy {
    /// Whether this rule accepts or rejects
    pub accept: bool,
    /// Destination address hash (0 = wildcard)
    pub addr_hash: u64,
    /// Port range start
    pub port_start: u16,
    /// Port range end
    pub port_end: u16,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Active circuits
static CIRCUITS: Mutex<Vec<TorCircuit>> = Mutex::new(Vec::new());

/// Known relay directory
static RELAY_DIRECTORY: Mutex<Vec<RelayNode>> = Mutex::new(Vec::new());

/// Next circuit ID counter
static NEXT_CIRCUIT_ID: Mutex<u64> = Mutex::new(1);

/// Simulated tick counter for timestamps
static TICK_COUNTER: Mutex<u64> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Simple hash function for key derivation and integrity checks.
/// Produces a deterministic u64 from a byte slice.
fn tor_hash(data: &[u8], seed: u64) -> u64 {
    let mut h: u64 = seed;
    for &b in data {
        h = h.wrapping_mul(0x517CC1B727220A95).wrapping_add(b as u64);
        h ^= h >> 17;
    }
    h
}

/// Derive a session key hash for a given hop index and circuit ID
fn derive_session_key(circuit_id: u64, hop_index: usize, relay_key_hash: u64) -> u64 {
    let mut buf = vec![0u8; 24];
    buf[0..8].copy_from_slice(&circuit_id.to_le_bytes());
    buf[8..16].copy_from_slice(&(hop_index as u64).to_le_bytes());
    buf[16..24].copy_from_slice(&relay_key_hash.to_le_bytes());
    tor_hash(&buf, ONION_KEY_SEED)
}

/// Apply one layer of onion encryption (XOR-based simulation)
fn onion_encrypt_layer(data: &mut [u8], key_hash: u64) {
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] ^= key_bytes[i % 8];
        // Rotate key influence per byte position
        data[i] = data[i].wrapping_add((key_hash >> ((i % 7) * 8)) as u8);
    }
}

/// Remove one layer of onion encryption (inverse of encrypt)
fn onion_decrypt_layer(data: &mut [u8], key_hash: u64) {
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] = data[i].wrapping_sub((key_hash >> ((i % 7) * 8)) as u8);
        data[i] ^= key_bytes[i % 8];
    }
}

/// Get current simulated tick
fn current_tick() -> u64 {
    let tick = TICK_COUNTER.lock();
    *tick
}

/// Advance the tick counter
fn advance_tick() {
    let mut tick = TICK_COUNTER.lock();
    *tick = tick.wrapping_add(1);
}

/// Select relay nodes for a circuit path from the directory.
/// Picks a guard node first, middle nodes, then an exit node.
fn select_path(hop_count: usize) -> Vec<RelayNode> {
    let dir = RELAY_DIRECTORY.lock();
    let mut path = Vec::new();

    if dir.len() < hop_count {
        return path;
    }

    // Select guard (first hop)
    for relay in dir.iter() {
        if relay.flags & FLAG_GUARD != 0 && relay.flags & FLAG_RUNNING != 0 {
            path.push(relay.clone());
            break;
        }
    }

    // Select middle relays
    for relay in dir.iter() {
        if path.len() >= hop_count - 1 {
            break;
        }
        if relay.flags & FLAG_GUARD == 0
            && relay.flags & FLAG_EXIT == 0
            && relay.flags & FLAG_RUNNING != 0
            && !path.iter().any(|n: &RelayNode| n.id == relay.id)
        {
            path.push(relay.clone());
        }
    }

    // Select exit (last hop)
    for relay in dir.iter() {
        if relay.flags & FLAG_EXIT != 0
            && relay.flags & FLAG_RUNNING != 0
            && !path.iter().any(|n: &RelayNode| n.id == relay.id)
        {
            path.push(relay.clone());
            break;
        }
    }

    path
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a new Tor circuit with the default number of hops.
/// Returns the circuit ID on success.
pub fn build_circuit() -> Option<u64> {
    let path = select_path(DEFAULT_HOP_COUNT);
    if path.len() < DEFAULT_HOP_COUNT {
        serial_println!(
            "  Tor: insufficient relays for circuit (have {}, need {})",
            path.len(),
            DEFAULT_HOP_COUNT
        );
        return None;
    }

    let mut id_counter = NEXT_CIRCUIT_ID.lock();
    let circuit_id = *id_counter;
    *id_counter = id_counter.wrapping_add(1);
    drop(id_counter);

    let now = current_tick();

    // Derive session keys for each hop
    let mut session_keys = Vec::new();
    for (i, node) in path.iter().enumerate() {
        let key = derive_session_key(circuit_id, i, node.public_key_hash);
        session_keys.push(key);
    }

    let circuit = TorCircuit {
        id: circuit_id,
        nodes: path.clone(),
        created: now,
        bytes_sent: 0,
        status: CircuitStatus::Open,
        session_keys,
        streams: Vec::new(),
        next_stream_id: 1,
        last_activity: now,
    };

    let mut circuits = CIRCUITS.lock();
    circuits.push(circuit);

    serial_println!(
        "  Tor: circuit {} built ({} hops: {}->{}->{})",
        circuit_id,
        path.len(),
        path[0].id,
        path[1].id,
        path[2].id
    );

    Some(circuit_id)
}

/// Extend an existing circuit by adding another relay hop.
/// Returns true on success.
pub fn extend_circuit(circuit_id: u64) -> bool {
    let dir = RELAY_DIRECTORY.lock();
    let mut circuits = CIRCUITS.lock();

    let circuit = match circuits.iter_mut().find(|c| c.id == circuit_id) {
        Some(c) => c,
        None => return false,
    };

    if circuit.status != CircuitStatus::Open {
        return false;
    }

    // Find a relay not already in the circuit
    let existing_ids: Vec<u64> = circuit.nodes.iter().map(|n| n.id).collect();
    let new_relay = dir
        .iter()
        .find(|r| r.flags & FLAG_RUNNING != 0 && !existing_ids.contains(&r.id));

    match new_relay {
        Some(relay) => {
            let hop_index = circuit.nodes.len();
            let key = derive_session_key(circuit_id, hop_index, relay.public_key_hash);
            circuit.session_keys.push(key);
            circuit.nodes.push(relay.clone());
            circuit.last_activity = current_tick();
            serial_println!(
                "  Tor: circuit {} extended to {} hops",
                circuit_id,
                circuit.nodes.len()
            );
            true
        }
        None => {
            serial_println!("  Tor: no available relay to extend circuit {}", circuit_id);
            false
        }
    }
}

/// Send a relay cell through a circuit with full onion encryption.
/// Data is encrypted in layers: outermost layer for the guard, innermost for exit.
/// Returns the number of bytes sent.
pub fn send_cell(circuit_id: u64, stream_id: u16, data: &[u8]) -> usize {
    let mut circuits = CIRCUITS.lock();
    let circuit = match circuits.iter_mut().find(|c| c.id == circuit_id) {
        Some(c) => c,
        None => return 0,
    };

    if circuit.status != CircuitStatus::Open {
        return 0;
    }

    // Pad or truncate data to cell payload size
    let mut payload = vec![0u8; CELL_PAYLOAD_SIZE];
    let copy_len = data.len().min(CELL_PAYLOAD_SIZE);
    payload[..copy_len].copy_from_slice(&data[..copy_len]);

    // Calculate integrity digest before encryption
    let digest = tor_hash(&payload, circuit.id);

    // Apply onion encryption layers (innermost key first, outermost last)
    let num_hops = circuit.session_keys.len();
    for i in (0..num_hops).rev() {
        onion_encrypt_layer(&mut payload, circuit.session_keys[i]);
    }

    let cell = TorCell {
        circuit_id,
        cell_type: CellType::Relay,
        stream_id,
        payload,
        digest,
    };

    circuit.bytes_sent = circuit.bytes_sent.wrapping_add(cell.payload.len() as u64);
    circuit.last_activity = current_tick();

    // Update stream stats if present
    if let Some(stream) = circuit
        .streams
        .iter_mut()
        .find(|s| s.stream_id == stream_id)
    {
        stream.bytes_sent = stream.bytes_sent.wrapping_add(copy_len as u64);
    }

    serial_println!(
        "  Tor: cell sent on circuit {} stream {} ({} bytes, digest={:#018X})",
        circuit_id,
        stream_id,
        copy_len,
        cell.digest
    );

    copy_len
}

/// Destroy a circuit, tearing down all hops.
pub fn destroy_circuit(circuit_id: u64) {
    let mut circuits = CIRCUITS.lock();
    if let Some(circuit) = circuits.iter_mut().find(|c| c.id == circuit_id) {
        circuit.status = CircuitStatus::Closed;
        // Clear session keys for security
        for key in circuit.session_keys.iter_mut() {
            *key = 0;
        }
        circuit.streams.clear();
        serial_println!(
            "  Tor: circuit {} destroyed (sent {} bytes total)",
            circuit_id,
            circuit.bytes_sent
        );
    }
}

/// Create a new stream on an existing circuit to a destination.
/// Returns the stream ID on success.
pub fn create_stream(circuit_id: u64, dest_hash: u64, dest_port: u16) -> Option<u16> {
    let mut circuits = CIRCUITS.lock();
    let circuit = match circuits.iter_mut().find(|c| c.id == circuit_id) {
        Some(c) => c,
        None => return None,
    };

    if circuit.status != CircuitStatus::Open {
        return None;
    }

    if circuit.streams.len() >= MAX_STREAMS_PER_CIRCUIT {
        serial_println!(
            "  Tor: circuit {} at max streams ({})",
            circuit_id,
            MAX_STREAMS_PER_CIRCUIT
        );
        return None;
    }

    let stream_id = circuit.next_stream_id;
    circuit.next_stream_id = circuit.next_stream_id.wrapping_add(1);

    let stream = TorStream {
        stream_id,
        dest_hash,
        dest_port,
        active: true,
        bytes_sent: 0,
        bytes_received: 0,
    };

    circuit.streams.push(stream);
    circuit.last_activity = current_tick();

    serial_println!(
        "  Tor: stream {} created on circuit {} (dest_hash={:#018X}, port={})",
        stream_id,
        circuit_id,
        dest_hash,
        dest_port
    );

    Some(stream_id)
}

/// Resolve a DNS name by tunneling the query through the circuit exit node.
/// Returns a simulated address hash (prevents DNS leakage).
pub fn resolve_dns_over_tor(circuit_id: u64, hostname_hash: u64) -> Option<u64> {
    let circuits = CIRCUITS.lock();
    let circuit = match circuits.iter().find(|c| c.id == circuit_id) {
        Some(c) => c,
        None => return None,
    };

    if circuit.status != CircuitStatus::Open || circuit.nodes.is_empty() {
        return None;
    }

    // Simulate DNS resolution through exit node
    let exit_node = &circuit.nodes[circuit.nodes.len() - 1];
    if exit_node.flags & FLAG_EXIT == 0 {
        serial_println!(
            "  Tor: circuit {} exit node does not support DNS resolution",
            circuit_id
        );
        return None;
    }

    // Derive a resolved address hash from the exit node and hostname
    let mut buf = vec![0u8; 16];
    buf[0..8].copy_from_slice(&hostname_hash.to_le_bytes());
    buf[8..16].copy_from_slice(&exit_node.public_key_hash.to_le_bytes());
    let resolved = tor_hash(&buf, 0x0D5E1ACA5E1EC7ED);

    serial_println!(
        "  Tor: DNS resolved via circuit {} exit (hostname_hash={:#018X} -> {:#018X})",
        circuit_id,
        hostname_hash,
        resolved
    );

    Some(resolved)
}

/// Get the exit policy for the exit node of a given circuit.
/// Returns a list of policy entries describing what traffic the exit allows.
pub fn get_exit_policy(circuit_id: u64) -> Vec<ExitPolicy> {
    let circuits = CIRCUITS.lock();
    let circuit = match circuits.iter().find(|c| c.id == circuit_id) {
        Some(c) => c,
        None => return Vec::new(),
    };

    if circuit.nodes.is_empty() {
        return Vec::new();
    }

    let exit_node = &circuit.nodes[circuit.nodes.len() - 1];
    let mut policies = Vec::new();

    // Generate default exit policy based on relay flags
    if exit_node.flags & FLAG_EXIT != 0 {
        // Accept HTTP
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 80,
            port_end: 80,
        });
        // Accept HTTPS
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 443,
            port_end: 443,
        });
        // Accept SSH
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 22,
            port_end: 22,
        });
        // Accept DNS
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 53,
            port_end: 53,
        });
        // Accept email ports
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 25,
            port_end: 25,
        });
        policies.push(ExitPolicy {
            accept: true,
            addr_hash: 0,
            port_start: 587,
            port_end: 587,
        });
        // Reject everything else
        policies.push(ExitPolicy {
            accept: false,
            addr_hash: 0,
            port_start: 0,
            port_end: 65535,
        });
    } else {
        // Non-exit: reject all
        policies.push(ExitPolicy {
            accept: false,
            addr_hash: 0,
            port_start: 0,
            port_end: 65535,
        });
    }

    policies
}

/// Get statistics for all active circuits
pub fn get_circuit_stats() -> Vec<(u64, CircuitStatus, usize, u64)> {
    let circuits = CIRCUITS.lock();
    circuits
        .iter()
        .map(|c| (c.id, c.status, c.nodes.len(), c.bytes_sent))
        .collect()
}

/// Perform garbage collection on closed/failed circuits
pub fn gc_circuits() {
    let mut circuits = CIRCUITS.lock();
    let before = circuits.len();
    circuits.retain(|c| c.status == CircuitStatus::Open || c.status == CircuitStatus::Building);
    let removed = before - circuits.len();
    if removed > 0 {
        serial_println!("  Tor: GC removed {} closed/failed circuits", removed);
    }
}

/// Populate the relay directory with initial bootstrap nodes
fn populate_directory() {
    let mut dir = RELAY_DIRECTORY.lock();

    let relays = vec![
        RelayNode {
            id: 1,
            addr_hash: 0xAB12CD34EF560718,
            public_key_hash: 0x1A2B3C4D5E6F0A1B,
            bandwidth: 5000,
            flags: FLAG_GUARD | FLAG_STABLE | FLAG_FAST | FLAG_RUNNING | FLAG_VALID,
        },
        RelayNode {
            id: 2,
            addr_hash: 0x1122334455660718,
            public_key_hash: 0x2A3B4C5D6E0F1A2B,
            bandwidth: 3000,
            flags: FLAG_STABLE | FLAG_FAST | FLAG_RUNNING | FLAG_VALID,
        },
        RelayNode {
            id: 3,
            addr_hash: 0xAABBCCDDEE0F1A2B,
            public_key_hash: 0x3A4B5C6D0E1F2A3B,
            bandwidth: 4000,
            flags: FLAG_STABLE | FLAG_RUNNING | FLAG_VALID,
        },
        RelayNode {
            id: 4,
            addr_hash: 0xFEDCBA9801234567,
            public_key_hash: 0x4A5B6C0D1E2F3A4B,
            bandwidth: 6000,
            flags: FLAG_EXIT | FLAG_STABLE | FLAG_FAST | FLAG_RUNNING | FLAG_VALID,
        },
        RelayNode {
            id: 5,
            addr_hash: 0x0A1B2C3D4E5F6A7B,
            public_key_hash: 0x5A6B0C1D2E3F4A5B,
            bandwidth: 2500,
            flags: FLAG_GUARD | FLAG_RUNNING | FLAG_VALID | FLAG_HSDIR,
        },
        RelayNode {
            id: 6,
            addr_hash: 0x1234567890ABCDEF,
            public_key_hash: 0x6A0B1C2D3E4F5A6B,
            bandwidth: 3500,
            flags: FLAG_EXIT | FLAG_RUNNING | FLAG_VALID,
        },
        RelayNode {
            id: 7,
            addr_hash: 0xFEDCBA0123456789,
            public_key_hash: 0x0A1B2C3D4E5F6A7B,
            bandwidth: 4500,
            flags: FLAG_STABLE | FLAG_FAST | FLAG_RUNNING | FLAG_VALID | FLAG_AUTHORITY,
        },
    ];

    for relay in relays {
        dir.push(relay);
    }
}

/// Initialize the Tor client subsystem
pub fn init() {
    populate_directory();
    advance_tick();

    let dir_count = {
        let dir = RELAY_DIRECTORY.lock();
        dir.len()
    };

    serial_println!(
        "  Tor: client initialized ({} relays in directory)",
        dir_count
    );
}
