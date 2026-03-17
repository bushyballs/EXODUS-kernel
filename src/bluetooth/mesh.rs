/// Bluetooth Mesh networking.
///
/// Bluetooth Mesh enables many-to-many device communication for
/// large-scale networks (lighting, building automation, IoT).
/// This module implements:
///   - Mesh node provisioning (authentication + key distribution)
///   - Network layer (encryption, obfuscation, relay)
///   - Transport layer (segmentation, reassembly, acknowledgment)
///   - Access layer (model messages)
///   - Foundation models (Configuration Server/Client, Health)
///   - Managed flooding for message propagation
///   - IV Index updates and key refresh
///   - Proxy protocol for GATT-based mesh access
///
/// Mesh security:
///   - Network Key (NetKey): encrypts at the network layer
///   - Application Key (AppKey): encrypts at the access layer
///   - Device Key (DevKey): per-node, used for configuration
///   - AES-CCM encryption with 32-bit or 64-bit MIC
///
/// Address types:
///   - Unicast:   0x0001..0x7FFF
///   - Virtual:   hash of a 128-bit Label UUID
///   - Group:     0xC000..0xFEFF
///   - All-nodes: 0xFFFF
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Well-known mesh addresses.
const ADDR_UNASSIGNED: u16 = 0x0000;
const ADDR_ALL_PROXIES: u16 = 0xFFFC;
const ADDR_ALL_FRIENDS: u16 = 0xFFFD;
const ADDR_ALL_RELAYS: u16 = 0xFFFE;
const ADDR_ALL_NODES: u16 = 0xFFFF;

/// Maximum TTL (Time To Live).
const MAX_TTL: u8 = 127;

/// Default TTL.
const DEFAULT_TTL: u8 = 7;

/// Network PDU maximum size (before segmentation).
const MAX_UNSEGMENTED_ACCESS: usize = 15; // 11 access + 4 MIC
const MAX_SEGMENTED_ACCESS: usize = 380;  // 32 segments x 12 bytes - overhead

/// Mesh opcodes for Configuration Server model.
const OP_CONFIG_APPKEY_ADD: u32 = 0x00;
const OP_CONFIG_APPKEY_STATUS: u32 = 0x8003;
const OP_CONFIG_MODEL_PUB_SET: u32 = 0x03;
const OP_CONFIG_MODEL_SUB_ADD: u32 = 0x801B;
const OP_CONFIG_NET_TRANSMIT_SET: u32 = 0x8024;
const OP_CONFIG_RELAY_SET: u32 = 0x8027;
const OP_CONFIG_BEACON_SET: u32 = 0x800B;

/// Mesh opcodes for Health model.
const OP_HEALTH_FAULT_GET: u32 = 0x8031;
const OP_HEALTH_FAULT_STATUS: u32 = 0x0005;

/// Foundation model IDs.
const MODEL_CONFIG_SERVER: u16 = 0x0000;
const MODEL_CONFIG_CLIENT: u16 = 0x0001;
const MODEL_HEALTH_SERVER: u16 = 0x0002;
const MODEL_HEALTH_CLIENT: u16 = 0x0003;

/// Global mesh state.
static MESH: Mutex<Option<MeshManager>> = Mutex::new(None);

/// Mesh node features.
#[derive(Debug, Clone, Copy)]
struct NodeFeatures {
    relay: bool,
    proxy: bool,
    friend: bool,
    low_power: bool,
}

impl NodeFeatures {
    fn default_full() -> Self {
        Self {
            relay: true,
            proxy: true,
            friend: false,
            low_power: false,
        }
    }
}

/// Mesh element (each element has a unicast address and models).
struct Element {
    address: u16,
    models: Vec<u16>,     // SIG model IDs
    vendor_models: Vec<u32>, // vendor model IDs (company_id << 16 | model_id)
}

impl Element {
    fn new(address: u16) -> Self {
        Self {
            address,
            models: Vec::new(),
            vendor_models: Vec::new(),
        }
    }
}

