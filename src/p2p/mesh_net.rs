/// Mesh Networking Layer for Genesis
///
/// Multi-hop mesh routing with flooding, path discovery,
/// neighbor management, and topology awareness.
/// Signal strength uses Q16 fixed-point (i32, 16 fractional bits).
///
/// All code is original. No external crates.
use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

/// Q16 fixed-point type: 1.0 == 65536
pub type Q16 = i32;

const Q16_ONE: Q16 = 1 << 16; // 65536 = 1.0
const Q16_ZERO: Q16 = 0;
const Q16_HALF: Q16 = 1 << 15; // 32768 = 0.5

/// Multiply two Q16 values: (a * b) >> 16
fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_NODES: usize = 256;
const MAX_NEIGHBORS: usize = 16;
const MAX_PACKETS: usize = 512;
const DEFAULT_TTL: u8 = 16;
const MAX_HOPS: usize = 32;
const FLOOD_CACHE_SIZE: usize = 128;
const SIGNAL_DECAY_PER_HOP: Q16 = 6554; // ~0.1 in Q16

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A node participating in the mesh network.
#[derive(Clone)]
pub struct MeshNode {
    pub id: u64,
    pub addr_hash: u64,
    pub neighbors: Vec<u64>,
    pub last_seen: u64,
    pub hop_count: u8,
    pub signal_strength: Q16,
}

/// A packet traversing the mesh.
#[derive(Clone)]
pub struct MeshPacket {
    pub src: u64,
    pub dst: u64,
    pub ttl: u8,
    pub data_hash: u64,
    pub hops: Vec<u64>,
    pub timestamp: u64,
}

/// Result of a topology query.
pub struct TopologySnapshot {
    pub node_count: usize,
    pub edge_count: usize,
    pub avg_neighbors: Q16,
    pub diameter_estimate: u8,
}

/// Result of a path search.
pub struct PathResult {
    pub found: bool,
    pub path: Vec<u64>,
    pub hop_count: u8,
    pub total_signal: Q16,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MESH_NODES: Mutex<Option<Vec<MeshNode>>> = Mutex::new(None);
static PACKET_LOG: Mutex<Option<Vec<MeshPacket>>> = Mutex::new(None);
static FLOOD_SEEN: Mutex<Option<Vec<u64>>> = Mutex::new(None);
static LOCAL_NODE_ID: Mutex<Option<u64>> = Mutex::new(None);
static MESH_ACTIVE: Mutex<bool> = Mutex::new(false);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    {
        let mut nodes = MESH_NODES.lock();
        *nodes = Some(Vec::new());
    }
    {
        let mut log = PACKET_LOG.lock();
        *log = Some(Vec::new());
    }
    {
        let mut seen = FLOOD_SEEN.lock();
        *seen = Some(Vec::new());
    }
    serial_println!("    mesh_net: initialized");
}

// ---------------------------------------------------------------------------
// Mesh join / leave
// ---------------------------------------------------------------------------

/// Join the mesh with the given node id and address hash.
pub fn join_mesh(id: u64, addr_hash: u64) -> bool {
    {
        let mut local = LOCAL_NODE_ID.lock();
        *local = Some(id);
    }

    let node = MeshNode {
        id,
        addr_hash,
        neighbors: Vec::new(),
        last_seen: 0,
        hop_count: 0,
        signal_strength: Q16_ONE,
    };

    let mut nodes = MESH_NODES.lock();
    if let Some(ref mut list) = *nodes {
        // Reject if mesh is full
        if list.len() >= MAX_NODES {
            return false;
        }
        // Reject duplicate
        for n in list.iter() {
            if n.id == id {
                return false;
            }
        }
        list.push(node);
    }

    let mut active = MESH_ACTIVE.lock();
    *active = true;
    true
}

/// Leave the mesh. Removes the local node and clears state.
pub fn leave_mesh() {
    let local_id;
    {
        let local = LOCAL_NODE_ID.lock();
        local_id = *local;
    }

    if let Some(id) = local_id {
        let mut nodes = MESH_NODES.lock();
        if let Some(ref mut list) = *nodes {
            list.retain(|n| n.id != id);
            // Remove from neighbor lists of remaining nodes
            for n in list.iter_mut() {
                n.neighbors.retain(|&nid| nid != id);
            }
        }
    }

    let mut local = LOCAL_NODE_ID.lock();
    *local = None;
    let mut active = MESH_ACTIVE.lock();
    *active = false;
}

// ---------------------------------------------------------------------------
// Neighbor management
// ---------------------------------------------------------------------------

/// Add a neighbor to a given node. Returns true on success.
pub fn add_neighbor(node_id: u64, neighbor_id: u64) -> bool {
    let mut nodes = MESH_NODES.lock();
    if let Some(ref mut list) = *nodes {
        // Verify neighbor exists in mesh
        let neighbor_exists = list.iter().any(|n| n.id == neighbor_id);
        if !neighbor_exists {
            return false;
        }

        for n in list.iter_mut() {
            if n.id == node_id {
                if n.neighbors.len() >= MAX_NEIGHBORS {
                    return false;
                }
                if n.neighbors.contains(&neighbor_id) {
                    return false;
                }
                n.neighbors.push(neighbor_id);
                return true;
            }
        }
    }
    false
}

