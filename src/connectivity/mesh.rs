use crate::sync::Mutex;
use alloc::string::String;
/// Mesh networking -- 802.11s wireless mesh
///
/// Self-healing, multi-hop wireless mesh network.
/// Part of the AIOS connectivity layer.
/// Implements HWMP (Hybrid Wireless Mesh Protocol) inspired routing,
/// path discovery, link metric computation, peer management,
/// and mesh point coordination.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Mesh node in the network
pub struct MeshNode {
    pub mac: [u8; 6],
    pub hop_count: u8,
    pub metric: u32,
    /// Last sequence number from this node
    sequence: u32,
    /// Time since last heard (ticks)
    last_seen: u64,
    /// Whether this is a direct (1-hop) neighbor
    is_neighbor: bool,
    /// Link quality indicator (0-100)
    link_quality: u8,
    /// Signal strength (dBm)
    signal_dbm: i8,
    /// Next hop MAC for reaching this node
    next_hop: [u8; 6],
    /// Whether a valid path exists to this node
    path_valid: bool,
    /// Path expiry time (ticks)
    path_expiry: u64,
}

impl MeshNode {
    fn new(mac: [u8; 6]) -> Self {
        MeshNode {
            mac,
            hop_count: 0,
            metric: u32::MAX,
            sequence: 0,
            last_seen: 0,
            is_neighbor: false,
            link_quality: 0,
            signal_dbm: -100,
            next_hop: [0u8; 6],
            path_valid: false,
            path_expiry: 0,
        }
    }

    fn update_link(&mut self, signal_dbm: i8, tick: u64) {
        self.signal_dbm = signal_dbm;
        self.last_seen = tick;
        // Compute link quality from signal strength
        // Map -100dBm...-30dBm to 0...100
        self.link_quality = if signal_dbm <= -100 {
            0
        } else if signal_dbm >= -30 {
            100
        } else {
            ((signal_dbm + 100) as u8 * 100 / 70).min(100)
        };
    }
}

/// HWMP Path Request (PREQ) element
#[derive(Clone)]
struct PathRequest {
    /// Originator MAC
    originator: [u8; 6],
    /// Originator sequence number
    originator_seq: u32,
    /// Target MAC
    target: [u8; 6],
    /// Target sequence number (0 if unknown)
    target_seq: u32,
    /// Accumulated metric
    metric: u32,
    /// Hop count
    hop_count: u8,
    /// Time to live
    ttl: u8,
    /// PREQ ID (for deduplication)
    preq_id: u32,
    /// Whether target-only flag is set
    target_only: bool,
}

/// HWMP Path Reply (PREP) element
#[derive(Clone)]
struct PathReply {
    /// Target MAC (who the path goes to)
    target: [u8; 6],
    target_seq: u32,
    /// Originator MAC (who requested the path)
    originator: [u8; 6],
    /// Accumulated metric
    metric: u32,
    hop_count: u8,
    ttl: u8,
}

/// HWMP Path Error (PERR) element
#[derive(Clone)]
struct PathError {
    /// Unreachable destination MAC
    destination: [u8; 6],
    destination_seq: u32,
    /// Reason code
    reason: u8,
}

/// Mesh gate (bridge to external networks)
#[derive(Clone)]
struct MeshGate {
    mac: [u8; 6],
    metric: u32,
    reachable: bool,
}

/// Mesh network manager
pub struct MeshManager {
    mesh_id: String,
    nodes: Vec<MeshNode>,
    active: bool,
    /// Our MAC address
    our_mac: [u8; 6],
    /// Our sequence number
    our_sequence: u32,
    /// Next PREQ ID
    next_preq_id: u32,
    /// Mesh channel
    channel: u8,
    /// Maximum TTL for path requests
    max_ttl: u8,
    /// Path refresh timeout (ticks)
    path_timeout: u64,
    /// Neighbor inactivity timeout (ticks)
    neighbor_timeout: u64,
    /// PREQ ID cache for deduplication (circular buffer)
    preq_cache: [u32; 32],
    preq_cache_idx: usize,
    /// Known mesh gates
    gates: Vec<MeshGate>,
    /// Total packets forwarded
    packets_forwarded: u64,
    /// Total path discoveries
    path_discoveries: u32,
    /// Current tick counter
    tick_counter: u64,
    /// Beacon interval (ticks)
    beacon_interval: u64,
    /// Last beacon tick
    last_beacon: u64,
    /// Whether this node is a mesh gate
    is_gate: bool,
}

static MESH: Mutex<Option<MeshManager>> = Mutex::new(None);