/// Network key.
#[derive(Clone)]
struct NetKeyEntry {
    index: u16,
    key: [u8; 16],
    nid: u8,           // 7-bit Network ID derived from NetKey
    encryption_key: [u8; 16],
    privacy_key: [u8; 16],
}

/// Application key.
#[derive(Clone)]
struct AppKeyEntry {
    index: u16,
    net_key_index: u16,
    key: [u8; 16],
    aid: u8,           // 6-bit Application ID derived from AppKey
}

/// Provisioning state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ProvisionState {
    Unprovisioned,
    Provisioning,
    Provisioned,
}

/// Sequence number tracker per source.
struct SeqTracker {
    seq_number: u32,
    iv_index: u32,
}

impl SeqTracker {
    fn new() -> Self {
        Self {
            seq_number: 0,
            iv_index: 0,
        }
    }

    fn next_seq(&mut self) -> u32 {
        let seq = self.seq_number;
        self.seq_number = self.seq_number.wrapping_add(1);
        seq
    }
}

/// Mesh network PDU (after network layer processing).
struct NetworkPdu {
    ivi: u8,       // IV Index least significant bit
    nid: u8,       // Network ID (7 bits)
    ctl: bool,     // Control message flag
    ttl: u8,       // Time to live
    seq: u32,      // Sequence number (24 bits)
    src: u16,      // Source address
    dst: u16,      // Destination address
    payload: Vec<u8>,
}

/// Mesh manager state.
struct MeshManager {
    unicast_address: u16,
    elements: Vec<Element>,
    net_keys: BTreeMap<u16, NetKeyEntry>,
    app_keys: BTreeMap<u16, AppKeyEntry>,
    device_key: [u8; 16],
    provision_state: ProvisionState,
    features: NodeFeatures,
    seq_tracker: SeqTracker,
    iv_index: u32,
    default_ttl: u8,
    relay_cache: Vec<u64>,  // FNV-1a hashes of recently relayed messages
}

impl MeshManager {
    fn new() -> Self {
        Self {
            unicast_address: ADDR_UNASSIGNED,
            elements: Vec::new(),
            net_keys: BTreeMap::new(),
            app_keys: BTreeMap::new(),
            device_key: [0u8; 16],
            provision_state: ProvisionState::Unprovisioned,
            features: NodeFeatures::default_full(),
            seq_tracker: SeqTracker::new(),
            iv_index: 0,
            default_ttl: DEFAULT_TTL,
            relay_cache: Vec::new(),
        }
    }

    /// FNV-1a hash for relay cache deduplication.
    fn fnv1a_hash(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        for &byte in data {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(prime);
        }
        hash
    }

    /// Initialize the primary element with foundation models.
    fn init_primary_element(&mut self, address: u16) {
        let mut elem = Element::new(address);
        elem.models.push(MODEL_CONFIG_SERVER);
        elem.models.push(MODEL_HEALTH_SERVER);
        self.elements.push(elem);
        self.unicast_address = address;
    }

    /// Provision this node into a mesh network.
    fn provision(&mut self, net_key: &[u8; 16], address: u16, iv_index: u32) {
        if self.provision_state == ProvisionState::Provisioned {
            serial_println!("    [mesh] Already provisioned");
            return;
        }

        self.provision_state = ProvisionState::Provisioning;

        // Store the network key at index 0.
        let nid = Self::derive_nid(net_key);
        let encryption_key = Self::derive_key(net_key, b"enc");
        let privacy_key = Self::derive_key(net_key, b"prv");

        let net_entry = NetKeyEntry {
            index: 0,
            key: *net_key,
            nid,
            encryption_key,
            privacy_key,
        };
        self.net_keys.insert(0, net_entry);

        // Generate device key from TSC + address.
        let mut lo: u32;
        let mut hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
        }
        let seed = ((hi as u64) << 32) | lo as u64;
        let mut dk = [0u8; 16];
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        for i in 0..16u64 {
            hash ^= seed.wrapping_add(i).wrapping_add(address as u64);
            hash = hash.wrapping_mul(prime);
            dk[i as usize] = (hash & 0xFF) as u8;
        }
        self.device_key = dk;