/// Remove a neighbor from a given node.
pub fn remove_neighbor(node_id: u64, neighbor_id: u64) -> bool {
    let mut nodes = MESH_NODES.lock();
    if let Some(ref mut list) = *nodes {
        for n in list.iter_mut() {
            if n.id == node_id {
                let before = n.neighbors.len();
                n.neighbors.retain(|&nid| nid != neighbor_id);
                return n.neighbors.len() < before;
            }
        }
    }
    false
}

/// Update the last-seen timestamp for a node.
pub fn touch_node(node_id: u64, timestamp: u64) {
    let mut nodes = MESH_NODES.lock();
    if let Some(ref mut list) = *nodes {
        for n in list.iter_mut() {
            if n.id == node_id {
                n.last_seen = timestamp;
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Packet sending / routing
// ---------------------------------------------------------------------------

/// Send a packet from src to dst through the mesh.
/// Returns true if the packet was accepted for routing.
pub fn send_packet(src: u64, dst: u64, data_hash: u64, timestamp: u64) -> bool {
    let active = MESH_ACTIVE.lock();
    if !*active {
        return false;
    }
    drop(active);

    let packet = MeshPacket {
        src,
        dst,
        ttl: DEFAULT_TTL,
        data_hash,
        hops: vec![src],
        timestamp,
    };

    route_packet(packet)
}

/// Route a packet towards its destination using greedy forwarding.
/// Falls back to flooding if no direct path is found.
pub fn route_packet(mut packet: MeshPacket) -> bool {
    if packet.ttl == 0 {
        return false;
    }
    packet.ttl -= 1;

    let current_node = match packet.hops.last() {
        Some(&id) => id,
        None => return false,
    };

    // Check if we reached the destination
    if current_node == packet.dst {
        log_packet(&packet);
        return true;
    }

    // Guard against hop limit
    if packet.hops.len() >= MAX_HOPS {
        return false;
    }

    // Try direct neighbor forwarding
    let next_hop = find_best_next_hop(current_node, packet.dst);

    if let Some(next) = next_hop {
        packet.hops.push(next);
        log_packet(&packet);
        return route_packet(packet);
    }

    // Fallback: flood the packet
    flood(packet)
}

/// Find the best next hop from current toward dst using XOR distance.
fn find_best_next_hop(current: u64, dst: u64) -> Option<u64> {
    let nodes = MESH_NODES.lock();
    if let Some(ref list) = *nodes {
        for n in list.iter() {
            if n.id == current {
                let mut best_id: Option<u64> = None;
                let mut best_dist: u64 = current ^ dst; // distance from current

                for &neighbor in &n.neighbors {
                    let dist = neighbor ^ dst;
                    if dist < best_dist {
                        best_dist = dist;
                        best_id = Some(neighbor);
                    }
                }
                return best_id;
            }
        }
    }
    None
}

/// Flood a packet to all neighbors (controlled flooding with seen-cache).
pub fn flood(packet: MeshPacket) -> bool {
    // Check if already flooded this packet
    {
        let mut seen = FLOOD_SEEN.lock();
        if let Some(ref mut cache) = *seen {
            let flood_id = packet.src ^ packet.data_hash ^ (packet.timestamp & 0xFFFF_FFFF);
            if cache.contains(&flood_id) {
                return false;
            }
            if cache.len() >= FLOOD_CACHE_SIZE {
                cache.remove(0);
            }
            cache.push(flood_id);
        }
    }

    let current = match packet.hops.last() {
        Some(&id) => id,
        None => return false,
    };

    let neighbor_ids: Vec<u64>;
    {
        let nodes = MESH_NODES.lock();
        if let Some(ref list) = *nodes {
            let node = list.iter().find(|n| n.id == current);
            neighbor_ids = match node {
                Some(n) => n.neighbors.clone(),
                None => return false,
            };
        } else {
            return false;
        }
    }

    let mut any_sent = false;
    for &nid in &neighbor_ids {
        if packet.hops.contains(&nid) {
            continue; // skip already-visited
        }
        let mut forwarded = packet.clone();
        forwarded.hops.push(nid);
        if forwarded.ttl > 0 {
            forwarded.ttl -= 1;
            log_packet(&forwarded);
            any_sent = true;
        }
    }
    any_sent
}

/// Record a packet in the packet log.
fn log_packet(packet: &MeshPacket) {
    let mut log = PACKET_LOG.lock();
    if let Some(ref mut list) = *log {
        if list.len() >= MAX_PACKETS {
            list.remove(0);
        }
        list.push(packet.clone());
    }
}

// ---------------------------------------------------------------------------
// Path finding (BFS)
// ---------------------------------------------------------------------------

/// Find the shortest path between two nodes using breadth-first search.
pub fn find_path(src: u64, dst: u64) -> PathResult {
    let nodes = MESH_NODES.lock();
    let list = match *nodes {
        Some(ref l) => l,
        None => {
            return PathResult {
                found: false,
                path: Vec::new(),
                hop_count: 0,
                total_signal: Q16_ZERO,
            };
        }
    };

    // BFS
    let mut queue: Vec<Vec<u64>> = vec![vec![src]];
    let mut visited: Vec<u64> = vec![src];

    while !queue.is_empty() {
        let path = queue.remove(0);
        let current = match path.last() {
            Some(&id) => id,
            None => continue,
        };

        if current == dst {
            let hop_count = if path.len() > 1 {
                (path.len() - 1) as u8
            } else {
                0
            };
            let total_signal = compute_path_signal(&path, list);
            return PathResult {
                found: true,
                path,
                hop_count,
                total_signal,
            };
        }

        // Find current node's neighbors
        let node = list.iter().find(|n| n.id == current);
        if let Some(n) = node {
            for &neighbor in &n.neighbors {
                if !visited.contains(&neighbor) {
                    visited.push(neighbor);
                    let mut new_path = path.clone();
                    new_path.push(neighbor);
                    queue.push(new_path);
                }
            }
        }
    }

    PathResult {
        found: false,
        path: Vec::new(),
        hop_count: 0,
        total_signal: Q16_ZERO,
    }
}

/// Compute aggregate signal strength along a path, decaying per hop.
fn compute_path_signal(path: &[u64], nodes: &[MeshNode]) -> Q16 {
    let mut signal = Q16_ONE;
    for &node_id in path.iter() {
        if let Some(n) = nodes.iter().find(|n| n.id == node_id) {
            signal = q16_mul(signal, n.signal_strength);
        }
        // Decay per hop
        signal = signal.saturating_sub(SIGNAL_DECAY_PER_HOP);
        if signal < Q16_ZERO {
            signal = Q16_ZERO;
        }
    }
    signal
}

// ---------------------------------------------------------------------------
// Topology
// ---------------------------------------------------------------------------

/// Get a snapshot of the current mesh topology.
pub fn get_topology() -> TopologySnapshot {
    let nodes = MESH_NODES.lock();
    let list = match *nodes {
        Some(ref l) => l,
        None => {
            return TopologySnapshot {
                node_count: 0,
                edge_count: 0,
                avg_neighbors: Q16_ZERO,
                diameter_estimate: 0,
            };
        }
    };

    let node_count = list.len();
    if node_count == 0 {
        return TopologySnapshot {
            node_count: 0,
            edge_count: 0,
            avg_neighbors: Q16_ZERO,
            diameter_estimate: 0,
        };
    }

    let mut edge_count: usize = 0;
    let mut total_neighbors: usize = 0;
    let mut max_hop: u8 = 0;

    for n in list.iter() {
        edge_count += n.neighbors.len();
        total_neighbors += n.neighbors.len();
        if n.hop_count > max_hop {
            max_hop = n.hop_count;
        }
    }

    // Edges are counted from both sides, so divide by 2
    edge_count /= 2;

    // Average neighbors in Q16: (total * 65536) / node_count
    let avg_neighbors = if node_count > 0 {
        ((total_neighbors as i64 * Q16_ONE as i64) / node_count as i64) as Q16
    } else {
        Q16_ZERO
    };

    TopologySnapshot {
        node_count,
        edge_count,
        avg_neighbors,
        diameter_estimate: max_hop,
    }
}

/// Get the number of active nodes in the mesh.
pub fn node_count() -> usize {
    let nodes = MESH_NODES.lock();
    match *nodes {
        Some(ref l) => l.len(),
        None => 0,
    }
}

/// Get total number of logged packets.
pub fn packet_count() -> usize {
    let log = PACKET_LOG.lock();
    match *log {
        Some(ref l) => l.len(),
        None => 0,
    }
}

/// Check if the mesh is currently active.
pub fn is_active() -> bool {
    let active = MESH_ACTIVE.lock();
    *active
}

/// Prune nodes not seen since the given cutoff timestamp.
pub fn prune_stale_nodes(cutoff: u64) -> usize {
    let mut nodes = MESH_NODES.lock();
    if let Some(ref mut list) = *nodes {
        let before = list.len();
        let stale_ids: Vec<u64> = list
            .iter()
            .filter(|n| n.last_seen < cutoff && n.hop_count > 0)
            .map(|n| n.id)
            .collect();

        list.retain(|n| !stale_ids.contains(&n.id));

        // Clean neighbor lists
        for n in list.iter_mut() {
            n.neighbors.retain(|nid| !stale_ids.contains(nid));
        }

        before - list.len()
    } else {
        0
    }
}

/// Get all neighbor IDs for a given node.
pub fn get_neighbors(node_id: u64) -> Vec<u64> {
    let nodes = MESH_NODES.lock();
    if let Some(ref list) = *nodes {
        for n in list.iter() {
            if n.id == node_id {
                return n.neighbors.clone();
            }
        }
    }
    Vec::new()
}
