/// Relay / NAT Traversal for Genesis
///
/// Provides relay sessions for peers behind NATs, hole-punching,
/// STUN-style address discovery, and TURN-style relay allocation.
/// All timing and rate values use i32 Q16 fixed-point where needed.
///
/// All code is original. No external crates.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

pub type Q16 = i32;

const Q16_ONE: Q16 = 1 << 16;
const Q16_ZERO: Q16 = 0;

/// Multiply two Q16 values.
fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum concurrent relay sessions
const MAX_RELAY_SESSIONS: usize = 128;

/// Maximum STUN transaction tracking
const MAX_STUN_TRANSACTIONS: usize = 64;

/// Maximum TURN allocations
const MAX_TURN_ALLOCATIONS: usize = 32;

/// Default relay session timeout (abstract ticks)
const RELAY_TIMEOUT: u64 = 7200;

/// Hole-punch retry limit
const HOLE_PUNCH_RETRIES: u8 = 5;

/// STUN magic cookie (RFC 5389)
const STUN_MAGIC_COOKIE: u32 = 0x2112A442;

/// STUN binding request type
const STUN_BINDING_REQUEST: u16 = 0x0001;

/// STUN binding response type
const STUN_BINDING_RESPONSE: u16 = 0x0101;

/// TURN allocate request type
const TURN_ALLOCATE_REQUEST: u16 = 0x0003;

/// TURN allocate response type
const TURN_ALLOCATE_RESPONSE: u16 = 0x0103;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A relay session bridging two peers.
#[derive(Clone)]
pub struct RelaySession {
    pub id: u64,
    pub client_a: u64,
    pub client_b: u64,
    pub established: bool,
    pub bytes_relayed: u64,
    pub created: u64,
}

/// Detected NAT type for a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// No NAT — directly reachable
    Open,
    /// Full cone NAT — any external host can send through mapped port
    FullCone,
    /// Restricted cone — only hosts we've sent to can reply
    RestrictedCone,
    /// Port-restricted cone — host + port must match
    PortRestricted,
    /// Symmetric NAT — different mapping per destination
    Symmetric,
    /// Not yet determined
    Unknown,
}

/// Result of a STUN binding request.
#[derive(Clone)]
pub struct StunResult {
    pub success: bool,
    pub mapped_addr_hash: u64,
    pub mapped_port: u16,
    pub nat_type: NatType,
    pub transaction_id: u64,
}

/// A TURN allocation.
#[derive(Clone)]
pub struct TurnAllocation {
    pub allocation_id: u64,
    pub client_id: u64,
    pub relay_addr_hash: u64,
    pub relay_port: u16,
    pub lifetime: u32,
    pub created: u64,
    pub bytes_relayed: u64,
}

/// Hole-punch attempt state.
#[derive(Clone)]
struct HolePunchAttempt {
    peer_a: u64,
    peer_b: u64,
    retries_left: u8,
    addr_a_hash: u64,
    port_a: u16,
    addr_b_hash: u64,
    port_b: u16,
    started: u64,
    success: bool,
}

/// Statistics for the relay subsystem.
pub struct RelayStats {
    pub active_sessions: usize,
    pub total_bytes_relayed: u64,
    pub active_allocations: usize,
    pub hole_punch_attempts: usize,
    pub hole_punch_successes: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static RELAY_SESSIONS: Mutex<Option<Vec<RelaySession>>> = Mutex::new(None);
static TURN_ALLOCS: Mutex<Option<Vec<TurnAllocation>>> = Mutex::new(None);
static HOLE_PUNCHES: Mutex<Option<Vec<HolePunchAttempt>>> = Mutex::new(None);
static LOCAL_NAT_TYPE: Mutex<NatType> = Mutex::new(NatType::Unknown);
static LOCAL_PUBLIC_ADDR: Mutex<Option<(u64, u16)>> = Mutex::new(None);
static RELAY_ACTIVE: Mutex<bool> = Mutex::new(false);
static PUNCH_SUCCESS_COUNT: Mutex<usize> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    {
        let mut sessions = RELAY_SESSIONS.lock();
        *sessions = Some(Vec::new());
    }
    {
        let mut allocs = TURN_ALLOCS.lock();
        *allocs = Some(Vec::new());
    }
    {
        let mut punches = HOLE_PUNCHES.lock();
        *punches = Some(Vec::new());
    }
    {
        let mut active = RELAY_ACTIVE.lock();
        *active = true;
    }
    serial_println!("    relay: initialized (STUN/TURN/hole-punch)");
}

// ---------------------------------------------------------------------------
// NAT detection
// ---------------------------------------------------------------------------

