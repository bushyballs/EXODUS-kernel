use crate::sync::Mutex;
/// Traffic Obfuscation / Camouflage for Genesis
///
/// Defeats deep packet inspection (DPI) and censorship by disguising
/// encrypted tunnel traffic as ordinary protocol traffic. Implements
/// multiple obfuscation methods that can be selected or rotated
/// based on detected censorship conditions.
///
/// Methods:
///   - ScrambleSuit: randomized handshake + uniform traffic patterns
///   - Obfs4: ntor-based handshake, probabilistic padding
///   - Meek: tunnel traffic inside HTTPS to CDN fronts
///   - Snowflake: WebRTC-based ephemeral bridges via browser proxies
///   - DPI Resist: active probing defense + protocol fingerprint removal
///   - Domain Fronting: route through CDN with mismatched SNI/Host
///
/// All crypto is simulated via hash-based transforms (no f32/f64).
/// Throughput measured in bytes; ratios use Q16 fixed-point.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum packet size before fragmentation
const MAX_PACKET_SIZE: usize = 1500;

/// Minimum padded packet size (to obscure real payload length)
const MIN_PADDED_SIZE: usize = 512;

/// Hash seed for obfuscation key derivation
const OBFS_KEY_SEED: u64 = 0x0BF5CA7ED5ECE7ED;

/// Hash seed for TLS record mimicry
const TLS_MIMIC_SEED: u64 = 0x71530E51CA7E0BF5;

/// Hash seed for HTTP mimicry
const HTTP_MIMIC_SEED: u64 = 0xAE77FACE0DECABED;

/// Hash seed for censorship detection
const CENSOR_DETECT_SEED: u64 = 0xCE550EDFACE0BEAD;

/// Bridge rotation interval in ticks
const BRIDGE_ROTATION_INTERVAL: u64 = 300;

/// Q16 fixed-point multiplier (1.0 = 65536)
const Q16_ONE: i32 = 65536;

/// TLS record type: Application Data
const TLS_APP_DATA: u8 = 0x17;

/// TLS version bytes: TLS 1.2
const TLS_VERSION_MAJOR: u8 = 0x03;
const TLS_VERSION_MINOR: u8 = 0x03;

/// HTTP/1.1 response header signature bytes
const HTTP_RESPONSE_SIG: [u8; 4] = [0x48, 0x54, 0x54, 0x50]; // "HTTP"

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Obfuscation method for traffic camouflage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObfsMethod {
    /// Randomized handshake + uniform traffic shaping
    ScrambleSuit,
    /// ntor handshake with probabilistic padding
    Obfs4,
    /// Tunnel inside HTTPS requests to CDN domains
    Meek,
    /// WebRTC ephemeral bridges via volunteer browsers
    Snowflake,
    /// Active probing defense + fingerprint scrubbing
    DpiResist,
    /// CDN routing with mismatched SNI and Host header
    DomainFront,
}

/// Censorship detection result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CensorshipType {
    /// No censorship detected
    None,
    /// IP-based blocking
    IpBlock,
    /// DNS poisoning / hijacking
    DnsPoisoning,
    /// Deep packet inspection filtering
    Dpi,
    /// TLS fingerprinting
    TlsFingerprint,
    /// Protocol-specific blocking (e.g., Tor, VPN)
    ProtocolBlock,
    /// Active probing by censor
    ActiveProbing,
    /// Throttling / bandwidth limitation
    Throttling,
}

/// Configuration for a specific obfuscation setup
#[derive(Debug, Clone)]
pub struct ObfsConfig {
    /// Active obfuscation method
    pub method: ObfsMethod,
    /// Hash of the bridge address
    pub bridge_hash: u64,
    /// Encryption key hash for this configuration
    pub key_hash: u64,
    /// Hash of the backend server address
    pub server_hash: u64,
    /// Whether this configuration is active
    active: bool,
    /// Bytes processed with this configuration
    bytes_processed: u64,
    /// Tick when this configuration was activated
    activated_tick: u64,
    /// CDN front domain hash (for Meek/DomainFront)
    cdn_front_hash: u64,
    /// Padding ratio (Q16 fixed-point: 65536 = 1.0x, 131072 = 2.0x)
    padding_ratio_q16: i32,
}