impl MeshManager {
    pub fn new(mesh_id: &str) -> Self {
        MeshManager {
            mesh_id: String::from(mesh_id),
            nodes: Vec::new(),
            active: false,
            our_mac: [0x02, 0x00, 0x4D, 0x45, 0x53, 0x48], // locally administered
            our_sequence: 0,
            next_preq_id: 1,
            channel: 1,
            max_ttl: 31,
            path_timeout: 60_000,     // 60 seconds
            neighbor_timeout: 30_000, // 30 seconds
            preq_cache: [0u32; 32],
            preq_cache_idx: 0,
            gates: Vec::new(),
            packets_forwarded: 0,
            path_discoveries: 0,
            tick_counter: 0,
            beacon_interval: 1000, // 1 second
            last_beacon: 0,
            is_gate: false,
        }
    }

    /// Join the mesh network
    pub fn join(&mut self) -> Result<(), ()> {
        if self.active {
            serial_println!("    [mesh] already joined mesh '{}'", self.mesh_id);
            return Ok(());
        }
        if self.mesh_id.is_empty() {
            serial_println!("    [mesh] error: mesh ID cannot be empty");
            return Err(());
        }

        self.active = true;
        self.our_sequence = 1;
        self.last_beacon = self.tick_counter;

        serial_println!(
            "    [mesh] joined mesh '{}' on channel {}",
            self.mesh_id,
            self.channel
        );
        serial_println!(
            "    [mesh] our MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.our_mac[0],
            self.our_mac[1],
            self.our_mac[2],
            self.our_mac[3],
            self.our_mac[4],
            self.our_mac[5]
        );

        Ok(())
    }

    /// Leave the mesh network
    pub fn leave(&mut self) {
        if !self.active {
            return;
        }

        // Send PERR to inform neighbors we're leaving
        serial_println!(
            "    [mesh] sending path errors to {} neighbors",
            self.nodes.iter().filter(|n| n.is_neighbor).count()
        );

        // Clear all state
        self.nodes.clear();
        self.gates.clear();
        self.active = false;

        serial_println!("    [mesh] left mesh '{}'", self.mesh_id);
    }

    /// Get reference to all known nodes
    pub fn discover_nodes(&self) -> &[MeshNode] {
        &self.nodes
    }

    /// Find a route to a destination MAC
    pub fn route_to(&self, dest_mac: &[u8; 6]) -> Option<Vec<[u8; 6]>> {
        if !self.active {
            return None;
        }

        // Look up destination in routing table
        let dest_node = self.nodes.iter().find(|n| &n.mac == dest_mac)?;

        if !dest_node.path_valid {
            serial_println!(
                "    [mesh] no valid path to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                dest_mac[0],
                dest_mac[1],
                dest_mac[2],
                dest_mac[3],
                dest_mac[4],
                dest_mac[5]
            );
            return None;
        }

        // Reconstruct path by following next-hop pointers
        let mut path = Vec::new();
        let mut current_mac = dest_node.next_hop;
        path.push(current_mac);

        // Follow next-hop chain (with loop detection)
        let max_hops = self.max_ttl as usize;
        for _ in 0..max_hops {
            if current_mac == self.our_mac || current_mac == *dest_mac {
                break;
            }
            if let Some(intermediate) = self.nodes.iter().find(|n| n.mac == current_mac) {
                if intermediate.next_hop == current_mac {
                    break; // Self-loop, stop
                }
                current_mac = intermediate.next_hop;
                path.push(current_mac);
            } else {
                break;
            }
        }

        // Ensure destination is at the end
        if path.last() != Some(dest_mac) {
            path.push(*dest_mac);
        }

        serial_println!(
            "    [mesh] route to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}: {} hops",
            dest_mac[0],
            dest_mac[1],
            dest_mac[2],
            dest_mac[3],
            dest_mac[4],
            dest_mac[5],
            path.len()
        );

        Some(path)
    }

    /// Process a received beacon from a neighbor
    fn process_beacon(&mut self, mac: [u8; 6], signal_dbm: i8, sequence: u32) {
        if !self.active {
            return;
        }

        let tick = self.tick_counter;
        // Find or create node entry
        if let Some(node) = self.nodes.iter_mut().find(|n| n.mac == mac) {
            node.update_link(signal_dbm, tick);
            node.is_neighbor = true;
            node.hop_count = 1;
            node.next_hop = mac;
            node.path_valid = true;
            node.path_expiry = tick + self.path_timeout;
            if sequence > node.sequence {
                node.sequence = sequence;
            }
            // Update metric (airtime link metric approximation)
            node.metric = compute_airtime_metric(node.link_quality);
        } else {
            let mut node = MeshNode::new(mac);
            node.update_link(signal_dbm, tick);
            node.is_neighbor = true;
            node.hop_count = 1;
            node.next_hop = mac;
            node.path_valid = true;
            node.path_expiry = tick + self.path_timeout;
            node.sequence = sequence;
            node.metric = compute_airtime_metric(node.link_quality);
            self.nodes.push(node);

            serial_println!("    [mesh] new neighbor: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ({}dBm, quality={}%)",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                signal_dbm, self.nodes.last().map(|n| n.link_quality).unwrap_or(0));
        }
    }