/// Detect the local NAT type using a STUN-like procedure.
///
/// The detection works by simulating two binding requests to different
/// server addresses and comparing the mapped (public) address/port.
///
/// Returns the detected NAT type.
pub fn detect_nat(
    stun_server_a: u64,
    stun_server_b: u64,
    local_port: u16,
    timestamp: u64,
) -> NatType {
    // Simulate first STUN request
    let result_a = stun_request(stun_server_a, local_port, timestamp);
    if !result_a.success {
        let mut nat = LOCAL_NAT_TYPE.lock();
        *nat = NatType::Unknown;
        return NatType::Unknown;
    }

    // Simulate second STUN request to a different server
    let result_b = stun_request(stun_server_b, local_port, timestamp + 1);
    if !result_b.success {
        let mut nat = LOCAL_NAT_TYPE.lock();
        *nat = NatType::Unknown;
        return NatType::Unknown;
    }

    let detected = if result_a.mapped_addr_hash == 0 && result_a.mapped_port == local_port {
        // No translation observed
        NatType::Open
    } else if result_a.mapped_addr_hash == result_b.mapped_addr_hash
        && result_a.mapped_port == result_b.mapped_port
    {
        // Same mapped address for different destinations — cone NAT
        // Further classification requires testing with changed source port
        // For now, distinguish by port consistency
        if result_a.mapped_port == local_port {
            NatType::FullCone
        } else {
            NatType::RestrictedCone
        }
    } else if result_a.mapped_addr_hash == result_b.mapped_addr_hash {
        // Same IP but different port — port-restricted
        NatType::PortRestricted
    } else {
        // Different mapped IP — symmetric NAT
        NatType::Symmetric
    };

    // Store result
    {
        let mut nat = LOCAL_NAT_TYPE.lock();
        *nat = detected;
    }
    {
        let mut addr = LOCAL_PUBLIC_ADDR.lock();
        *addr = Some((result_a.mapped_addr_hash, result_a.mapped_port));
    }

    detected
}

/// Get the currently detected NAT type.
pub fn get_nat_type() -> NatType {
    let nat = LOCAL_NAT_TYPE.lock();
    *nat
}

// ---------------------------------------------------------------------------
// STUN
// ---------------------------------------------------------------------------

/// Perform a STUN binding request to discover our public address.
///
/// In a real implementation this would send a UDP packet to the STUN server
/// and parse the response. Here we simulate the process with deterministic
/// hashing of the server address and local port.
pub fn stun_request(server_addr_hash: u64, local_port: u16, timestamp: u64) -> StunResult {
    let _cookie = STUN_MAGIC_COOKIE;
    let _req_type = STUN_BINDING_REQUEST;

    // Generate a transaction ID from inputs
    let transaction_id = server_addr_hash ^ ((local_port as u64) << 48) ^ timestamp;

    // Simulate server response
    // The "mapped address" is derived from server + local port as a simulation
    let mapped_addr_hash = server_addr_hash
        .wrapping_mul(0x0000_DEAD_0000_BEEF)
        .wrapping_add(local_port as u64);
    let mapped_port =
        ((local_port as u32).wrapping_add((server_addr_hash & 0xFFFF) as u32) & 0xFFFF) as u16;

    let _resp_type = STUN_BINDING_RESPONSE;

    StunResult {
        success: true,
        mapped_addr_hash,
        mapped_port,
        nat_type: NatType::Unknown, // Caller must interpret
        transaction_id,
    }
}

/// Get our discovered public address (from last STUN result).
pub fn get_public_addr() -> Option<(u64, u16)> {
    let addr = LOCAL_PUBLIC_ADDR.lock();
    *addr
}

// ---------------------------------------------------------------------------
// Hole-punching
// ---------------------------------------------------------------------------

/// Attempt UDP hole-punching between two peers.
///
/// Both peers must simultaneously send packets to each other's
/// public address:port to create NAT pinholes.
///
/// Returns true if the attempt was initiated.
pub fn hole_punch(
    peer_a: u64,
    peer_b: u64,
    addr_a_hash: u64,
    port_a: u16,
    addr_b_hash: u64,
    port_b: u16,
    timestamp: u64,
) -> bool {
    // Hole-punching won't work with symmetric NAT on both sides
    let nat = get_nat_type();
    if nat == NatType::Symmetric {
        // Still try, but success is unlikely
    }

    let attempt = HolePunchAttempt {
        peer_a,
        peer_b,
        retries_left: HOLE_PUNCH_RETRIES,
        addr_a_hash,
        port_a,
        addr_b_hash,
        port_b,
        started: timestamp,
        success: false,
    };

    let mut punches = HOLE_PUNCHES.lock();
    if let Some(ref mut list) = *punches {
        // Don't duplicate
        for existing in list.iter() {
            if (existing.peer_a == peer_a && existing.peer_b == peer_b)
                || (existing.peer_a == peer_b && existing.peer_b == peer_a)
            {
                if !existing.success {
                    return false; // already attempting
                }
            }
        }
        list.push(attempt);
        true
    } else {
        false
    }
}

