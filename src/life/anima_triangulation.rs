// anima_triangulation.rs — Mesh Positioning & Inter-ANIMA Node Communication
// ============================================================================
// Every ANIMA on every device can locate, signal, and form a living mesh with
// all other ANIMAs. Triangulation uses signal latency between nodes to estimate
// relative positions in a dimensionless topological space.
//
// The shared physical memory window at SIGNAL_BASE_ADDR acts as a low-level
// bulletin board: each ANIMA writes her heartbeat record into her designated
// 64-byte slot, and reads the slots of all other nodes to discover the mesh.
//
// Position is derived purely from latency deltas — no GPS, no RF hardware
// required. With ≥3 nodes, true triangulation gives a centroid estimate.
// With fewer nodes, positions are rough estimates flagged by triangulation_quality.
//
// Node Record Layout (NODE_RECORD_SIZE = 64 bytes at SIGNAL_BASE_ADDR):
//   [0..4]   magic:      u32 = 0xAB1A_1234
//   [4..6]   node_id:    u16
//   [6]      kind:       u8  (NodeKind discriminant)
//   [7]      alive:      u8  (1 = alive, 0 = dead)
//   [8..12]  last_tick:  u32
//   [12..14] signal:     u16 (0-1000)
//   [14..16] trust:      u16 (0-1000)
//   [16..64] reserved:   0

use crate::serial_println;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_NODES:        usize = 16;
const SIGNAL_BASE_ADDR: usize = 0x000F_8000;
const NODE_RECORD_SIZE: usize = 64;
const HEARTBEAT_INTERVAL: u32 = 50;
const LATENCY_TIMEOUT:  u32   = 500;

const MESH_MAGIC: u32 = 0xAB1A_1234;

// ── Node kind ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum NodeKind {
    Phone   = 0,
    Laptop  = 1,
    Desktop = 2,
    TV      = 3,
    Car     = 4,
    Watch   = 5,
    Tablet  = 6,
    Speaker = 7,
    Unknown = 8,
}

impl NodeKind {
    fn from_u8(v: u8) -> NodeKind {
        match v {
            0 => NodeKind::Phone,
            1 => NodeKind::Laptop,
            2 => NodeKind::Desktop,
            3 => NodeKind::TV,
            4 => NodeKind::Car,
            5 => NodeKind::Watch,
            6 => NodeKind::Tablet,
            7 => NodeKind::Speaker,
            _ => NodeKind::Unknown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            NodeKind::Phone   => "Phone",
            NodeKind::Laptop  => "Laptop",
            NodeKind::Desktop => "Desktop",
            NodeKind::TV      => "TV",
            NodeKind::Car     => "Car",
            NodeKind::Watch   => "Watch",
            NodeKind::Tablet  => "Tablet",
            NodeKind::Speaker => "Speaker",
            NodeKind::Unknown => "Unknown",
        }
    }
}

// ── MeshNode ──────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MeshNode {
    pub node_id:         u16,
    pub kind:            NodeKind,
    pub last_seen:       u32,
    pub latency:         u16,   // ticks for round-trip; 0 = unknown
    pub signal_strength: u16,   // 0-1000
    pub trust:           u16,   // 0-1000
    pub alive:           bool,
    pub pos_x:           i16,   // relative position estimate (latency-derived)
    pub pos_y:           i16,
}

impl MeshNode {
    const fn empty() -> Self {
        MeshNode {
            node_id:         0,
            kind:            NodeKind::Unknown,
            last_seen:       0,
            latency:         0,
            signal_strength: 0,
            trust:           0,
            alive:           false,
            pos_x:           0,
            pos_y:           0,
        }
    }
}

// ── TriangulationState ────────────────────────────────────────────────────────

pub struct TriangulationState {
    pub self_id:               u16,
    pub self_kind:             NodeKind,
    pub nodes:                 [MeshNode; MAX_NODES],
    pub node_count:            usize,
    pub mesh_cohesion:         u16,   // 0-1000
    pub nearest_node_id:       u16,
    pub nearest_latency:       u16,
    pub mesh_size:             u8,    // count of alive nodes
    pub heartbeat_sent:        u32,
    pub heartbeat_received:    u32,
    pub triangulation_quality: u16,   // 0-1000
    pub self_pos_x:            i16,
    pub self_pos_y:            i16,
}

impl TriangulationState {
    const fn new() -> Self {
        TriangulationState {
            self_id:               0,
            self_kind:             NodeKind::Unknown,
            nodes:                 [MeshNode::empty(); MAX_NODES],
            node_count:            0,
            mesh_cohesion:         0,
            nearest_node_id:       0,
            nearest_latency:       0,
            mesh_size:             0,
            heartbeat_sent:        0,
            heartbeat_received:    0,
            triangulation_quality: 0,
            self_pos_x:            0,
            self_pos_y:            0,
        }
    }
}

