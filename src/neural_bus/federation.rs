use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::string::String;
/// Cross-device neural bus federation
///
/// Part of the AIOS neural bus layer. Bridges neural bus signals across
/// devices on a local network so that multiple Genesis AIOS instances
/// can share observations, coordinate predictions, and distribute
/// inference workloads.
///
/// Each device is a "peer" with a unique device ID and network address.
/// The federation protocol:
///   1. Discovery: peers announce themselves via broadcast packets
///   2. Handshake: mutual authentication with trust levels
///   3. Sync: continuous bidirectional signal relay
///   4. Conflict resolution: vector-clock-based ordering
///
/// Signals are serialised into a compact binary envelope before
/// transmission and deserialised on receipt.
use alloc::vec::Vec;

/// Trust level for a federated peer
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    /// Unknown / newly discovered peer
    Untrusted = 0,
    /// Peer has been seen before but not verified
    Recognized = 1,
    /// Peer has passed authentication
    Verified = 2,
    /// Peer is a fully trusted member of the same user's device fleet
    Trusted = 3,
}

/// Connection state of a peer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Not connected
    Disconnected,
    /// Discovery broadcast received, handshake pending
    Discovered,
    /// Handshake in progress
    Handshaking,
    /// Fully connected and syncing
    Connected,
    /// Temporarily unreachable (will retry)
    Stale,
}

/// A remote device on the federated bus
pub struct FederatedPeer {
    /// Unique device identifier
    pub device_id: String,
    /// Network address (e.g. "192.168.1.42:9876")
    pub address: String,
    /// Whether peer is connected (legacy field)
    pub connected: bool,
    /// Detailed connection state
    pub state: PeerState,
    /// Trust level of this peer
    pub trust: TrustLevel,
    /// Round-trip latency estimate in microseconds
    pub latency_us: u64,
    /// Vector clock counter for this peer
    pub vector_clock: u64,
    /// Signals sent to this peer
    pub signals_sent: u64,
    /// Signals received from this peer
    pub signals_received: u64,
    /// Last heartbeat timestamp (monotonic)
    pub last_heartbeat: u64,
    /// Number of failed connection attempts
    pub failed_attempts: u32,
    /// Encryption key fingerprint (simplified: first 8 bytes of hash)
    pub key_fingerprint: u64,
}

impl FederatedPeer {
    /// Create a new peer entry.
    pub fn new(device_id: &str, address: &str) -> Self {
        FederatedPeer {
            device_id: String::from(device_id),
            address: String::from(address),
            connected: false,
            state: PeerState::Discovered,
            trust: TrustLevel::Untrusted,
            latency_us: 0,
            vector_clock: 0,
            signals_sent: 0,
            signals_received: 0,
            last_heartbeat: 0,
            failed_attempts: 0,
            key_fingerprint: fnv_hash(device_id.as_bytes()),
        }
    }

    /// Check if the peer is healthy (connected and responsive).
    pub fn is_healthy(&self, current_time: u64, timeout: u64) -> bool {
        self.state == PeerState::Connected
            && current_time.saturating_sub(self.last_heartbeat) < timeout
    }
}

/// Bridges neural bus signals across devices
pub struct Federation {
    /// Known peers
    pub peers: Vec<FederatedPeer>,
    /// This device's unique ID
    pub local_id: String,
    /// This device's vector clock
    pub local_clock: u64,
    /// Maximum number of peers
    pub max_peers: usize,
    /// Heartbeat interval (in ticks)
    pub heartbeat_interval: u64,
    /// Tick counter
    pub tick: u64,
    /// Send buffer: serialised signal envelopes waiting to be transmitted
    send_buffer: Vec<Vec<u8>>,
    /// Receive buffer: signal envelopes received from peers
    recv_buffer: Vec<Vec<u8>>,
    /// Maximum send buffer size (in total bytes)
    pub max_buffer_bytes: usize,
    /// Current send buffer size
    buffer_bytes_used: usize,
    /// Total signals forwarded to remote peers
    pub total_forwarded: u64,
    /// Total signals received from remote peers
    pub total_received: u64,
    /// Whether federation is active
    pub active: bool,
}

// ── Simple FNV hash for fingerprinting ──────────────────────────────

fn fnv_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── Signal envelope binary format ───────────────────────────────────
//
// Header (16 bytes):
//   [0..8]  sender device ID hash (u64 LE)
//   [8..16] vector clock value (u64 LE)
// Payload:
//   [16..]  raw signal bytes