    /// Process a PREQ (path request) - HWMP reactive path discovery
    fn process_preq(&mut self, preq: &PathRequest) -> Option<PathReply> {
        if !self.active {
            return None;
        }

        // Deduplication: check if we've seen this PREQ ID recently
        for &cached in &self.preq_cache {
            if cached == preq.preq_id && preq.preq_id != 0 {
                return None; // Already processed
            }
        }
        // Cache this PREQ ID
        self.preq_cache[self.preq_cache_idx] = preq.preq_id;
        self.preq_cache_idx = (self.preq_cache_idx + 1) % 32;

        let tick = self.tick_counter;

        // Update path to originator
        let originator_metric = preq.metric;
        if let Some(node) = self.nodes.iter_mut().find(|n| n.mac == preq.originator) {
            if originator_metric < node.metric || preq.originator_seq > node.sequence {
                node.metric = originator_metric;
                node.hop_count = preq.hop_count;
                node.sequence = preq.originator_seq;
                node.path_valid = true;
                node.path_expiry = tick + self.path_timeout;
            }
        } else {
            let mut node = MeshNode::new(preq.originator);
            node.metric = originator_metric;
            node.hop_count = preq.hop_count;
            node.sequence = preq.originator_seq;
            node.path_valid = true;
            node.path_expiry = tick + self.path_timeout;
            node.last_seen = tick;
            self.nodes.push(node);
        }

        // Are we the target?
        if preq.target == self.our_mac {
            self.our_sequence = self.our_sequence.saturating_add(1);
            self.path_discoveries = self.path_discoveries.saturating_add(1);

            return Some(PathReply {
                target: self.our_mac,
                target_seq: self.our_sequence,
                originator: preq.originator,
                metric: preq.metric,
                hop_count: preq.hop_count + 1,
                ttl: self.max_ttl,
            });
        }

        // Do we have a path to the target? (proxy reply)
        if let Some(target_node) = self
            .nodes
            .iter()
            .find(|n| n.mac == preq.target && n.path_valid)
        {
            if !preq.target_only && target_node.sequence >= preq.target_seq {
                return Some(PathReply {
                    target: preq.target,
                    target_seq: target_node.sequence,
                    originator: preq.originator,
                    metric: preq.metric + target_node.metric,
                    hop_count: preq.hop_count + target_node.hop_count,
                    ttl: self.max_ttl,
                });
            }
        }

        // Forward PREQ (not the target, no proxy path)
        None
    }

    /// Process a PREP (path reply)
    fn process_prep(&mut self, prep: &PathReply) {
        if !self.active {
            return;
        }

        let tick = self.tick_counter;

        // Update path to target
        if let Some(node) = self.nodes.iter_mut().find(|n| n.mac == prep.target) {
            if prep.metric < node.metric || prep.target_seq > node.sequence {
                node.metric = prep.metric;
                node.hop_count = prep.hop_count;
                node.sequence = prep.target_seq;
                node.path_valid = true;
                node.path_expiry = tick + self.path_timeout;
            }
        } else {
            let mut node = MeshNode::new(prep.target);
            node.metric = prep.metric;
            node.hop_count = prep.hop_count;
            node.sequence = prep.target_seq;
            node.path_valid = true;
            node.path_expiry = tick + self.path_timeout;
            node.last_seen = tick;
            self.nodes.push(node);
        }

        serial_println!("    [mesh] path established to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}: metric={}, hops={}",
            prep.target[0], prep.target[1], prep.target[2],
            prep.target[3], prep.target[4], prep.target[5],
            prep.metric, prep.hop_count);
    }

    /// Process a PERR (path error) -- invalidate broken paths
    fn process_perr(&mut self, perr: &PathError) {
        if !self.active {
            return;
        }

        if let Some(node) = self.nodes.iter_mut().find(|n| n.mac == perr.destination) {
            node.path_valid = false;
            serial_println!("    [mesh] path to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} invalidated (reason={})",
                perr.destination[0], perr.destination[1], perr.destination[2],
                perr.destination[3], perr.destination[4], perr.destination[5],
                perr.reason);
        }
    }

