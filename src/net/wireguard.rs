use super::{Ipv4Addr, NetError};
use crate::crypto::chacha20;
use crate::sync::Mutex;
/// WireGuard VPN protocol for Genesis (Hoags VPN)
///
/// Native kernel-level WireGuard implementation.
/// WireGuard is a modern VPN protocol that's simpler and faster
/// than IPsec or OpenVPN.
///
/// Protocol: UDP-based, uses Noise IK handshake pattern,
/// ChaCha20-Poly1305 for encryption, BLAKE2s for hashing,
/// Curve25519 for key exchange.
///
/// This is the Hoags VPN — built into the kernel, not bolted on.
///
/// Inspired by: WireGuard whitepaper (Jason Donenfeld),
/// Linux kernel WireGuard module. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// WireGuard message types
pub const MSG_HANDSHAKE_INIT: u8 = 1;
pub const MSG_HANDSHAKE_RESP: u8 = 2;
pub const MSG_COOKIE: u8 = 3;
pub const MSG_DATA: u8 = 4;

/// WireGuard default port
pub const DEFAULT_PORT: u16 = 51820;

// ---------------------------------------------------------------------------
// Session counter and anti-replay
// ---------------------------------------------------------------------------

/// Monotonic session counter — incremented for every outbound data packet.
/// WireGuard uses a 64-bit "nonce" counter; wrapping would be catastrophic so
/// we use saturating arithmetic (at 2^64-1 packets the tunnel is dead anyway).
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Advance the session counter and return the previous value (the nonce to use).
#[inline]
fn next_session_counter() -> u64 {
    SESSION_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Anti-replay window state: tracks the 64 most recent counters seen.
///
/// Layout:
///   `window_top` — the highest counter accepted so far.
///   `window_bits` — a 64-bit bitmask; bit N is set if counter
///                   (window_top - N) has been seen.
///
/// A packet is accepted iff:
///   1. Its counter > window_top  (new packet, slide window), OR
///   2. Its counter is within [window_top - 63, window_top] and the
///      corresponding bit in `window_bits` is NOT yet set (not a replay).
struct ReplayWindow {
    window_top: u64,
    window_bits: u64,
}

impl ReplayWindow {
    const fn new() -> Self {
        ReplayWindow {
            window_top: 0,
            window_bits: 0,
        }
    }

    /// Check whether `counter` is acceptable (not replayed).
    /// If acceptable, marks it as seen and returns `true`.
    /// Returns `false` if the counter is a replay or too old.
    fn check_and_update(&mut self, counter: u64) -> bool {
        if counter > self.window_top {
            // New high-water mark — slide the window.
            let shift = counter.saturating_sub(self.window_top);
            if shift >= 64 {
                // Jumped more than a full window; reset.
                self.window_bits = 1; // mark the current top
            } else {
                self.window_bits = (self.window_bits << shift) | 1;
            }
            self.window_top = counter;
            true
        } else {
            // Counter is at or below the current top.
            let offset = self.window_top.saturating_sub(counter);
            if offset >= 64 {
                // Outside the 64-packet window — too old, reject.
                return false;
            }
            let mask = 1u64 << offset;
            if self.window_bits & mask != 0 {
                // Already seen — replay, reject.
                return false;
            }
            // Mark as seen.
            self.window_bits |= mask;
            true
        }
    }
}

// ---------------------------------------------------------------------------
// AEAD helpers (ChaCha20-Poly1305 wired to WireGuard nonce format)
// ---------------------------------------------------------------------------

/// WireGuard nonce encoding: 4-byte zero prefix || 8-byte little-endian counter.
///
/// WireGuard specifies a 96-bit nonce where the first 32 bits are always zero
/// and the counter occupies the remaining 64 bits (little-endian).
#[inline]
fn wg_nonce(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..12].copy_from_slice(&counter.to_le_bytes());
    nonce
}

/// Encrypt `plaintext` using ChaCha20-Poly1305 with a WireGuard-format nonce.
///
/// Returns `(ciphertext, tag)`.  The session key must be exactly 32 bytes.
/// The caller provides the 64-bit counter which is encoded into the nonce.
///
/// Additional authenticated data (`aad`) is authenticated but not encrypted;
/// for WireGuard data packets this is typically the 4-byte packet header.
pub fn wg_aead_encrypt(
    key: &[u8; 32],
    counter: u64,
    aad: &[u8],
    plaintext: &[u8],
) -> (Vec<u8>, [u8; 16]) {
    let nonce = wg_nonce(counter);
    let mut ct = Vec::from(plaintext);
    let tag = chacha20::aead_encrypt(key, &nonce, aad, &mut ct);
    (ct, tag)
}

