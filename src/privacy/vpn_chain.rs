use crate::sync::Mutex;
/// Multi-Hop VPN Chaining for Genesis
///
/// Cascades multiple VPN connections through different servers and protocols
/// so that no single VPN provider can observe both source and destination.
/// Supports WireGuard, OpenVPN, IPSec, and custom tunnel protocols.
///
/// Features:
///   - Dynamic hop addition/removal without full reconnect
///   - Exit node rotation for IP diversity
///   - Latency measurement and monitoring per hop
///   - DNS and WebRTC leak testing
///   - Apparent IP tracking after each chain modification
///
/// All cryptographic operations are simulated via hash-based transforms
/// (no floating point). Latency values use integer milliseconds.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum hops in a VPN chain
const MAX_CHAIN_HOPS: usize = 8;

/// Default handshake timeout in milliseconds
const HANDSHAKE_TIMEOUT_MS: u32 = 5000;

/// Keep-alive interval in milliseconds
const KEEPALIVE_INTERVAL_MS: u32 = 25000;

/// Hash seed for VPN key derivation
const VPN_KEY_SEED: u64 = 0x5CA1AB1EDE1EC7ED;

/// Hash seed for apparent IP derivation
const IP_DERIVE_SEED: u64 = 0xADDE55C0DE0F1CE5;

/// Hash seed for leak test tokens
const LEAK_TEST_SEED: u64 = 0x1EA40E570FEDFACE;

/// MTU for VPN tunnel (accounting for encapsulation overhead)
const VPN_MTU: u32 = 1400;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// VPN protocol used for a hop
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnProtocol {
    /// WireGuard — modern, fast, minimal codebase
    WireGuard,
    /// OpenVPN — widely supported, TLS-based
    OpenVPN,
    /// IPSec/IKEv2 — standards-based, enterprise-grade
    IPSec,
    /// Custom protocol for specialized use cases
    Custom,
}

/// Connection state of a VPN hop
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopState {
    /// Not yet connected
    Disconnected,
    /// Handshake in progress
    Connecting,
    /// Fully established
    Connected,
    /// Connection lost, attempting reconnect
    Reconnecting,
    /// Permanently failed
    Failed,
}

/// A single hop in the VPN chain
#[derive(Debug, Clone)]
pub struct VpnHop {
    /// Unique hop identifier
    pub id: u64,
    /// Hash of the server address/endpoint
    pub server_hash: u64,
    /// VPN protocol used for this hop
    pub protocol: VpnProtocol,
    /// Whether this hop is currently connected
    pub connected: bool,
    /// Measured latency to this hop in milliseconds
    pub latency_ms: u32,
    /// Current connection state
    state: HopState,
    /// Session key hash for this hop
    session_key: u64,
    /// Bytes sent through this hop
    bytes_sent: u64,
    /// Bytes received through this hop
    bytes_received: u64,
    /// Handshake completion tick
    handshake_tick: u64,
    /// Server geographic region hash (for diversity)
    region_hash: u64,
}

/// A chain of cascaded VPN hops
#[derive(Debug, Clone)]
pub struct VpnChain {
    /// Ordered list of VPN hops in the chain
    pub hops: Vec<VpnHop>,
    /// Whether the entire chain is active
    pub active: bool,
    /// Sum of latencies across all hops in milliseconds
    pub total_latency: u32,
    /// Total bytes sent through the chain
    pub bytes_through: u64,
    /// Chain creation tick
    created_tick: u64,
    /// Last measured apparent IP hash at the exit
    apparent_ip_hash: u64,
    /// Number of exit rotations performed
    rotation_count: u32,
}

/// Result of a leak test
#[derive(Debug, Clone)]
pub struct LeakTestResult {
    /// Whether a DNS leak was detected
    pub dns_leak: bool,
    /// Whether a WebRTC leak was detected
    pub webrtc_leak: bool,
    /// Whether an IPv6 leak was detected
    pub ipv6_leak: bool,
    /// Hash of the observed external IP (should match chain exit)
    pub observed_ip_hash: u64,
    /// Hash of the expected IP from chain exit
    pub expected_ip_hash: u64,
    /// Whether the test passed overall
    pub passed: bool,
}