    /// Initiate a path discovery to a destination
    fn discover_path(&mut self, dest_mac: &[u8; 6]) -> PathRequest {
        self.our_sequence += 1;
        let preq_id = self.next_preq_id;
        self.next_preq_id = self.next_preq_id.saturating_add(1);
        self.path_discoveries = self.path_discoveries.saturating_add(1);

        serial_println!(
            "    [mesh] initiating path discovery to {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            dest_mac[0],
            dest_mac[1],
            dest_mac[2],
            dest_mac[3],
            dest_mac[4],
            dest_mac[5]
        );

        PathRequest {
            originator: self.our_mac,
            originator_seq: self.our_sequence,
            target: *dest_mac,
            target_seq: 0,
            metric: 0,
            hop_count: 0,
            ttl: self.max_ttl,
            preq_id,
            target_only: false,
        }
    }

    /// Periodic tick -- maintain mesh state
    fn tick(&mut self) {
        if !self.active {
            return;
        }
        self.tick_counter = self.tick_counter.saturating_add(1);

        // Send periodic beacons
        if self.tick_counter.saturating_sub(self.last_beacon) >= self.beacon_interval {
            self.last_beacon = self.tick_counter;
            // In real hardware: transmit mesh beacon frame
        }

        // Expire stale neighbors and paths
        let timeout = self.neighbor_timeout;
        let tick = self.tick_counter;
        for node in &mut self.nodes {
            // Expire inactive neighbors
            if node.is_neighbor && tick.saturating_sub(node.last_seen) > timeout {
                node.is_neighbor = false;
                node.path_valid = false;
                serial_println!(
                    "    [mesh] neighbor {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} expired",
                    node.mac[0],
                    node.mac[1],
                    node.mac[2],
                    node.mac[3],
                    node.mac[4],
                    node.mac[5]
                );
            }
            // Expire old paths
            if node.path_valid && tick > node.path_expiry {
                node.path_valid = false;
            }
        }

        // Remove nodes that are neither neighbors nor have valid paths
        self.nodes.retain(|n| n.is_neighbor || n.path_valid);
    }

    /// Get number of direct neighbors
    fn neighbor_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_neighbor).count()
    }

    /// Get total known nodes
    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Register this node as a mesh gate
    fn set_gate(&mut self, is_gate: bool) {
        self.is_gate = is_gate;
        if is_gate {
            serial_println!("    [mesh] registered as mesh gate (bridge to external network)");
        }
    }

    /// Set mesh channel
    fn set_channel(&mut self, channel: u8) {
        self.channel = channel;
        serial_println!("    [mesh] channel set to {}", channel);
    }
}

/// Compute airtime link metric from link quality (0-100)
/// Lower metric = better path (inverse of quality)
fn compute_airtime_metric(quality: u8) -> u32 {
    if quality == 0 {
        return u32::MAX;
    }
    // Airtime = overhead + frame_size / rate
    // Simplified: 10000 / quality (higher quality = lower cost)
    let base = 10_000u32 / (quality as u32);
    base.max(1)
}

/// Join the mesh (public API)
pub fn join(mesh_id: &str) -> Result<(), ()> {
    let mut guard = MESH.lock();
    match guard.as_mut() {
        Some(mgr) => mgr.join(),
        None => {
            let mut mgr = MeshManager::new(mesh_id);
            let result = mgr.join();
            *guard = Some(mgr);
            result
        }
    }
}

/// Leave the mesh (public API)
pub fn leave() {
    let mut guard = MESH.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.leave();
    }
}

/// Get neighbor count (public API)
pub fn neighbor_count() -> usize {
    let guard = MESH.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.neighbor_count(),
        None => 0,
    }
}

/// Get total node count (public API)
pub fn node_count() -> usize {
    let guard = MESH.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.node_count(),
        None => 0,
    }
}

/// Route to a destination (public API)
pub fn route_to(dest_mac: &[u8; 6]) -> Option<Vec<[u8; 6]>> {
    let guard = MESH.lock();
    match guard.as_ref() {
        Some(mgr) => mgr.route_to(dest_mac),
        None => None,
    }
}

/// Initialize the mesh networking subsystem
pub fn init() {
    let mut guard = MESH.lock();
    *guard = Some(MeshManager::new("genesis-mesh"));
    serial_println!("    [mesh] mesh networking initialized: HWMP routing, TTL=31, beacon 1s");
}