/// Decrypt `ciphertext` using ChaCha20-Poly1305 with a WireGuard-format nonce.
///
/// Returns `Ok(plaintext)` if the tag verifies, or `Err(())` if the tag is
/// invalid (indicates tampering or key mismatch).
pub fn wg_aead_decrypt(
    key: &[u8; 32],
    counter: u64,
    aad: &[u8],
    ciphertext: &[u8],
    tag: &[u8; 16],
) -> Result<Vec<u8>, ()> {
    let nonce = wg_nonce(counter);
    let mut pt = Vec::from(ciphertext);
    chacha20::aead_decrypt(key, &nonce, aad, &mut pt, tag)?;
    Ok(pt)
}

/// A Curve25519 public key (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublicKey(pub [u8; 32]);

/// A Curve25519 private key (32 bytes)
#[derive(Clone)]
pub struct PrivateKey(pub [u8; 32]);

/// A pre-shared key (32 bytes, optional)
#[derive(Clone)]
pub struct PresharedKey(pub [u8; 32]);

/// WireGuard peer configuration
pub struct Peer {
    pub public_key: PublicKey,
    pub preshared_key: Option<PresharedKey>,
    pub endpoint: Option<(Ipv4Addr, u16)>,
    pub allowed_ips: Vec<(Ipv4Addr, u8)>, // (network, prefix_len)
    pub keepalive_interval: u16,          // seconds, 0 = disabled
    // Session state
    pub last_handshake: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub session_index: u32,
    /// Sending session key derived from the Noise handshake (all-zero until a
    /// real handshake is complete — used as a placeholder).
    pub session_key_send: [u8; 32],
    /// Receiving session key derived from the Noise handshake.
    pub session_key_recv: [u8; 32],
    /// Per-peer anti-replay window for inbound data packets.
    replay_window: ReplayWindow,
}

impl Peer {
    /// Create a new peer with a given public key and optional pre-shared key.
    ///
    /// Session keys default to all-zero until a successful Noise IK handshake
    /// derives real keys via X25519 + BLAKE2s HKDF.
    pub fn new(public_key: PublicKey, preshared_key: Option<PresharedKey>) -> Self {
        Peer {
            public_key,
            preshared_key,
            endpoint: None,
            allowed_ips: Vec::new(),
            keepalive_interval: 0,
            last_handshake: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            session_index: 0,
            session_key_send: [0u8; 32],
            session_key_recv: [0u8; 32],
            replay_window: ReplayWindow::new(),
        }
    }
}

/// WireGuard interface configuration
pub struct WireGuardInterface {
    pub name: &'static str,
    pub private_key: PrivateKey,
    pub listen_port: u16,
    pub peers: BTreeMap<PublicKey, Peer>,
    pub address: Ipv4Addr,
    pub mtu: u16,
}

impl WireGuardInterface {
    /// Create a new WireGuard interface
    pub fn new(name: &'static str, private_key: PrivateKey, listen_port: u16) -> Self {
        WireGuardInterface {
            name,
            private_key,
            listen_port,
            peers: BTreeMap::new(),
            address: Ipv4Addr::ANY,
            mtu: 1420, // WireGuard default MTU
        }
    }

    /// Add a peer
    pub fn add_peer(&mut self, peer: Peer) {
        self.peers.insert(peer.public_key, peer);
    }

    /// Remove a peer by public key
    pub fn remove_peer(&mut self, key: &PublicKey) {
        self.peers.remove(key);
    }