/// Latency measurement for a single hop
#[derive(Debug, Clone)]
pub struct HopLatency {
    /// Hop ID
    pub hop_id: u64,
    /// Round-trip latency in milliseconds
    pub rtt_ms: u32,
    /// Jitter (variation) in milliseconds
    pub jitter_ms: u32,
    /// Packet loss estimate (Q16 fixed-point, 0 = 0%, 65536 = 100%)
    pub loss_q16: i32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// The active VPN chain
static CHAIN: Mutex<Option<VpnChain>> = Mutex::new(None);

/// Next hop ID counter
static NEXT_HOP_ID: Mutex<u64> = Mutex::new(1);

/// Simulated tick counter
static TICK: Mutex<u64> = Mutex::new(0);

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Hash function for VPN key derivation
fn vpn_hash(data: &[u8], seed: u64) -> u64 {
    let mut h: u64 = seed;
    for &b in data {
        h = h.wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(b as u64);
        h ^= h >> 27;
        h = h.wrapping_mul(0x9E3779B97F4A7C15);
    }
    h
}

/// Derive a session key for a VPN hop
fn derive_hop_session_key(hop_id: u64, server_hash: u64, protocol: VpnProtocol) -> u64 {
    let mut buf = vec![0u8; 20];
    buf[0..8].copy_from_slice(&hop_id.to_le_bytes());
    buf[8..16].copy_from_slice(&server_hash.to_le_bytes());
    buf[16..20].copy_from_slice(&(protocol as u32).to_le_bytes());
    vpn_hash(&buf, VPN_KEY_SEED)
}

/// Derive the apparent IP hash for the chain exit
fn derive_apparent_ip(exit_server_hash: u64, session_key: u64) -> u64 {
    let mut buf = vec![0u8; 16];
    buf[0..8].copy_from_slice(&exit_server_hash.to_le_bytes());
    buf[8..16].copy_from_slice(&session_key.to_le_bytes());
    vpn_hash(&buf, IP_DERIVE_SEED)
}

/// Simulate latency measurement for a hop (returns ms based on hop properties)
fn simulate_latency(server_hash: u64, protocol: VpnProtocol) -> u32 {
    // Base latency varies by protocol
    let base: u32 = match protocol {
        VpnProtocol::WireGuard => 15,
        VpnProtocol::OpenVPN => 35,
        VpnProtocol::IPSec => 25,
        VpnProtocol::Custom => 20,
    };
    // Add variation based on server hash
    let variation = ((server_hash >> 16) as u32) % 50;
    base.wrapping_add(variation)
}

/// Get current tick
fn current_tick() -> u64 {
    let t = TICK.lock();
    *t
}

/// Get protocol name string
fn protocol_name(p: VpnProtocol) -> &'static str {
    match p {
        VpnProtocol::WireGuard => "WireGuard",
        VpnProtocol::OpenVPN => "OpenVPN",
        VpnProtocol::IPSec => "IPSec",
        VpnProtocol::Custom => "Custom",
    }
}