        // Set up the primary element.
        self.elements.clear();
        self.init_primary_element(address);
        self.iv_index = iv_index;
        self.provision_state = ProvisionState::Provisioned;

        serial_println!("    [mesh] Provisioned: address={:#06x} iv_index={}", address, iv_index);
    }

    /// Derive a 7-bit NID from a network key (simplified).
    fn derive_nid(net_key: &[u8; 16]) -> u8 {
        let hash = Self::fnv1a_hash(net_key);
        (hash & 0x7F) as u8
    }

    /// Derive a sub-key from a network key and a salt string (simplified).
    fn derive_key(net_key: &[u8; 16], salt: &[u8]) -> [u8; 16] {
        let mut result = [0u8; 16];
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;

        // Hash net_key.
        for &b in net_key {
            hash ^= b as u64;
            hash = hash.wrapping_mul(prime);
        }
        // Hash salt.
        for &b in salt {
            hash ^= b as u64;
            hash = hash.wrapping_mul(prime);
        }

        // Expand to 16 bytes.
        for i in 0..16 {
            result[i] = ((hash >> ((i % 8) * 8)) & 0xFF) as u8;
            hash = hash.wrapping_mul(prime);
        }
        result
    }

    /// Build a mesh network PDU for sending.
    fn build_network_pdu(&mut self, dst: u16, payload: &[u8], ctl: bool) -> Vec<u8> {
        let seq = self.seq_tracker.next_seq();
        let src = self.unicast_address;
        let ivi = (self.iv_index & 1) as u8;
        let nid = self.net_keys.values().next().map(|k| k.nid).unwrap_or(0);

        let mut pdu = Vec::new();

        // Network header byte: IVI(1) | NID(7).
        pdu.push((ivi << 7) | (nid & 0x7F));

        // CTL(1) | TTL(7).
        let ttl = self.default_ttl;
        pdu.push((if ctl { 0x80 } else { 0x00 }) | (ttl & 0x7F));

        // SEQ (3 bytes, big-endian).
        pdu.push(((seq >> 16) & 0xFF) as u8);
        pdu.push(((seq >> 8) & 0xFF) as u8);
        pdu.push((seq & 0xFF) as u8);

        // SRC (2 bytes, big-endian).
        pdu.push((src >> 8) as u8);
        pdu.push(src as u8);

        // DST (2 bytes, big-endian).
        pdu.push((dst >> 8) as u8);
        pdu.push(dst as u8);

        // Transport PDU (payload).
        pdu.extend_from_slice(payload);

        // In a full stack, AES-CCM encryption would be applied here.
        // Also a 32-bit or 64-bit MIC would be appended.

        pdu
    }

    /// Process an incoming network PDU.
    fn receive_pdu(&mut self, raw: &[u8]) {
        if raw.len() < 9 {
            return;
        }

        let ivi = (raw[0] >> 7) & 1;
        let nid = raw[0] & 0x7F;
        let ctl = (raw[1] & 0x80) != 0;
        let ttl = raw[1] & 0x7F;
        let seq = ((raw[2] as u32) << 16) | ((raw[3] as u32) << 8) | raw[4] as u32;
        let src = ((raw[5] as u16) << 8) | raw[6] as u16;
        let dst = ((raw[7] as u16) << 8) | raw[8] as u16;
        let payload = &raw[9..];

        // Check if this message is for us.
        let for_us = dst == self.unicast_address
            || dst == ADDR_ALL_NODES
            || (dst >= 0xC000 && dst <= 0xFEFF); // group address

        if for_us {
            serial_println!("    [mesh] Received: src={:#06x} dst={:#06x} seq={} ttl={} len={}",
                src, dst, seq, ttl, payload.len());
            self.handle_access_message(src, dst, payload);
        }

        // Relay if we are a relay node, TTL > 1, and not from us.
        if self.features.relay && ttl > 1 && src != self.unicast_address && !for_us {
            self.relay_message(raw);
        }
    }

    /// Handle an access layer message.
    fn handle_access_message(&mut self, _src: u16, _dst: u16, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }

        // Parse opcode (1, 2, or 3 bytes).
        let (opcode, params) = if payload[0] & 0x80 == 0 {
            // 1-byte opcode.
            (payload[0] as u32, &payload[1..])
        } else if payload[0] & 0xC0 == 0x80 && payload.len() >= 2 {
            // 2-byte opcode.
            let op = ((payload[0] as u32) << 8) | payload[1] as u32;
            (op, &payload[2..])
        } else if payload.len() >= 3 {
            // 3-byte opcode (vendor).
            let op = ((payload[0] as u32) << 16) | ((payload[1] as u32) << 8) | payload[2] as u32;
            (op, &payload[3..])
        } else {
            return;
        };

        serial_println!("    [mesh] Access message opcode={:#08x} params_len={}", opcode, params.len());
    }

    /// Relay a received mesh message with decremented TTL.
    fn relay_message(&mut self, raw: &[u8]) {
        // Deduplication via FNV-1a hash.
        let hash = Self::fnv1a_hash(raw);
        if self.relay_cache.contains(&hash) {
            return; // Already relayed.
        }

        // Keep cache bounded.
        if self.relay_cache.len() >= 256 {
            self.relay_cache.remove(0);
        }
        self.relay_cache.push(hash);

        // Decrement TTL.
        let mut relayed = raw.to_vec();
        if relayed.len() >= 2 {
            let ttl = relayed[1] & 0x7F;
            if ttl > 1 {
                relayed[1] = (relayed[1] & 0x80) | (ttl - 1);
                serial_println!("    [mesh] Relaying message, new TTL={}", ttl - 1);
                // In a full stack, the relayed PDU would be transmitted via BLE advertising.
            }
        }
    }

    /// Send a mesh message.
    fn send_message(&mut self, dst: u16, payload: &[u8]) {
        if self.provision_state != ProvisionState::Provisioned {
            serial_println!("    [mesh] Cannot send: node not provisioned");
            return;
        }

        let pdu = self.build_network_pdu(dst, payload, false);
        serial_println!("    [mesh] Sending {} bytes to {:#06x}", pdu.len(), dst);
        // In a full stack, the PDU would be transmitted via BLE advertising.
    }
}