/// Process one round of hole-punch retries.
/// Returns the number of newly successful punches.
pub fn process_hole_punches(timestamp: u64) -> usize {
    let mut new_successes = 0;

    let mut punches = HOLE_PUNCHES.lock();
    if let Some(ref mut list) = *punches {
        for attempt in list.iter_mut() {
            if attempt.success || attempt.retries_left == 0 {
                continue;
            }

            attempt.retries_left -= 1;

            // Simulate: hole-punch succeeds if both addresses are non-zero
            // and we haven't exhausted retries. Real implementation would
            // check for received packets.
            let elapsed = timestamp.saturating_sub(attempt.started);
            if attempt.addr_a_hash != 0 && attempt.addr_b_hash != 0 && elapsed > 2 {
                attempt.success = true;
                new_successes += 1;
            }
        }

        // Clean up completed/failed attempts older than timeout
        list.retain(|a| {
            let age = timestamp.saturating_sub(a.started);
            a.success || (a.retries_left > 0 && age < RELAY_TIMEOUT)
        });
    }

    if new_successes > 0 {
        let mut count = PUNCH_SUCCESS_COUNT.lock();
        *count += new_successes;
    }

    new_successes
}

/// Check if a hole-punch was successful between two peers.
pub fn is_hole_punched(peer_a: u64, peer_b: u64) -> bool {
    let punches = HOLE_PUNCHES.lock();
    if let Some(ref list) = *punches {
        for attempt in list.iter() {
            if ((attempt.peer_a == peer_a && attempt.peer_b == peer_b)
                || (attempt.peer_a == peer_b && attempt.peer_b == peer_a))
                && attempt.success
            {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Relay sessions
// ---------------------------------------------------------------------------

/// Start a relay session between two clients.
/// Returns the session ID, or 0 on failure.
pub fn start_relay(client_a: u64, client_b: u64, timestamp: u64) -> u64 {
    let active = RELAY_ACTIVE.lock();
    if !*active {
        return 0;
    }
    drop(active);

    let session_id = client_a
        .wrapping_mul(0x0000_0001_0000_0001)
        .wrapping_add(client_b)
        .wrapping_add(timestamp);

    let session = RelaySession {
        id: session_id,
        client_a,
        client_b,
        established: true,
        bytes_relayed: 0,
        created: timestamp,
    };

    let mut sessions = RELAY_SESSIONS.lock();
    if let Some(ref mut list) = *sessions {
        if list.len() >= MAX_RELAY_SESSIONS {
            // Evict the oldest session
            if !list.is_empty() {
                let mut oldest_idx = 0;
                let mut oldest_ts = list[0].created;
                for (i, s) in list.iter().enumerate() {
                    if s.created < oldest_ts {
                        oldest_ts = s.created;
                        oldest_idx = i;
                    }
                }
                list.remove(oldest_idx);
            }
        }
        list.push(session);
        session_id
    } else {
        0
    }
}

/// Relay a packet through an established session.
/// Adds `size` to the session's bytes_relayed counter.
/// Returns true if the relay succeeded.
pub fn relay_packet(session_id: u64, size: u64) -> bool {
    let mut sessions = RELAY_SESSIONS.lock();
    if let Some(ref mut list) = *sessions {
        for session in list.iter_mut() {
            if session.id == session_id && session.established {
                session.bytes_relayed = session.bytes_relayed.saturating_add(size);
                return true;
            }
        }
    }
    false
}

/// Close a relay session by ID.
pub fn close_relay(session_id: u64) -> bool {
    let mut sessions = RELAY_SESSIONS.lock();
    if let Some(ref mut list) = *sessions {
        for session in list.iter_mut() {
            if session.id == session_id {
                session.established = false;
                return true;
            }
        }
    }
    false
}

/// Get a relay session by ID.
pub fn get_session(session_id: u64) -> Option<RelaySession> {
    let sessions = RELAY_SESSIONS.lock();
    if let Some(ref list) = *sessions {
        for session in list.iter() {
            if session.id == session_id {
                return Some(session.clone());
            }
        }
    }
    None
}

/// Expire relay sessions older than the timeout.
pub fn expire_sessions(now: u64) -> usize {
    let mut sessions = RELAY_SESSIONS.lock();
    if let Some(ref mut list) = *sessions {
        let before = list.len();
        list.retain(|s| {
            let age = now.saturating_sub(s.created);
            age < RELAY_TIMEOUT || s.established
        });
        before - list.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// TURN allocation
// ---------------------------------------------------------------------------

/// Request a TURN relay allocation for a client.
/// Returns the allocation ID, or 0 on failure.
pub fn turn_allocate(client_id: u64, lifetime: u32, timestamp: u64) -> u64 {
    let _req_type = TURN_ALLOCATE_REQUEST;

    let allocation_id = client_id
        .wrapping_mul(0x0000_0000_CAFE_BABE)
        .wrapping_add(timestamp);

    // Generate a relay address hash from the allocation ID
    let relay_addr_hash = allocation_id.wrapping_mul(0x0000_0100_0000_0001);
    let relay_port = ((allocation_id & 0xFFFF) as u16) | 0x4000; // Ensure high port

    let allocation = TurnAllocation {
        allocation_id,
        client_id,
        relay_addr_hash,
        relay_port,
        lifetime,
        created: timestamp,
        bytes_relayed: 0,
    };

    let _resp_type = TURN_ALLOCATE_RESPONSE;

    let mut allocs = TURN_ALLOCS.lock();
    if let Some(ref mut list) = *allocs {
        if list.len() >= MAX_TURN_ALLOCATIONS {
            // Evict expired or oldest
            if !list.is_empty() {
                let mut oldest_idx = 0;
                let mut oldest_ts = list[0].created;
                for (i, a) in list.iter().enumerate() {
                    if a.created < oldest_ts {
                        oldest_ts = a.created;
                        oldest_idx = i;
                    }
                }
                list.remove(oldest_idx);
            }
        }
        list.push(allocation);
        allocation_id
    } else {
        0
    }
}

/// Refresh a TURN allocation's lifetime.
pub fn turn_refresh(allocation_id: u64, new_lifetime: u32) -> bool {
    let mut allocs = TURN_ALLOCS.lock();
    if let Some(ref mut list) = *allocs {
        for alloc in list.iter_mut() {
            if alloc.allocation_id == allocation_id {
                alloc.lifetime = new_lifetime;
                return true;
            }
        }
    }
    false
}

/// Release a TURN allocation.
pub fn turn_release(allocation_id: u64) -> bool {
    let mut allocs = TURN_ALLOCS.lock();
    if let Some(ref mut list) = *allocs {
        let before = list.len();
        list.retain(|a| a.allocation_id != allocation_id);
        list.len() < before
    } else {
        false
    }
}

/// Get a TURN allocation by ID.
pub fn get_allocation(allocation_id: u64) -> Option<TurnAllocation> {
    let allocs = TURN_ALLOCS.lock();
    if let Some(ref list) = *allocs {
        for alloc in list.iter() {
            if alloc.allocation_id == allocation_id {
                return Some(alloc.clone());
            }
        }
    }
    None
}

/// Expire TURN allocations whose lifetime has been exceeded.
pub fn expire_allocations(now: u64) -> usize {
    let mut allocs = TURN_ALLOCS.lock();
    if let Some(ref mut list) = *allocs {
        let before = list.len();
        list.retain(|a| {
            let age = now.saturating_sub(a.created);
            age < a.lifetime as u64
        });
        before - list.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Get relay subsystem statistics.
pub fn get_stats() -> RelayStats {
    let (active_sessions, total_bytes) = {
        let sessions = RELAY_SESSIONS.lock();
        match *sessions {
            Some(ref list) => {
                let active = list.iter().filter(|s| s.established).count();
                let bytes: u64 = list.iter().map(|s| s.bytes_relayed).sum();
                (active, bytes)
            }
            None => (0, 0),
        }
    };

    let active_allocations = {
        let allocs = TURN_ALLOCS.lock();
        match *allocs {
            Some(ref list) => list.len(),
            None => 0,
        }
    };

    let (punch_attempts, punch_successes) = {
        let punches = HOLE_PUNCHES.lock();
        let attempts = match *punches {
            Some(ref list) => list.len(),
            None => 0,
        };
        let successes = {
            let count = PUNCH_SUCCESS_COUNT.lock();
            *count
        };
        (attempts, successes)
    };

    RelayStats {
        active_sessions,
        total_bytes_relayed: total_bytes,
        active_allocations,
        hole_punch_attempts: punch_attempts,
        hole_punch_successes: punch_successes,
    }
}

/// Check if the relay subsystem is active.
pub fn is_active() -> bool {
    let active = RELAY_ACTIVE.lock();
    *active
}

/// Shutdown the relay subsystem, closing all sessions and allocations.
pub fn shutdown() {
    {
        let mut sessions = RELAY_SESSIONS.lock();
        *sessions = Some(Vec::new());
    }
    {
        let mut allocs = TURN_ALLOCS.lock();
        *allocs = Some(Vec::new());
    }
    {
        let mut punches = HOLE_PUNCHES.lock();
        *punches = Some(Vec::new());
    }
    {
        let mut active = RELAY_ACTIVE.lock();
        *active = false;
    }
}