    /// Process an incoming WireGuard packet
    pub fn process_incoming(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, NetError> {
        if data.len() < 4 {
            return Err(NetError::InvalidPacket);
        }

        let msg_type = data[0];

        match msg_type {
            MSG_HANDSHAKE_INIT => self.handle_handshake_init(data),
            MSG_HANDSHAKE_RESP => self.handle_handshake_response(data),
            MSG_DATA => self.handle_data(data),
            _ => Err(NetError::InvalidPacket),
        }
    }

    /// Handle handshake initiation (Noise IK — initiator → responder)
    ///
    /// Packet layout (148 bytes):
    ///   [type(1)][reserved(3)][sender_index(4)][ephemeral_pub(32)]
    ///   [encrypted_static(32+16)][encrypted_timestamp(12+16)][mac1(16)][mac2(16)]
    ///
    /// At this stage we parse the sender index and ephemeral key, log receipt,
    /// and build a minimal handshake response with placeholder session keys.
    /// Full Noise IK (X25519 DH + BLAKE2s chain key) follows once the DH
    /// primitives are fully wired.
    fn handle_handshake_init(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, NetError> {
        if data.len() < 148 {
            return Err(NetError::InvalidPacket);
        }

        let sender_index = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        // Ephemeral public key from the initiator (bytes 8..40)
        let _ephemeral_pub = &data[8..40];

        serial_println!(
            "  WireGuard: handshake initiation from sender_index={}",
            sender_index
        );

        // Noise IK step 1-5: Full implementation requires X25519 DH with the
        // local static private key and the peer's static key extracted from the
        // encrypted_static field.  That DH is deferred until x25519::diffie_hellman
        // is exposed from the crypto layer.  For now we acknowledge receipt and
        // return None (no response packet) so the caller can track state.
        //
        // When X25519 is wired, the sequence will be:
        //   hs_key = DH(local_static_priv, peer_ephemeral_pub)
        //   decrypt static_key field → look up peer
        //   derive session keys via BLAKE2s HKDF
        //   build and return MSG_HANDSHAKE_RESP (92 bytes)

        Ok(None)
    }

    /// Handle handshake response (Noise IK — responder → initiator)
    ///
    /// Packet layout (92 bytes):
    ///   [type(1)][reserved(3)][sender_index(4)][receiver_index(4)]
    ///   [ephemeral_pub(32)][encrypted_nothing(0+16)][mac1(16)][mac2(16)]
    fn handle_handshake_response(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, NetError> {
        if data.len() < 92 {
            return Err(NetError::InvalidPacket);
        }

        let sender_index = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let receiver_index = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);

        serial_println!(
            "  WireGuard: handshake response sender={} receiver={}",
            sender_index,
            receiver_index
        );

        // TODO (Noise IK step 6-7): DH(local_ephemeral_priv, peer_ephemeral_pub)
        // then derive final send/recv session keys via BLAKE2s HKDF and store
        // them in the matching peer's session_key_send / session_key_recv fields.

        Ok(None)
    }

    /// Handle an encrypted WireGuard data packet.
    ///
    /// Packet layout:
    ///   [type(1)][reserved(3)][receiver_index(4)][counter(8)]
    ///   [encrypted_ip_packet(N)][poly1305_tag(16)]
    ///
    /// 1. Parse receiver_index and counter.
    /// 2. Look up the peer by receiver_index.
    /// 3. Anti-replay check via the per-peer ReplayWindow.
    /// 4. Decrypt with ChaCha20-Poly1305 using the peer's recv session key.
    /// 5. Return the plaintext IP packet.
    fn handle_data(&mut self, data: &[u8]) -> Result<Option<Vec<u8>>, NetError> {
        // Minimum: 1 (type) + 3 (reserved) + 4 (index) + 8 (counter) + 16 (tag) = 32
        if data.len() < 32 {
            return Err(NetError::InvalidPacket);
        }

        let receiver_index = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let counter = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);

        // The encrypted payload is bytes 16..(len-16); the final 16 bytes are the tag.
        let payload_end = data.len().saturating_sub(16);
        if payload_end < 16 {
            return Err(NetError::InvalidPacket);
        }
        let ciphertext = &data[16..payload_end];
        let tag_bytes = &data[payload_end..];
        if tag_bytes.len() != 16 {
            return Err(NetError::InvalidPacket);
        }
        let tag: &[u8; 16] = tag_bytes.try_into().map_err(|_| NetError::InvalidPacket)?;

        // Locate the peer that owns this receiver_index.
        let peer = self
            .peers
            .values_mut()
            .find(|p| p.session_index == receiver_index)
            .ok_or(NetError::Unreachable)?;

        // Anti-replay: reject duplicates and packets too far behind the window.
        if !peer.replay_window.check_and_update(counter) {
            serial_println!(
                "  WireGuard: replay detected or counter too old (counter={}, index={})",
                counter,
                receiver_index
            );
            return Err(NetError::InvalidPacket);
        }

        // AAD for WireGuard data packets is the first 16 bytes of the packet
        // header (type || reserved || receiver_index || counter).
        let aad = &data[0..16];

        // Decrypt with the peer's receive session key.
        let session_key = peer.session_key_recv;
        let plaintext =
            wg_aead_decrypt(&session_key, counter, aad, ciphertext, tag).map_err(|_| {
                serial_println!("  WireGuard: AEAD tag mismatch on data packet");
                NetError::InvalidPacket
            })?;

        peer.rx_bytes = peer.rx_bytes.saturating_add(plaintext.len() as u64);

        Ok(Some(plaintext))
    }

    /// Encrypt and send data through the WireGuard tunnel to `peer_key`.
    ///
    /// Packet layout produced:
    ///   [type(1)][reserved(3)][receiver_index(4)][counter(8)]
    ///   [ciphertext(N)][poly1305_tag(16)]
    ///
    /// The counter is taken from the global `SESSION_COUNTER` and incremented
    /// atomically, ensuring strict monotonicity across concurrent callers.
    pub fn send_data(&mut self, peer_key: &PublicKey, data: &[u8]) -> Result<Vec<u8>, NetError> {
        let peer = self.peers.get_mut(peer_key).ok_or(NetError::Unreachable)?;

        // Acquire a monotonic counter for this packet.
        let counter = next_session_counter();

        // Build the unencrypted header so we can use it as AAD.
        // Header: [MSG_DATA][0,0,0][receiver_index LE][counter LE]
        let mut header = [0u8; 16];
        header[0] = MSG_DATA;
        // bytes 1-3 = reserved (already zero)
        header[4..8].copy_from_slice(&peer.session_index.to_le_bytes());
        header[8..16].copy_from_slice(&counter.to_le_bytes());

        // Encrypt the IP packet payload with the peer's send session key.
        let session_key = peer.session_key_send;
        let (ciphertext, tag) = wg_aead_encrypt(&session_key, counter, &header, data);

        peer.tx_bytes = peer.tx_bytes.saturating_add(data.len() as u64);

        // Assemble the final WireGuard data packet.
        // Total size: 16 (header) + len(ciphertext) + 16 (tag)
        let mut packet = Vec::with_capacity(16 + ciphertext.len() + 16);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&ciphertext);
        packet.extend_from_slice(&tag);

        Ok(packet)
    }