/// Throughput statistics
#[derive(Debug, Clone)]
pub struct ThroughputStats {
    /// Raw payload bytes sent
    pub payload_bytes: u64,
    /// Total wire bytes (including padding and headers)
    pub wire_bytes: u64,
    /// Overhead ratio in Q16 fixed-point (65536 = 1.0x = no overhead)
    pub overhead_q16: i32,
    /// Current obfuscation method
    pub method: ObfsMethod,
    /// Bridge rotations performed
    pub rotations: u32,
}

/// A wrapped (obfuscated) packet ready for transmission
#[derive(Debug, Clone)]
pub struct ObfuscatedPacket {
    /// The obfuscated payload
    pub data: Vec<u8>,
    /// Obfuscation method used
    pub method: ObfsMethod,
    /// Original payload size before obfuscation
    pub original_size: usize,
    /// Wire size after obfuscation (including padding)
    pub wire_size: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Active obfuscation configuration
static CONFIG: Mutex<Option<ObfsConfig>> = Mutex::new(None);

/// Total payload bytes processed
static PAYLOAD_BYTES: Mutex<u64> = Mutex::new(0);

/// Total wire bytes (payload + overhead)
static WIRE_BYTES: Mutex<u64> = Mutex::new(0);

/// Bridge rotation count
static ROTATION_COUNT: Mutex<u32> = Mutex::new(0);

/// Simulated tick counter
static TICK: Mutex<u64> = Mutex::new(0);

/// Known bridge list (hashes)
static BRIDGES: Mutex<Vec<u64>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Hash function for obfuscation operations
fn obfs_hash(data: &[u8], seed: u64) -> u64 {
    let mut h: u64 = seed;
    for &b in data {
        h = h.wrapping_mul(0x00A4D63463A3FDEB).wrapping_add(b as u64);
        h ^= h >> 31;
        h = h.wrapping_mul(0x0CF019D385EA7B75);
    }
    h
}

/// Apply scramblesuit-style obfuscation: XOR with key stream + uniform padding
fn scramblesuit_wrap(data: &mut Vec<u8>, key_hash: u64) {
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] ^= key_bytes[i % 8];
        data[i] = data[i].wrapping_add(key_bytes[(i + 5) % 8]);
    }
    // Pad to uniform size to defeat length-based analysis
    while data.len() < MIN_PADDED_SIZE {
        let pad_byte = key_bytes[data.len() % 8];
        data.push(pad_byte);
    }
}

/// Remove scramblesuit obfuscation
fn scramblesuit_unwrap(data: &mut Vec<u8>, key_hash: u64, original_len: usize) {
    // Remove padding
    data.truncate(original_len);
    let key_bytes = key_hash.to_le_bytes();
    for i in 0..data.len() {
        data[i] = data[i].wrapping_sub(key_bytes[(i + 5) % 8]);
        data[i] ^= key_bytes[i % 8];
    }
}

/// Apply obfs4-style obfuscation with ntor-derived key stream
fn obfs4_wrap(data: &mut Vec<u8>, key_hash: u64) {
    // ntor-like key derivation
    let ntor_key = obfs_hash(&key_hash.to_le_bytes(), OBFS_KEY_SEED);
    let ntor_bytes = ntor_key.to_le_bytes();
    for i in 0..data.len() {
        let stream_byte = ntor_bytes[i % 8] ^ ((i as u8).wrapping_mul(0xAB));
        data[i] ^= stream_byte;
    }
    // Probabilistic padding: add random-looking bytes
    let pad_amount = (ntor_key as usize % 64).wrapping_add(16);
    for j in 0..pad_amount {
        let pad = ntor_bytes[j % 8].wrapping_add(j as u8);
        data.push(pad);
    }
}

/// Apply meek-style wrapping: disguise as HTTPS traffic to CDN
fn meek_wrap(data: &mut Vec<u8>, cdn_front_hash: u64) {
    let front_bytes = cdn_front_hash.to_le_bytes();
    // Prepend HTTP-like header bytes
    let mut wrapped = Vec::new();
    // POST-like request preamble
    wrapped.extend_from_slice(&[0x50, 0x4F, 0x53, 0x54, 0x20, 0x2F]); // "POST /"
                                                                      // Add CDN-derived path bytes
    for i in 0..8 {
        wrapped.push(0x61 + (front_bytes[i] % 26)); // a-z
    }
    wrapped.extend_from_slice(&[0x20, 0x48, 0x54, 0x54, 0x50]); // " HTTP"
    wrapped.push(0x0D);
    wrapped.push(0x0A);
    // Content-Length-like header
    let len_val = data.len() as u16;
    wrapped.extend_from_slice(&len_val.to_be_bytes());
    wrapped.push(0x0D);
    wrapped.push(0x0A);
    wrapped.push(0x0D);
    wrapped.push(0x0A);
    // Encrypted payload
    for i in 0..data.len() {
        wrapped.push(data[i] ^ front_bytes[i % 8]);
    }
    *data = wrapped;
}