fn encode_envelope(sender_hash: u64, clock: u64, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + payload.len());
    buf.extend_from_slice(&sender_hash.to_le_bytes());
    buf.extend_from_slice(&clock.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

fn decode_envelope(data: &[u8]) -> Option<(u64, u64, &[u8])> {
    if data.len() < 16 {
        return None;
    }
    let mut sender_bytes = [0u8; 8];
    sender_bytes.copy_from_slice(&data[0..8]);
    let sender_hash = u64::from_le_bytes(sender_bytes);

    let mut clock_bytes = [0u8; 8];
    clock_bytes.copy_from_slice(&data[8..16]);
    let clock = u64::from_le_bytes(clock_bytes);

    Some((sender_hash, clock, &data[16..]))
}

impl Federation {
    /// Create a new federation bridge for this device.
    pub fn new(local_id: &str) -> Self {
        serial_println!("    [federation] Initialising for device '{}'", local_id);
        Federation {
            peers: Vec::new(),
            local_id: String::from(local_id),
            local_clock: 0,
            max_peers: 32,
            heartbeat_interval: 100,
            tick: 0,
            send_buffer: Vec::new(),
            recv_buffer: Vec::new(),
            max_buffer_bytes: 1024 * 1024, // 1 MB
            buffer_bytes_used: 0,
            total_forwarded: 0,
            total_received: 0,
            active: true,
        }
    }

    /// Add a peer to the federation.
    pub fn add_peer(&mut self, device_id: &str, address: &str) {
        // Check for duplicate
        for peer in &self.peers {
            if peer.device_id == device_id {
                serial_println!("    [federation] Peer '{}' already known", device_id);
                return;
            }
        }

        if self.peers.len() >= self.max_peers {
            // Evict the least-trusted, oldest peer
            self.evict_worst_peer();
        }

        let peer = FederatedPeer::new(device_id, address);
        serial_println!("    [federation] Added peer '{}' at {}", device_id, address);
        self.peers.push(peer);
    }

    /// Initiate a handshake with a peer.
    pub fn handshake(&mut self, device_id: &str) -> bool {
        for peer in self.peers.iter_mut() {
            if peer.device_id == device_id {
                match peer.state {
                    PeerState::Discovered | PeerState::Stale => {
                        peer.state = PeerState::Handshaking;
                        // In a real system, we'd exchange crypto challenges here.
                        // Simulate success:
                        peer.state = PeerState::Connected;
                        peer.connected = true;
                        peer.trust = TrustLevel::Recognized;
                        peer.last_heartbeat = self.tick;
                        serial_println!(
                            "    [federation] Handshake with '{}' succeeded",
                            device_id
                        );
                        return true;
                    }
                    PeerState::Connected => return true, // Already connected
                    _ => {
                        peer.failed_attempts = peer.failed_attempts.saturating_add(1);
                        return false;
                    }
                }
            }
        }
        false
    }

    /// Broadcast a signal to all connected peers.
    pub fn broadcast_remote(&self, signal_bytes: &[u8]) {
        if !self.active || signal_bytes.is_empty() {
            return;
        }
        let sender_hash = fnv_hash(self.local_id.as_bytes());
        let envelope = encode_envelope(sender_hash, self.local_clock, signal_bytes);

        // In a real kernel, we'd enqueue this for the network stack.
        // For now, log the action.
        let connected_count = self
            .peers
            .iter()
            .filter(|p| p.state == PeerState::Connected)
            .count();

        if connected_count > 0 {
            serial_println!(
                "    [federation] Broadcasting {} bytes to {} peers",
                envelope.len(),
                connected_count
            );
        }
    }

    /// Send a signal to a specific peer by device ID.
    pub fn send_to_peer(&mut self, device_id: &str, signal_bytes: &[u8]) -> bool {
        let sender_hash = fnv_hash(self.local_id.as_bytes());
        self.local_clock += 1;
        let envelope = encode_envelope(sender_hash, self.local_clock, signal_bytes);

        for peer in self.peers.iter_mut() {
            if peer.device_id == device_id && peer.state == PeerState::Connected {
                // Buffer the envelope for transmission
                if self.buffer_bytes_used + envelope.len() <= self.max_buffer_bytes {
                    self.buffer_bytes_used += envelope.len();
                    peer.signals_sent = peer.signals_sent.saturating_add(1);
                    return true;
                } else {
                    serial_println!(
                        "    [federation] Send buffer full, dropping signal to '{}'",
                        device_id
                    );
                    return false;
                }
            }
        }
        false
    }

    /// Process incoming data from a peer.
    pub fn receive_from_peer(&mut self, device_id: &str, data: &[u8]) -> Option<Vec<u8>> {
        if let Some((_sender_hash, clock, payload)) = decode_envelope(data) {
            // Update peer's vector clock
            for peer in self.peers.iter_mut() {
                if peer.device_id == device_id {
                    if clock > peer.vector_clock {
                        peer.vector_clock = clock;
                    }
                    peer.signals_received = peer.signals_received.saturating_add(1);
                    peer.last_heartbeat = self.tick;
                    self.total_received = self.total_received.saturating_add(1);
                    break;
                }
            }
            // Update local clock: max(local, remote) + 1
            if clock >= self.local_clock {
                self.local_clock = clock + 1;
            }
            return Some(payload.to_vec());
        }
        None
    }

    /// Periodic tick: send heartbeats, detect stale peers.
    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1);
        let current_tick = self.tick;
        let hb_interval = self.heartbeat_interval;
        let timeout = hb_interval * 3;

        // Send heartbeats
        if current_tick % hb_interval == 0 {
            let sender_hash = fnv_hash(self.local_id.as_bytes());
            self.local_clock += 1;
            let _heartbeat = encode_envelope(sender_hash, self.local_clock, &[0xFF]); // 0xFF = heartbeat marker
            for peer in self.peers.iter_mut() {
                if peer.state == PeerState::Connected {
                    peer.signals_sent = peer.signals_sent.saturating_add(1);
                }
            }
        }

        // Detect stale peers
        for peer in self.peers.iter_mut() {
            if peer.state == PeerState::Connected {
                if current_tick.saturating_sub(peer.last_heartbeat) > timeout {
                    peer.state = PeerState::Stale;
                    peer.connected = false;
                    serial_println!("    [federation] Peer '{}' went stale", peer.device_id);
                }
            }
        }
    }

    /// Get the number of connected peers.
    pub fn connected_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| p.state == PeerState::Connected)
            .count()
    }

    /// Get the number of trusted peers.
    pub fn trusted_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| p.trust >= TrustLevel::Verified)
            .count()
    }

    /// Remove a peer from the federation.
    pub fn remove_peer(&mut self, device_id: &str) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.device_id != device_id);
        before != self.peers.len()
    }

    /// Evict the worst peer (lowest trust, then oldest).
    fn evict_worst_peer(&mut self) {
        if self.peers.is_empty() {
            return;
        }
        let mut worst_idx = 0;
        let mut worst_trust = TrustLevel::Trusted;
        let mut worst_heartbeat = u64::MAX;

        for (i, peer) in self.peers.iter().enumerate() {
            if peer.trust < worst_trust
                || (peer.trust == worst_trust && peer.last_heartbeat < worst_heartbeat)
            {
                worst_trust = peer.trust;
                worst_heartbeat = peer.last_heartbeat;
                worst_idx = i;
            }
        }
        serial_println!(
            "    [federation] Evicting peer '{}' (trust={:?})",
            self.peers[worst_idx].device_id,
            worst_trust
        );
        self.peers.swap_remove(worst_idx);
    }

    /// Upgrade a peer's trust level.
    pub fn set_trust(&mut self, device_id: &str, trust: TrustLevel) {
        for peer in self.peers.iter_mut() {
            if peer.device_id == device_id {
                peer.trust = trust;
                serial_println!(
                    "    [federation] Set trust for '{}' to {:?}",
                    device_id,
                    trust
                );
                return;
            }
        }
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct FederationState {
    federation: Federation,
}

static FEDERATION: Mutex<Option<FederationState>> = Mutex::new(None);

pub fn init() {
    let federation = Federation::new("genesis-local-0");
    let mut guard = FEDERATION.lock();
    *guard = Some(FederationState { federation });
    serial_println!("    [federation] Cross-device federation subsystem initialised");
}

/// Add a peer to the global federation.
pub fn add_peer_global(device_id: &str, address: &str) {
    let mut guard = FEDERATION.lock();
    if let Some(state) = guard.as_mut() {
        state.federation.add_peer(device_id, address);
    }
}

/// Broadcast signal bytes to all federated peers.
pub fn broadcast_global(signal_bytes: &[u8]) {
    let guard = FEDERATION.lock();
    if let Some(state) = guard.as_ref() {
        state.federation.broadcast_remote(signal_bytes);
    }
}