    /// Initiate a WireGuard handshake toward a peer.
    ///
    /// Builds a MSG_HANDSHAKE_INIT packet (148 bytes) with:
    ///   - A placeholder ephemeral public key (all-zero; real X25519 keygen to follow)
    ///   - Zero-filled encrypted_static and encrypted_timestamp fields
    ///   - Zero MAC1/MAC2 (real BLAKE2s MACs require the full Noise chain)
    ///
    /// When X25519 and BLAKE2s are fully wired the ephemeral keypair will be
    /// generated via `crypto::x25519::generate_keypair()` and the fields will
    /// be populated with proper DH output and BLAKE2s MACs.
    pub fn initiate_handshake(&self, peer_key: &PublicKey) -> Option<Vec<u8>> {
        // Verify the peer exists.
        let peer = self.peers.get(peer_key)?;

        // Allocate a local sender index (low 32 bits of counter).
        let sender_index = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst) as u32;

        serial_println!(
            "  WireGuard: initiating handshake to peer, sender_index={}",
            sender_index
        );

        // MSG_HANDSHAKE_INIT wire format (148 bytes):
        //   [0]     type      = MSG_HANDSHAKE_INIT (1)
        //   [1..4]  reserved  = 0x00 0x00 0x00
        //   [4..8]  sender_index (LE u32)
        //   [8..40] unencrypted_ephemeral (32-byte Curve25519 pubkey)
        //             → zeros; replace with actual ephemeral pubkey once
        //               crypto::x25519::generate_keypair() is exposed.
        //   [40..88] encrypted_static (32-byte static pubkey + 16-byte Poly1305 tag)
        //             → zeros; fill with Noise encrypt(static_pub) when DH is ready.
        //   [88..116] encrypted_timestamp (12-byte TAI64N + 16-byte tag)
        //             → zeros; fill with actual timestamp + encrypt when DH is ready.
        //   [116..132] mac1 (16 bytes, BLAKE2s over header)
        //             → zeros; fill when BLAKE2s MAC is wired.
        //   [132..148] mac2 (16 bytes, cookie-based)
        //             → zeros.

        let mut packet = [0u8; 148];
        packet[0] = MSG_HANDSHAKE_INIT;
        // reserved bytes 1-3 remain zero
        packet[4..8].copy_from_slice(&sender_index.to_le_bytes());
        // bytes 8..40: ephemeral public key — placeholder zeros.
        // bytes 40..116: encrypted fields — placeholder zeros.
        // bytes 116..148: MAC fields — placeholder zeros.

        // Suppress unused-variable warning for peer (used for future DH).
        let _ = peer;

        Some(packet.to_vec())
    }
}

/// Global WireGuard interfaces
static WG_INTERFACES: Mutex<Vec<WireGuardInterface>> = Mutex::new(Vec::new());

/// Initialize WireGuard subsystem
pub fn init() {
    serial_println!("  WireGuard: Hoags VPN subsystem ready");
}

/// Create a new WireGuard interface
pub fn create_interface(name: &'static str, private_key: [u8; 32], port: u16) {
    let iface = WireGuardInterface::new(name, PrivateKey(private_key), port);
    WG_INTERFACES.lock().push(iface);
    serial_println!("  WireGuard: created interface {} on port {}", name, port);
}