/// Get current tick
fn current_tick() -> u64 {
    let t = TICK.lock();
    *t
}

/// Method name for logging
fn method_name(m: ObfsMethod) -> &'static str {
    match m {
        ObfsMethod::ScrambleSuit => "ScrambleSuit",
        ObfsMethod::Obfs4 => "obfs4",
        ObfsMethod::Meek => "meek",
        ObfsMethod::Snowflake => "Snowflake",
        ObfsMethod::DpiResist => "DPI-Resist",
        ObfsMethod::DomainFront => "DomainFront",
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Wrap (obfuscate) outgoing traffic using the active obfuscation method.
/// Returns an obfuscated packet ready for transmission.
pub fn wrap_traffic(payload: &[u8]) -> Option<ObfuscatedPacket> {
    let config_guard = CONFIG.lock();
    let config = match config_guard.as_ref() {
        Some(c) if c.active => c,
        _ => {
            serial_println!("  Obfs: no active configuration");
            return None;
        }
    };

    let method = config.method;
    let key = config.key_hash;
    let cdn_hash = config.cdn_front_hash;
    let original_size = payload.len();

    let mut data = Vec::from(payload);

    match method {
        ObfsMethod::ScrambleSuit => scramblesuit_wrap(&mut data, key),
        ObfsMethod::Obfs4 => obfs4_wrap(&mut data, key),
        ObfsMethod::Meek | ObfsMethod::DomainFront => meek_wrap(&mut data, cdn_hash),
        ObfsMethod::Snowflake => {
            // Snowflake: WebRTC-style framing
            let frame_key = obfs_hash(&key.to_le_bytes(), 0x5A0FACE5ABCDEF01);
            let frame_bytes = frame_key.to_le_bytes();
            for i in 0..data.len() {
                data[i] ^= frame_bytes[i % 8];
            }
            // Add WebRTC-like framing header
            let mut framed = vec![0x80, 0x60]; // RTP-like header
            framed.extend_from_slice(&(data.len() as u16).to_be_bytes());
            framed.extend_from_slice(&data);
            data = framed;
        }
        ObfsMethod::DpiResist => {
            // DPI resist: fragment + randomize + scrub fingerprints
            let scrub_key = obfs_hash(&key.to_le_bytes(), CENSOR_DETECT_SEED);
            let scrub_bytes = scrub_key.to_le_bytes();
            for i in 0..data.len() {
                data[i] ^= scrub_bytes[i % 8];
                data[i] = data[i].rotate_left((i as u32) % 7 + 1);
            }
            // Pad to defeat length correlation
            while data.len() < MIN_PADDED_SIZE {
                data.push(scrub_bytes[data.len() % 8]);
            }
        }
    }

    let wire_size = data.len();
    drop(config_guard);

    // Update statistics
    let mut pb = PAYLOAD_BYTES.lock();
    *pb = pb.wrapping_add(original_size as u64);
    drop(pb);
    let mut wb = WIRE_BYTES.lock();
    *wb = wb.wrapping_add(wire_size as u64);

    Some(ObfuscatedPacket {
        data,
        method,
        original_size,
        wire_size,
    })
}

/// Unwrap (deobfuscate) incoming traffic, recovering the original payload.
/// Returns the deobfuscated payload bytes.
pub fn unwrap_traffic(packet: &ObfuscatedPacket) -> Vec<u8> {
    let config_guard = CONFIG.lock();
    let config = match config_guard.as_ref() {
        Some(c) => c,
        None => return Vec::new(),
    };

    let key = config.key_hash;
    let mut data = packet.data.clone();

    match packet.method {
        ObfsMethod::ScrambleSuit => {
            scramblesuit_unwrap(&mut data, key, packet.original_size);
        }
        ObfsMethod::Obfs4 => {
            // Strip probabilistic padding first
            data.truncate(packet.original_size);
            let ntor_key = obfs_hash(&key.to_le_bytes(), OBFS_KEY_SEED);
            let ntor_bytes = ntor_key.to_le_bytes();
            for i in 0..data.len() {
                let stream_byte = ntor_bytes[i % 8] ^ ((i as u8).wrapping_mul(0xAB));
                data[i] ^= stream_byte;
            }
        }
        ObfsMethod::Meek | ObfsMethod::DomainFront => {
            // Find payload after HTTP headers (after \r\n\r\n pattern)
            let cdn_hash = config.cdn_front_hash;
            let front_bytes = cdn_hash.to_le_bytes();
            // Skip header (find double CRLF)
            let header_end = data
                .windows(2)
                .position(|w| w[0] == 0x0D && w[1] == 0x0A)
                .map(|p| p + 2)
                .unwrap_or(0);
            // Skip content-length + CRLF
            let payload_start = if header_end + 4 < data.len() {
                header_end + 4
            } else {
                header_end
            };
            let encrypted = &data[payload_start..];
            let mut decrypted = Vec::new();
            for (i, &b) in encrypted.iter().enumerate() {
                decrypted.push(b ^ front_bytes[i % 8]);
            }
            data = decrypted;
        }
        ObfsMethod::Snowflake => {
            // Strip RTP-like header (4 bytes)
            if data.len() > 4 {
                data = data[4..].to_vec();
            }
            let frame_key = obfs_hash(&key.to_le_bytes(), 0x5A0FACE5ABCDEF01);
            let frame_bytes = frame_key.to_le_bytes();
            for i in 0..data.len() {
                data[i] ^= frame_bytes[i % 8];
            }
        }
        ObfsMethod::DpiResist => {
            data.truncate(packet.original_size);
            let scrub_key = obfs_hash(&key.to_le_bytes(), CENSOR_DETECT_SEED);
            let scrub_bytes = scrub_key.to_le_bytes();
            for i in 0..data.len() {
                data[i] = data[i].rotate_right((i as u32) % 7 + 1);
                data[i] ^= scrub_bytes[i % 8];
            }
        }
    }

    data
}

/// Detect censorship conditions on the network.
/// Analyzes probe results to determine the type of censorship present.
pub fn detect_censorship(probe_hash: u64) -> CensorshipType {
    // Simulate censorship detection by analyzing probe characteristics
    let mut probe_buf = vec![0u8; 16];
    probe_buf[0..8].copy_from_slice(&probe_hash.to_le_bytes());
    probe_buf[8..16].copy_from_slice(&current_tick().to_le_bytes());
    let analysis = obfs_hash(&probe_buf, CENSOR_DETECT_SEED);

    let result = match analysis & 0x07 {
        0 => CensorshipType::None,
        1 => CensorshipType::IpBlock,
        2 => CensorshipType::DnsPoisoning,
        3 => CensorshipType::Dpi,
        4 => CensorshipType::TlsFingerprint,
        5 => CensorshipType::ProtocolBlock,
        6 => CensorshipType::ActiveProbing,
        _ => CensorshipType::Throttling,
    };

    let type_str = match result {
        CensorshipType::None => "none",
        CensorshipType::IpBlock => "IP block",
        CensorshipType::DnsPoisoning => "DNS poisoning",
        CensorshipType::Dpi => "DPI",
        CensorshipType::TlsFingerprint => "TLS fingerprint",
        CensorshipType::ProtocolBlock => "protocol block",
        CensorshipType::ActiveProbing => "active probing",
        CensorshipType::Throttling => "throttling",
    };

    serial_println!(
        "  Obfs: censorship probe result: {} (analysis={:#018X})",
        type_str,
        analysis
    );
    result
}

/// Select a bridge from the known bridge list.
/// Chooses based on availability and diversity from current bridge.
pub fn select_bridge() -> Option<u64> {
    let bridges = BRIDGES.lock();
    if bridges.is_empty() {
        serial_println!("  Obfs: no bridges available");
        return None;
    }

    let config_guard = CONFIG.lock();
    let current_bridge = config_guard.as_ref().map(|c| c.bridge_hash).unwrap_or(0);
    drop(config_guard);

    // Pick a different bridge than the current one
    let selected = bridges
        .iter()
        .find(|&&b| b != current_bridge)
        .or_else(|| bridges.first())
        .copied();

    if let Some(bridge) = selected {
        serial_println!("  Obfs: bridge selected: {:#018X}", bridge);
    }

    selected
}

/// Rotate the obfuscation method, switching to a different technique.
/// Automatically selects based on detected censorship type.
pub fn rotate_method(censorship: CensorshipType) -> ObfsMethod {
    let new_method = match censorship {
        CensorshipType::None => ObfsMethod::Obfs4,
        CensorshipType::IpBlock => ObfsMethod::Snowflake,
        CensorshipType::DnsPoisoning => ObfsMethod::DomainFront,
        CensorshipType::Dpi => ObfsMethod::DpiResist,
        CensorshipType::TlsFingerprint => ObfsMethod::ScrambleSuit,
        CensorshipType::ProtocolBlock => ObfsMethod::Meek,
        CensorshipType::ActiveProbing => ObfsMethod::DpiResist,
        CensorshipType::Throttling => ObfsMethod::Obfs4,
    };

    let mut config_guard = CONFIG.lock();
    if let Some(config) = config_guard.as_mut() {
        let old_method = config.method;
        config.method = new_method;
        // Re-derive key for new method
        let mut key_buf = vec![0u8; 16];
        key_buf[0..8].copy_from_slice(&config.bridge_hash.to_le_bytes());
        key_buf[8..16].copy_from_slice(&(new_method as u64).to_le_bytes());
        config.key_hash = obfs_hash(&key_buf, OBFS_KEY_SEED);

        serial_println!(
            "  Obfs: method rotated from {} to {} (key={:#018X})",
            method_name(old_method),
            method_name(new_method),
            config.key_hash
        );
    }

    let mut rot = ROTATION_COUNT.lock();
    *rot = rot.wrapping_add(1);

    new_method
}

/// Pad a packet to a target size to resist traffic analysis.
/// Uses deterministic padding derived from the key to allow stripping.
/// Target size is calculated to obscure the real payload length.
pub fn pad_packet(data: &[u8]) -> Vec<u8> {
    let config_guard = CONFIG.lock();
    let key = config_guard.as_ref().map(|c| c.key_hash).unwrap_or(0);
    let ratio = config_guard
        .as_ref()
        .map(|c| c.padding_ratio_q16)
        .unwrap_or(Q16_ONE);
    drop(config_guard);

    // Calculate target size using Q16 ratio
    let target_size_raw = ((data.len() as i64) * (ratio as i64)) >> 16;
    let target_size = (target_size_raw as usize)
        .max(MIN_PADDED_SIZE)
        .min(MAX_PACKET_SIZE);

    let mut padded = Vec::from(data);
    let key_bytes = key.to_le_bytes();

    while padded.len() < target_size {
        let idx = padded.len();
        let pad_byte = key_bytes[idx % 8] ^ (idx as u8);
        padded.push(pad_byte);
    }

    padded
}

/// Mimic a TLS 1.2 Application Data record.
/// Wraps the payload in a TLS record header to look like HTTPS traffic.
pub fn mimic_tls(payload: &[u8]) -> Vec<u8> {
    let config_guard = CONFIG.lock();
    let key = config_guard.as_ref().map(|c| c.key_hash).unwrap_or(0);
    drop(config_guard);

    let mut record = Vec::new();

    // TLS record header (5 bytes)
    record.push(TLS_APP_DATA); // Content type: Application Data
    record.push(TLS_VERSION_MAJOR); // Version major: 3
    record.push(TLS_VERSION_MINOR); // Version minor: 3 (TLS 1.2)
    let length = payload.len() as u16;
    record.extend_from_slice(&length.to_be_bytes()); // Record length

    // Encrypt payload with TLS-mimicry key
    let tls_key = obfs_hash(&key.to_le_bytes(), TLS_MIMIC_SEED);
    let tls_bytes = tls_key.to_le_bytes();
    for (i, &b) in payload.iter().enumerate() {
        record.push(b ^ tls_bytes[i % 8]);
    }

    serial_println!(
        "  Obfs: TLS mimic record ({} bytes, type=0x{:02X})",
        record.len(),
        TLS_APP_DATA
    );
    record
}

/// Mimic an HTTP/1.1 response.
/// Wraps the payload to look like a chunked HTTP response body.
pub fn mimic_http(payload: &[u8]) -> Vec<u8> {
    let config_guard = CONFIG.lock();
    let key = config_guard.as_ref().map(|c| c.key_hash).unwrap_or(0);
    drop(config_guard);

    let mut response = Vec::new();

    // HTTP/1.1 200 OK response header
    response.extend_from_slice(&HTTP_RESPONSE_SIG); // "HTTP"
    response.extend_from_slice(&[0x2F, 0x31, 0x2E, 0x31]); // "/1.1"
    response.extend_from_slice(&[0x20, 0x32, 0x30, 0x30]); // " 200"
    response.push(0x0D);
    response.push(0x0A); // \r\n

    // Content-Type header
    response.extend_from_slice(&[0x43, 0x6F, 0x6E, 0x74, 0x65, 0x6E, 0x74]); // "Content"
    response.extend_from_slice(&[0x2D, 0x54, 0x79, 0x70, 0x65, 0x3A, 0x20]); // "-Type: "
    response.extend_from_slice(&[0x74, 0x65, 0x78, 0x74]); // "text"
    response.push(0x0D);
    response.push(0x0A);

    // Content-Length header
    response.extend_from_slice(&[0x43, 0x6F, 0x6E, 0x74, 0x65, 0x6E, 0x74]); // "Content"
    response.extend_from_slice(&[0x2D, 0x4C, 0x65, 0x6E]); // "-Len"
    response.extend_from_slice(&[0x67, 0x74, 0x68, 0x3A, 0x20]); // "gth: "
                                                                 // Encode length as ASCII digits
    let len_str = payload.len();
    let mut digits = Vec::new();
    let mut val = len_str;
    if val == 0 {
        digits.push(0x30); // "0"
    } else {
        while val > 0 {
            digits.push(0x30 + (val % 10) as u8);
            val /= 10;
        }
        digits.reverse();
    }
    response.extend_from_slice(&digits);
    response.push(0x0D);
    response.push(0x0A);

    // End of headers
    response.push(0x0D);
    response.push(0x0A);

    // Encrypted body
    let http_key = obfs_hash(&key.to_le_bytes(), HTTP_MIMIC_SEED);
    let http_bytes = http_key.to_le_bytes();
    for (i, &b) in payload.iter().enumerate() {
        response.push(b ^ http_bytes[i % 8]);
    }

    serial_println!(
        "  Obfs: HTTP mimic response ({} bytes total, {} body)",
        response.len(),
        payload.len()
    );
    response
}

/// Get throughput statistics for the obfuscation layer.
pub fn get_throughput() -> ThroughputStats {
    let pb = *PAYLOAD_BYTES.lock();
    let wb = *WIRE_BYTES.lock();
    let rot = *ROTATION_COUNT.lock();

    let config_guard = CONFIG.lock();
    let method = config_guard
        .as_ref()
        .map(|c| c.method)
        .unwrap_or(ObfsMethod::Obfs4);
    drop(config_guard);

    // Calculate overhead ratio in Q16 fixed-point
    let overhead_q16: i32 = if pb > 0 {
        ((wb as i64 * Q16_ONE as i64) / pb as i64) as i32
    } else {
        Q16_ONE
    };

    ThroughputStats {
        payload_bytes: pb,
        wire_bytes: wb,
        overhead_q16,
        method,
        rotations: rot,
    }
}

/// Seed the bridge list with known bridge addresses
fn seed_bridges() {
    let mut bridges = BRIDGES.lock();
    let known_bridges: Vec<u64> = vec![
        0xBE1D6E0A1B2C3D4E,
        0xBE1D6E5F6A7B8C9D,
        0xBE1D6EAEBFCA0B1C,
        0xBE1D6E2D3E4F5A6B,
        0xBE1D6E7C8D9EAFB0,
        0xBE1D6E1F2A3B4C5D,
    ];

    for bridge in known_bridges {
        bridges.push(bridge);
    }
}

/// Initialize the traffic obfuscation subsystem
pub fn init() {
    seed_bridges();

    // Set up default configuration with obfs4
    let default_bridge = {
        let bridges = BRIDGES.lock();
        bridges.first().copied().unwrap_or(0)
    };

    let key_hash = obfs_hash(&default_bridge.to_le_bytes(), OBFS_KEY_SEED);
    let server_hash = obfs_hash(&key_hash.to_le_bytes(), 0x5E12FE12CA5CADED);
    let cdn_hash = obfs_hash(&server_hash.to_le_bytes(), HTTP_MIMIC_SEED);

    let config = ObfsConfig {
        method: ObfsMethod::Obfs4,
        bridge_hash: default_bridge,
        key_hash,
        server_hash,
        active: true,
        bytes_processed: 0,
        activated_tick: 0,
        cdn_front_hash: cdn_hash,
        padding_ratio_q16: 98304, // 1.5x padding ratio in Q16
    };

    let mut config_guard = CONFIG.lock();
    *config_guard = Some(config);
    drop(config_guard);

    let bridge_count = {
        let bridges = BRIDGES.lock();
        bridges.len()
    };

    serial_println!(
        "  Obfs: traffic obfuscation initialized (obfs4, {} bridges)",
        bridge_count
    );
}