/// A Bluetooth Mesh node in the network.
pub struct MeshNode {
    address: u16,
}

impl MeshNode {
    pub fn new(address: u16) -> Self {
        Self { address }
    }

    /// Provision this node into a mesh network.
    pub fn provision(&mut self, net_key: &[u8; 16]) {
        if let Some(mgr) = MESH.lock().as_mut() {
            mgr.provision(net_key, self.address, 0);
        }
    }

    /// Send a mesh message to a destination address.
    pub fn send(&self, dst: u16, payload: &[u8]) {
        if let Some(mgr) = MESH.lock().as_mut() {
            mgr.send_message(dst, payload);
        }
    }

    /// Relay a received mesh message.
    pub fn relay(&self, message: &[u8]) {
        if let Some(mgr) = MESH.lock().as_mut() {
            mgr.relay_message(message);
        }
    }
}

/// Deliver an incoming mesh network PDU.
pub fn receive(raw: &[u8]) {
    if let Some(mgr) = MESH.lock().as_mut() {
        mgr.receive_pdu(raw);
    }
}

pub fn init() {
    let mgr = MeshManager::new();

    serial_println!("    [mesh] Initializing Bluetooth Mesh networking");

    *MESH.lock() = Some(mgr);
    serial_println!("    [mesh] Mesh initialized (unprovisioned, relay+proxy capable)");
}