/// Recalculate chain aggregate values (total latency, apparent IP, etc.)
fn recalculate_chain(chain: &mut VpnChain) {
    chain.total_latency = 0;
    for hop in chain.hops.iter() {
        chain.total_latency = chain.total_latency.wrapping_add(hop.latency_ms);
    }

    // Update apparent IP from the last (exit) hop
    if let Some(exit_hop) = chain.hops.last() {
        chain.apparent_ip_hash = derive_apparent_ip(exit_hop.server_hash, exit_hop.session_key);
    } else {
        chain.apparent_ip_hash = 0;
    }

    // Chain is active only if all hops are connected
    chain.active = !chain.hops.is_empty() && chain.hops.iter().all(|h| h.connected);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a new VPN hop to the chain.
/// Returns the hop ID on success, or None if the chain is full.
pub fn add_hop(server_hash: u64, protocol: VpnProtocol) -> Option<u64> {
    let mut chain_guard = CHAIN.lock();
    let chain = chain_guard.get_or_insert_with(|| VpnChain {
        hops: Vec::new(),
        active: false,
        total_latency: 0,
        bytes_through: 0,
        created_tick: current_tick(),
        apparent_ip_hash: 0,
        rotation_count: 0,
    });

    if chain.hops.len() >= MAX_CHAIN_HOPS {
        serial_println!("  VPN: chain at maximum hops ({})", MAX_CHAIN_HOPS);
        return None;
    }

    let mut id_counter = NEXT_HOP_ID.lock();
    let hop_id = *id_counter;
    *id_counter = id_counter.wrapping_add(1);
    drop(id_counter);

    let session_key = derive_hop_session_key(hop_id, server_hash, protocol);
    let latency = simulate_latency(server_hash, protocol);
    let region_hash = vpn_hash(&server_hash.to_le_bytes(), 0xABCDEF0123456789);

    let hop = VpnHop {
        id: hop_id,
        server_hash,
        protocol,
        connected: false,
        latency_ms: latency,
        state: HopState::Disconnected,
        session_key,
        bytes_sent: 0,
        bytes_received: 0,
        handshake_tick: 0,
        region_hash,
    };

    chain.hops.push(hop);
    recalculate_chain(chain);

    serial_println!(
        "  VPN: hop {} added ({} @ server {:#018X}, latency ~{}ms)",
        hop_id,
        protocol_name(protocol),
        server_hash,
        latency
    );

    Some(hop_id)
}

/// Remove a hop from the chain by its ID.
/// Returns true if the hop was found and removed.
pub fn remove_hop(hop_id: u64) -> bool {
    let mut chain_guard = CHAIN.lock();
    let chain = match chain_guard.as_mut() {
        Some(c) => c,
        None => return false,
    };

    let before = chain.hops.len();
    chain.hops.retain(|h| h.id != hop_id);

    if chain.hops.len() < before {
        recalculate_chain(chain);
        serial_println!(
            "  VPN: hop {} removed ({} hops remaining)",
            hop_id,
            chain.hops.len()
        );
        true
    } else {
        false
    }
}

/// Connect the entire VPN chain, establishing each hop sequentially.
/// Returns true if all hops connected successfully.
pub fn connect_chain() -> bool {
    let mut chain_guard = CHAIN.lock();
    let chain = match chain_guard.as_mut() {
        Some(c) => c,
        None => {
            serial_println!("  VPN: no chain configured");
            return false;
        }
    };

    if chain.hops.is_empty() {
        serial_println!("  VPN: chain has no hops");
        return false;
    }

    let now = current_tick();
    let mut all_connected = true;

    for hop in chain.hops.iter_mut() {
        hop.state = HopState::Connecting;

        // Simulate handshake: derive a handshake token and verify
        let mut hs_buf = vec![0u8; 16];
        hs_buf[0..8].copy_from_slice(&hop.id.to_le_bytes());
        hs_buf[8..16].copy_from_slice(&hop.server_hash.to_le_bytes());
        let hs_token = vpn_hash(&hs_buf, hop.session_key);

        // Handshake succeeds if token has certain properties (simulated)
        if hs_token & 0x0F != 0x0F {
            hop.state = HopState::Connected;
            hop.connected = true;
            hop.handshake_tick = now;
            serial_println!(
                "  VPN: hop {} connected ({}, handshake={:#018X})",
                hop.id,
                protocol_name(hop.protocol),
                hs_token
            );
        } else {
            hop.state = HopState::Failed;
            hop.connected = false;
            all_connected = false;
            serial_println!("  VPN: hop {} handshake FAILED", hop.id);
        }
    }

    recalculate_chain(chain);

    if chain.active {
        serial_println!(
            "  VPN: chain connected ({} hops, total latency {}ms, apparent_ip={:#018X})",
            chain.hops.len(),
            chain.total_latency,
            chain.apparent_ip_hash
        );
    } else {
        serial_println!("  VPN: chain partially connected");
    }

    all_connected
}

/// Disconnect the entire VPN chain, tearing down all hops.
pub fn disconnect() {
    let mut chain_guard = CHAIN.lock();
    if let Some(chain) = chain_guard.as_mut() {
        for hop in chain.hops.iter_mut() {
            if hop.connected {
                serial_println!(
                    "  VPN: disconnecting hop {} (sent={}, recv={})",
                    hop.id,
                    hop.bytes_sent,
                    hop.bytes_received
                );
            }
            hop.connected = false;
            hop.state = HopState::Disconnected;
            // Clear session key for security
            hop.session_key = 0;
        }
        chain.active = false;
        chain.apparent_ip_hash = 0;
        serial_println!(
            "  VPN: chain disconnected ({} bytes total)",
            chain.bytes_through
        );
    }
}

/// Rotate the exit node (last hop) to get a new apparent IP.
/// Replaces the exit hop with a new server while keeping other hops intact.
pub fn rotate_exit(new_server_hash: u64, protocol: VpnProtocol) -> bool {
    let mut chain_guard = CHAIN.lock();
    let chain = match chain_guard.as_mut() {
        Some(c) => c,
        None => return false,
    };

    if chain.hops.is_empty() {
        return false;
    }

    let old_exit = chain.hops.last().cloned();

    // Remove old exit
    chain.hops.pop();

    // Create new exit hop
    let mut id_counter = NEXT_HOP_ID.lock();
    let hop_id = *id_counter;
    *id_counter = id_counter.wrapping_add(1);
    drop(id_counter);

    let session_key = derive_hop_session_key(hop_id, new_server_hash, protocol);
    let latency = simulate_latency(new_server_hash, protocol);
    let region_hash = vpn_hash(&new_server_hash.to_le_bytes(), 0xABCDEF0123456789);

    let new_hop = VpnHop {
        id: hop_id,
        server_hash: new_server_hash,
        protocol,
        connected: true, // auto-connect on rotation
        latency_ms: latency,
        state: HopState::Connected,
        session_key,
        bytes_sent: 0,
        bytes_received: 0,
        handshake_tick: current_tick(),
        region_hash,
    };

    chain.hops.push(new_hop);
    chain.rotation_count = chain.rotation_count.wrapping_add(1);
    recalculate_chain(chain);

    if let Some(old) = old_exit {
        serial_println!(
            "  VPN: exit rotated from {:#018X} to {:#018X} (rotation #{})",
            old.server_hash,
            new_server_hash,
            chain.rotation_count
        );
    }

    true
}

/// Measure latency for each hop in the chain.
/// Returns a vector of per-hop latency measurements.
pub fn measure_latency() -> Vec<HopLatency> {
    let chain_guard = CHAIN.lock();
    let chain = match chain_guard.as_ref() {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut results = Vec::new();
    for hop in chain.hops.iter() {
        let base_rtt = hop.latency_ms;
        // Simulate jitter as a fraction of base latency
        let jitter = (hop.server_hash as u32 % 10).wrapping_add(1);
        // Simulate packet loss (Q16 fixed-point: 0 = no loss)
        let loss_q16: i32 = if hop.connected {
            (hop.server_hash as i32) & 0x0FFF // 0-4095 out of 65536 = 0-6.25% loss
        } else {
            65536 // 100% loss if disconnected
        };

        results.push(HopLatency {
            hop_id: hop.id,
            rtt_ms: base_rtt.wrapping_mul(2), // round-trip
            jitter_ms: jitter,
            loss_q16,
        });
    }

    if !results.is_empty() {
        let total_rtt: u32 = results.iter().map(|r| r.rtt_ms).sum();
        serial_println!(
            "  VPN: latency measured ({} hops, total RTT ~{}ms)",
            results.len(),
            total_rtt
        );
    }

    results
}

/// Get the apparent IP hash as seen from outside the VPN chain.
/// This is the IP hash at the exit node.
pub fn get_apparent_ip() -> u64 {
    let chain_guard = CHAIN.lock();
    match chain_guard.as_ref() {
        Some(chain) if chain.active => chain.apparent_ip_hash,
        _ => 0,
    }
}

/// Run a leak test to check for DNS, WebRTC, and IPv6 leaks.
/// Returns a detailed test result.
pub fn test_leak() -> LeakTestResult {
    let chain_guard = CHAIN.lock();
    let chain = match chain_guard.as_ref() {
        Some(c) => c,
        None => {
            return LeakTestResult {
                dns_leak: true,
                webrtc_leak: true,
                ipv6_leak: true,
                observed_ip_hash: 0,
                expected_ip_hash: 0,
                passed: false,
            };
        }
    };

    if !chain.active || chain.hops.is_empty() {
        return LeakTestResult {
            dns_leak: true,
            webrtc_leak: true,
            ipv6_leak: true,
            observed_ip_hash: 0,
            expected_ip_hash: chain.apparent_ip_hash,
            passed: false,
        };
    }

    let expected_ip = chain.apparent_ip_hash;

    // Simulate leak detection by hashing chain properties
    let mut test_buf = vec![0u8; 24];
    test_buf[0..8].copy_from_slice(&expected_ip.to_le_bytes());
    test_buf[8..16].copy_from_slice(&chain.bytes_through.to_le_bytes());
    test_buf[16..24].copy_from_slice(&(chain.hops.len() as u64).to_le_bytes());
    let test_token = vpn_hash(&test_buf, LEAK_TEST_SEED);

    // DNS leak: check if DNS queries escape the tunnel
    let dns_leak = (test_token & 0x01) != 0 && chain.hops.len() < 2;
    // WebRTC leak: check if STUN/TURN reveals real IP
    let webrtc_leak = (test_token & 0x02) != 0 && chain.hops.len() < 3;
    // IPv6 leak: check if IPv6 traffic bypasses tunnel
    let ipv6_leak = (test_token & 0x04) != 0 && chain.hops.len() < 2;

    let observed_ip = if dns_leak || webrtc_leak || ipv6_leak {
        // Simulated real IP leaking through
        vpn_hash(&test_buf, 0xBAD1EABAD1EA0000)
    } else {
        expected_ip
    };

    let passed = !dns_leak && !webrtc_leak && !ipv6_leak && observed_ip == expected_ip;

    serial_println!(
        "  VPN: leak test {} (dns={}, webrtc={}, ipv6={})",
        if passed { "PASSED" } else { "FAILED" },
        dns_leak,
        webrtc_leak,
        ipv6_leak
    );

    LeakTestResult {
        dns_leak,
        webrtc_leak,
        ipv6_leak,
        observed_ip_hash: observed_ip,
        expected_ip_hash: expected_ip,
        passed,
    }
}

/// Get a summary of the current chain state
pub fn get_chain_info() -> Option<(usize, bool, u32, u64, u64, u32)> {
    let chain_guard = CHAIN.lock();
    chain_guard.as_ref().map(|c| {
        (
            c.hops.len(),
            c.active,
            c.total_latency,
            c.bytes_through,
            c.apparent_ip_hash,
            c.rotation_count,
        )
    })
}

/// Initialize the VPN chain subsystem
pub fn init() {
    // Ensure chain state is clean
    let mut chain_guard = CHAIN.lock();
    *chain_guard = None;
    drop(chain_guard);

    // Reset counters
    let mut id = NEXT_HOP_ID.lock();
    *id = 1;
    drop(id);

    let mut tick = TICK.lock();
    *tick = 0;

    serial_println!(
        "  VPN: multi-hop chain subsystem initialized (max {} hops, MTU {})",
        MAX_CHAIN_HOPS,
        VPN_MTU
    );
}