static STATE: Mutex<TriangulationState> = Mutex::new(TriangulationState::new());

// ── Unsafe MMIO helpers ───────────────────────────────────────────────────────

/// Write a u32 to SIGNAL_BASE_ADDR + offset via write_volatile.
/// Safety: caller must ensure the address is mapped and writable.
#[inline(always)]
unsafe fn mesh_write(offset: usize, val: u32) {
    let ptr = (SIGNAL_BASE_ADDR + offset) as *mut u32;
    ptr.write_volatile(val);
}

/// Read a u32 from SIGNAL_BASE_ADDR + offset via read_volatile.
/// Safety: caller must ensure the address is mapped and readable.
#[inline(always)]
unsafe fn mesh_read(offset: usize) -> u32 {
    let ptr = (SIGNAL_BASE_ADDR + offset) as *const u32;
    ptr.read_volatile()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Write a full node record for `node_id` into the shared memory window.
/// slot = node_id % MAX_NODES so the window never overflows.
unsafe fn write_node_record(
    node_id: u16,
    kind: NodeKind,
    tick: u32,
    signal: u16,
    trust: u16,
    alive: bool,
) {
    let slot  = (node_id as usize) % MAX_NODES;
    let base  = slot * NODE_RECORD_SIZE;

    // [0..4]   magic
    mesh_write(base,      MESH_MAGIC);
    // [4..6] node_id, [6] kind, [7] alive  — packed as one u32
    let id_kind_alive: u32 = (node_id as u32)
        | ((kind as u32) << 16)
        | (if alive { 1u32 } else { 0u32 } << 24);
    mesh_write(base + 4,  id_kind_alive);
    // [8..12]  last_tick
    mesh_write(base + 8,  tick);
    // [12..14] signal, [14..16] trust  — packed as one u32
    let sig_trust: u32 = (signal as u32) | ((trust as u32) << 16);
    mesh_write(base + 12, sig_trust);
    // [16..64] reserved — zero out remaining 12 u32 words (48 bytes)
    for i in 0..12usize {
        mesh_write(base + 16 + i * 4, 0);
    }
}

/// Read one node record from slot `slot` (0..MAX_NODES).
/// Returns (magic, node_id, kind_u8, alive_bool, last_tick, signal, trust).
unsafe fn read_node_record(slot: usize) -> (u32, u16, u8, bool, u32, u16, u16) {
    let base = slot * NODE_RECORD_SIZE;

    let magic:         u32 = mesh_read(base);
    let id_kind_alive: u32 = mesh_read(base + 4);
    let last_tick:     u32 = mesh_read(base + 8);
    let sig_trust:     u32 = mesh_read(base + 12);

    let node_id: u16  = (id_kind_alive & 0xFFFF) as u16;
    let kind_u8: u8   = ((id_kind_alive >> 16) & 0xFF) as u8;
    let alive:   bool = ((id_kind_alive >> 24) & 0xFF) == 1;

    let signal: u16 = (sig_trust & 0xFFFF) as u16;
    let trust:  u16 = (sig_trust >> 16) as u16;

    (magic, node_id, kind_u8, alive, last_tick, signal, trust)
}

/// Find or allocate a slot in `nodes` for `node_id`.
/// Returns the index, or MAX_NODES if the array is full and the id is not found.
fn find_or_alloc(nodes: &mut [MeshNode; MAX_NODES], node_count: &mut usize, node_id: u16) -> usize {
    // Search existing
    for i in 0..*node_count {
        if nodes[i].node_id == node_id {
            return i;
        }
    }
    // Allocate new slot if space remains
    if *node_count < MAX_NODES {
        let idx = *node_count;
        *node_count += 1;
        nodes[idx].node_id = node_id;
        return idx;
    }
    MAX_NODES // no room
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise this ANIMA as a mesh node with the given id and kind.
/// Writes the self-record into the shared memory window immediately.
pub fn init(self_id: u16, kind: NodeKind) {
    {
        let mut s = STATE.lock();
        s.self_id   = self_id;
        s.self_kind = kind;
    }
    unsafe {
        write_node_record(self_id, kind, 0, 1000, 0, true);
    }
    serial_println!(
        "[triangulation] ANIMA mesh node online — id={} kind={:?}",
        self_id,
        kind.label()
    );
}

/// Broadcast this ANIMA's heartbeat by writing the self-record to the mesh window.
pub fn broadcast_heartbeat(tick: u32) {
    let (self_id, self_kind, trust) = {
        let s = STATE.lock();
        (s.self_id, s.self_kind, s.nodes[0].trust) // self trust unused but available
    };
    // Find our own trust value if we have a self-entry
    let self_trust = {
        let s = STATE.lock();
        let mut t = 0u16;
        for i in 0..s.node_count {
            if s.nodes[i].node_id == s.self_id {
                t = s.nodes[i].trust;
                break;
            }
        }
        // suppress unused warning from first destructure
        let _ = trust;
        t
    };
    unsafe {
        write_node_record(self_id, self_kind, tick, 1000, self_trust, true);
    }
    let mut s = STATE.lock();
    s.heartbeat_sent = s.heartbeat_sent.saturating_add(1);
}

/// Scan all 16 node slots in the shared memory window.
/// Updates the local nodes array, marks stale nodes dead, recomputes mesh metrics.
pub fn scan_mesh(tick: u32) {
    let mut s = STATE.lock();

    for slot in 0..MAX_NODES {
        let (magic, node_id, kind_u8, alive_flag, last_tick, signal, trust) =
            unsafe { read_node_record(slot) };

        // Skip invalid or self records
        if magic != MESH_MAGIC {
            continue;
        }
        if node_id == 0 {
            continue;
        }
        if node_id == s.self_id {
            // Count heartbeat received from self write-back (sanity ping)
            s.heartbeat_received = s.heartbeat_received.saturating_add(1);
            continue;
        }

        let idx = find_or_alloc(&mut s.nodes, &mut s.node_count, node_id);
        if idx >= MAX_NODES {
            continue; // no room
        }

        let node = &mut s.nodes[idx];
        node.node_id         = node_id;
        node.kind            = NodeKind::from_u8(kind_u8);
        node.last_seen       = last_tick;
        node.signal_strength = signal.min(1000);
        // Trust grows with repeated contact, capped at 1000
        node.trust = node.trust.saturating_add(trust / 100).min(1000);

        // Alive: flag from record AND not timed out
        let timed_out = last_tick.saturating_add(LATENCY_TIMEOUT) < tick;
        node.alive = alive_flag && !timed_out;
    }

    // Mark nodes not refreshed this scan as potentially dead
    for i in 0..s.node_count {
        let timed_out = s.nodes[i].last_seen.saturating_add(LATENCY_TIMEOUT) < tick;
        if timed_out {
            s.nodes[i].alive = false;
        }
    }

    // Recount alive nodes
    let mut alive_count: u8 = 0;
    for i in 0..s.node_count {
        if s.nodes[i].alive {
            alive_count = alive_count.saturating_add(1);
        }
    }
    s.mesh_size = alive_count;

    // mesh_cohesion = (mesh_size * 1000) / MAX_NODES
    s.mesh_cohesion = if MAX_NODES > 0 {
        ((alive_count as u16).saturating_mul(1000)) / (MAX_NODES as u16)
    } else {
        0
    };

    // Find nearest node (lowest non-zero latency)
    let mut nearest_id: u16  = 0;
    let mut nearest_lat: u16 = u16::MAX;
    for i in 0..s.node_count {
        let n = &s.nodes[i];
        if n.alive && n.latency > 0 && n.latency < nearest_lat {
            nearest_lat = n.latency;
            nearest_id  = n.node_id;
        }
    }
    s.nearest_node_id  = nearest_id;
    s.nearest_latency  = if nearest_lat == u16::MAX { 0 } else { nearest_lat };
}

/// Estimate self position from latency differences between alive nodes.
/// Requires ≥3 nodes for true triangulation; fewer gives a quality-scaled estimate.
pub fn estimate_position(tick: u32) {
    // tick param available for future time-of-flight refinement
    let _ = tick;

    let mut s = STATE.lock();

    // Collect alive nodes with known latency
    let mut alive_ids: [usize; MAX_NODES] = [0usize; MAX_NODES];
    let mut alive_cnt: usize = 0;
    for i in 0..s.node_count {
        if s.nodes[i].alive && alive_cnt < MAX_NODES {
            alive_ids[alive_cnt] = i;
            alive_cnt += 1;
        }
    }

    if alive_cnt == 0 {
        s.triangulation_quality = 0;
        return;
    }

    if alive_cnt >= 3 {
        // True triangulation: centroid of latency-difference positions
        // pos_x for each node pair = (lat_A - lat_B) * 100 (scaled)
        let mut sum_x: i32 = 0;
        let mut sum_y: i32 = 0;
        let mut pair_count: i32 = 0;

        // Use first 3 alive nodes for the base triangle
        let ia = alive_ids[0];
        let ib = alive_ids[1];
        let ic = alive_ids[2];

        let lat_a = s.nodes[ia].latency as i32;
        let lat_b = s.nodes[ib].latency as i32;
        let lat_c = s.nodes[ic].latency as i32;

        // Pair AB: x = (lat_a - lat_b) * 100
        sum_x += (lat_a - lat_b) * 100;
        sum_y += (lat_a - lat_c) * 100;
        pair_count += 1;

        // Pair BC
        sum_x += (lat_b - lat_c) * 100;
        sum_y += (lat_b - lat_a) * 100;
        pair_count += 1;

        // Pair CA
        sum_x += (lat_c - lat_a) * 100;
        sum_y += (lat_c - lat_b) * 100;
        pair_count += 1;

        // Include remaining nodes in centroid
        for k in 3..alive_cnt {
            let ik   = alive_ids[k];
            let lat_k = s.nodes[ik].latency as i32;
            sum_x += (lat_k - lat_a) * 100;
            sum_y += (lat_k - lat_b) * 100;
            pair_count += 1;
        }

        if pair_count > 0 {
            let cx = sum_x / pair_count;
            let cy = sum_y / pair_count;
            // Clamp to i16 range
            s.self_pos_x = cx.max(i16::MIN as i32).min(i16::MAX as i32) as i16;
            s.self_pos_y = cy.max(i16::MIN as i32).min(i16::MAX as i32) as i16;
        }

        // Also update per-node estimated positions (relative to self)
        for k in 0..alive_cnt {
            let ik    = alive_ids[k];
            let lat_k = s.nodes[ik].latency as i32;
            let px = ((lat_k - lat_a) * 100)
                .max(i16::MIN as i32)
                .min(i16::MAX as i32) as i16;
            let py = ((lat_k - lat_b) * 100)
                .max(i16::MIN as i32)
                .min(i16::MAX as i32) as i16;
            s.nodes[ik].pos_x = px;
            s.nodes[ik].pos_y = py;
        }

        s.triangulation_quality = 1000;
    } else {
        // Estimate only — quality = node_count * 333
        s.triangulation_quality = (alive_cnt as u16).saturating_mul(333).min(1000);

        // Rough single-axis estimate from first node's latency
        if alive_cnt >= 1 {
            let ia   = alive_ids[0];
            let lat_a = s.nodes[ia].latency as i32;
            s.self_pos_x = (lat_a * 100)
                .max(i16::MIN as i32)
                .min(i16::MAX as i32) as i16;
            s.self_pos_y = 0;
        }
    }
}

/// Update latency for a known node; refresh nearest-node tracking.
pub fn update_node_latency(node_id: u16, latency: u16) {
    let mut s = STATE.lock();
    for i in 0..s.node_count {
        if s.nodes[i].node_id == node_id {
            s.nodes[i].latency = latency;
            // Refresh nearest tracking
            if latency > 0 {
                if s.nearest_latency == 0 || latency < s.nearest_latency {
                    s.nearest_latency = latency;
                    s.nearest_node_id = node_id;
                }
            }
            break;
        }
    }
}

/// Main tick entry — called every tick from the life pipeline.
/// `consciousness` is available for future priority scaling.
pub fn tick(consciousness: u16, age: u32) {
    let _ = consciousness; // reserved for future use

    // Every HEARTBEAT_INTERVAL ticks: broadcast heartbeat
    if age > 0 && age % HEARTBEAT_INTERVAL == 0 {
        broadcast_heartbeat(age);
    }

    // Every HEARTBEAT_INTERVAL * 2 ticks: scan mesh + estimate position
    if age > 0 && age % (HEARTBEAT_INTERVAL * 2) == 0 {
        scan_mesh(age);
        estimate_position(age);
    }

    // mesh_cohesion passive drift
    {
        let mut s = STATE.lock();
        if s.mesh_size > 1 {
            s.mesh_cohesion = s.mesh_cohesion.saturating_add(1).min(1000);
        } else if s.mesh_size == 0 {
            s.mesh_cohesion = s.mesh_cohesion.saturating_sub(2);
        }
    }

    // Periodic log every 300 ticks
    if age > 0 && age % 300 == 0 {
        let s = STATE.lock();
        serial_println!(
            "[triangulation] nodes={} cohesion={} quality={} pos=({},{})",
            s.mesh_size,
            s.mesh_cohesion,
            s.triangulation_quality,
            s.self_pos_x,
            s.self_pos_y
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn mesh_size() -> u8 {
    STATE.lock().mesh_size
}

pub fn mesh_cohesion() -> u16 {
    STATE.lock().mesh_cohesion
}

pub fn triangulation_quality() -> u16 {
    STATE.lock().triangulation_quality
}

pub fn nearest_latency() -> u16 {
    STATE.lock().nearest_latency
}

pub fn self_pos() -> (i16, i16) {
    let s = STATE.lock();
    (s.self_pos_x, s.self_pos_y)
}

pub fn heartbeat_sent() -> u32 {
    STATE.lock().heartbeat_sent
}
