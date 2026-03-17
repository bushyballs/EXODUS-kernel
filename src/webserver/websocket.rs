/// WebSocket protocol implementation for the Genesis webserver (RFC 6455)
///
/// Provides:
///   - HTTP Upgrade handshake (with SHA-1 + base64 for Sec-WebSocket-Accept)
///   - Frame parsing and building (text, binary, close, ping, pong)
///   - Masking/unmasking per RFC 6455
///   - Connection tracking and broadcast
///   - Fragmented message reassembly
///   - Ping/pong keepalive
///
/// All code is original. Built for bare-metal Genesis kernel.

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

use super::http::{HttpRequest, HttpResponse, StatusCode};

// ============================================================================
// WebSocket opcodes (RFC 6455 Section 5.2)
// ============================================================================

/// Continuation frame (fragment)
pub const OPCODE_CONTINUATION: u8 = 0x00;
/// Text frame (UTF-8 encoded)
pub const OPCODE_TEXT: u8 = 0x01;
/// Binary frame
pub const OPCODE_BINARY: u8 = 0x02;
/// Connection close
pub const OPCODE_CLOSE: u8 = 0x08;
/// Ping
pub const OPCODE_PING: u8 = 0x09;
/// Pong
pub const OPCODE_PONG: u8 = 0x0A;

// ============================================================================
// WebSocket close status codes (RFC 6455 Section 7.4.1)
// ============================================================================

pub const CLOSE_NORMAL: u16 = 1000;
pub const CLOSE_GOING_AWAY: u16 = 1001;
pub const CLOSE_PROTOCOL_ERROR: u16 = 1002;
pub const CLOSE_UNSUPPORTED: u16 = 1003;
pub const CLOSE_NO_STATUS: u16 = 1005;
pub const CLOSE_ABNORMAL: u16 = 1006;
pub const CLOSE_INVALID_DATA: u16 = 1007;
pub const CLOSE_POLICY: u16 = 1008;
pub const CLOSE_TOO_LARGE: u16 = 1009;
pub const CLOSE_EXTENSION: u16 = 1010;
pub const CLOSE_SERVER_ERROR: u16 = 1011;

/// WebSocket magic GUID used in handshake (RFC 6455 Section 4.2.2)
const WS_MAGIC_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

// ============================================================================
// SHA-1 (minimal, only for WebSocket handshake)
// ============================================================================

/// Compute SHA-1 hash (RFC 3174). Only used for WebSocket Accept header.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    // Pre-processing: padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::from(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    // Append original length in bits as 64-bit big-endian
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit (64-byte) block
    let mut block_offset = 0;
    while block_offset < padded.len() {
        let block = &padded[block_offset..block_offset + 64];
        let mut w = [0u32; 80];

        // Prepare message schedule
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999_u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1_u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC_u32),
                _ => (b ^ c ^ d, 0xCA62C1D6_u32),
            };

            let temp = a.rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);

        block_offset += 64;
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&h0.to_be_bytes());
    result[4..8].copy_from_slice(&h1.to_be_bytes());
    result[8..12].copy_from_slice(&h2.to_be_bytes());
    result[12..16].copy_from_slice(&h3.to_be_bytes());
    result[16..20].copy_from_slice(&h4.to_be_bytes());
    result
}

// ============================================================================
// Base64 encoding (needed for WebSocket handshake)
// ============================================================================

const BASE64_CHARS: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes to base64 string
fn base64_encode(data: &[u8]) -> String {
    let mut result = Vec::with_capacity(((data.len() + 2) / 3) * 4);
    let mut i = 0;

    while i + 2 < data.len() {
        let n = ((data[i] as u32) << 16)
              | ((data[i + 1] as u32) << 8)
              | (data[i + 2] as u32);
        result.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        result.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        result.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        result.push(BASE64_CHARS[(n & 0x3F) as usize]);
        i += 3;
    }

    let remaining = data.len() - i;
    if remaining == 1 {
        let n = (data[i] as u32) << 16;
        result.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        result.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        result.push(b'=');
        result.push(b'=');
    } else if remaining == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        result.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        result.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        result.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        result.push(b'=');
    }

    String::from_utf8(result).unwrap_or_else(|_| String::new())
}

// ============================================================================
// WebSocket frame
// ============================================================================

/// A parsed WebSocket frame
#[derive(Debug, Clone)]
pub struct WsFrame {
    /// Whether this is the final frame in a message
    pub fin: bool,
    /// Frame opcode
    pub opcode: u8,
    /// Whether the payload is masked
    pub masked: bool,
    /// Masking key (4 bytes, only if masked)
    pub mask_key: [u8; 4],
    /// Payload data (unmasked)
    pub payload: Vec<u8>,
}

impl WsFrame {
    /// Parse a WebSocket frame from raw bytes.
    /// Returns (frame, bytes_consumed) or None if incomplete.
    pub fn parse(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 2 { return None; }

        let fin = (data[0] & 0x80) != 0;
        let opcode = data[0] & 0x0F;
        let masked = (data[1] & 0x80) != 0;
        let mut payload_len = (data[1] & 0x7F) as u64;
        let mut offset = 2;

        // Extended payload length
        if payload_len == 126 {
            if data.len() < 4 { return None; }
            payload_len = u16::from_be_bytes([data[2], data[3]]) as u64;
            offset = 4;
        } else if payload_len == 127 {
            if data.len() < 10 { return None; }
            payload_len = u64::from_be_bytes([
                data[2], data[3], data[4], data[5],
                data[6], data[7], data[8], data[9],
            ]);
            offset = 10;
        }

        // Masking key
        let mut mask_key = [0u8; 4];
        if masked {
            if data.len() < offset + 4 { return None; }
            mask_key.copy_from_slice(&data[offset..offset + 4]);
            offset += 4;
        }

        // Check if we have the full payload
        let total = offset + payload_len as usize;
        if data.len() < total { return None; }

        // Extract and unmask payload
        let mut payload = data[offset..total].to_vec();
        if masked {
            for i in 0..payload.len() {
                payload[i] ^= mask_key[i % 4];
            }
        }

        Some((WsFrame {
            fin,
            opcode,
            masked,
            mask_key,
            payload,
        }, total))
    }

    /// Build a WebSocket frame to send (server frames are NOT masked)
    pub fn build(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
        let mut frame = Vec::with_capacity(10 + payload.len());

        // First byte: FIN + opcode
        let byte0 = if fin { 0x80 | opcode } else { opcode };
        frame.push(byte0);

        // Second byte + extended length (no mask bit for server->client)
        if payload.len() < 126 {
            frame.push(payload.len() as u8);
        } else if payload.len() < 65536 {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }

        // Payload (unmasked for server->client)
        frame.extend_from_slice(payload);
        frame
    }

    /// Build a text frame
    pub fn text(message: &str) -> Vec<u8> {
        Self::build(OPCODE_TEXT, message.as_bytes(), true)
    }

    /// Build a binary frame
    pub fn binary(data: &[u8]) -> Vec<u8> {
        Self::build(OPCODE_BINARY, data, true)
    }

    /// Build a close frame with a status code
    pub fn close(code: u16, reason: &str) -> Vec<u8> {
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason.as_bytes());
        Self::build(OPCODE_CLOSE, &payload, true)
    }

    /// Build a ping frame
    pub fn ping(data: &[u8]) -> Vec<u8> {
        Self::build(OPCODE_PING, data, true)
    }

    /// Build a pong frame (must echo ping payload)
    pub fn pong(data: &[u8]) -> Vec<u8> {
        Self::build(OPCODE_PONG, data, true)
    }
}

// ============================================================================
// WebSocket connection state
// ============================================================================

/// State of a WebSocket connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    /// Connection is open and active
    Open,
    /// Close handshake initiated by us
    Closing,
    /// Connection fully closed
    Closed,
}

/// A tracked WebSocket connection
struct WsConnection {
    /// Connection ID (matches the HTTP connection ID)
    conn_id: u32,
    /// Current state
    state: WsState,
    /// Fragmented message buffer (for multi-frame messages)
    fragment_buf: Vec<u8>,
    /// Opcode of the fragmented message in progress
    fragment_opcode: u8,
    /// Last ping timestamp (kernel ticks)
    last_ping: u64,
    /// Last pong received timestamp
    last_pong: u64,
    /// Total messages received
    messages_received: u64,
    /// Total messages sent
    messages_sent: u64,
}

/// WebSocket message handler callback type
pub type WsMessageHandler = fn(conn_id: u32, opcode: u8, data: &[u8]) -> Option<Vec<u8>>;

/// Active WebSocket connections
static WS_CONNECTIONS: Mutex<BTreeMap<u32, WsConnection>> = Mutex::new(BTreeMap::new());

/// Registered message handler
static WS_HANDLER: Mutex<Option<WsMessageHandler>> = Mutex::new(None);

/// Ping interval in ticks
static PING_INTERVAL: Mutex<u64> = Mutex::new(30);

// ============================================================================
// Handshake
// ============================================================================

/// Check if an HTTP request is a WebSocket upgrade request
pub fn is_upgrade_request(req: &HttpRequest) -> bool {
    // Must have Upgrade: websocket header
    let has_upgrade = if let Some(upgrade) = req.headers.get("upgrade") {
        upgrade.eq_ignore_ascii_case("websocket")
    } else {
        false
    };

    // Must have Connection: Upgrade header
    let has_connection = if let Some(conn) = req.headers.get("connection") {
        let lower = conn.to_ascii_lowercase();
        lower.contains("upgrade")
    } else {
        false
    };

    // Must have Sec-WebSocket-Key
    let has_key = req.headers.contains("sec-websocket-key");

    // Must have Sec-WebSocket-Version: 13
    let correct_version = if let Some(ver) = req.headers.get("sec-websocket-version") {
        ver == "13"
    } else {
        false
    };

    has_upgrade && has_connection && has_key && correct_version
}

/// Handle a WebSocket upgrade request, returning the 101 Switching Protocols response
pub fn handle_upgrade(req: &HttpRequest, conn_id: u32) -> HttpResponse {
    let ws_key = match req.headers.get("sec-websocket-key") {
        Some(key) => key,
        None => return HttpResponse::text(StatusCode::BadRequest, "Missing Sec-WebSocket-Key"),
    };

    // Compute Sec-WebSocket-Accept: base64(sha1(key + GUID))
    let mut accept_input = Vec::with_capacity(ws_key.len() + WS_MAGIC_GUID.len());
    accept_input.extend_from_slice(ws_key.as_bytes());
    accept_input.extend_from_slice(WS_MAGIC_GUID);
    let hash = sha1(&accept_input);
    let accept_value = base64_encode(&hash);

    // Register the WebSocket connection
    let ws_conn = WsConnection {
        conn_id,
        state: WsState::Open,
        fragment_buf: Vec::new(),
        fragment_opcode: 0,
        last_ping: 0,
        last_pong: 0,
        messages_received: 0,
        messages_sent: 0,
    };
    WS_CONNECTIONS.lock().insert(conn_id, ws_conn);

    // Build 101 Switching Protocols response
    let mut resp = HttpResponse::new(StatusCode::SwitchingProtocols);
    resp.headers.set("upgrade", "websocket");
    resp.headers.set("connection", "Upgrade");
    resp.headers.set("sec-websocket-accept", &accept_value);

    // Check for requested subprotocol
    if let Some(protocols) = req.headers.get("sec-websocket-protocol") {
        // Accept the first requested protocol
        if let Some(first) = protocols.split(',').next() {
            resp.headers.set("sec-websocket-protocol", first.trim());
        }
    }

    serial_println!("  [ws] WebSocket upgrade for connection {}", conn_id);
    resp
}

// ============================================================================
// Frame processing
// ============================================================================

/// Process raw data received on a WebSocket connection.
/// Returns response frames to send back (if any).
pub fn process_frame_data(data: &[u8], conn_id: u32) -> Option<Vec<u8>> {
    let (frame, _consumed) = WsFrame::parse(data)?;

    let mut conns = WS_CONNECTIONS.lock();
    let ws_conn = conns.get_mut(&conn_id)?;

    if ws_conn.state == WsState::Closed {
        drop(conns);
        return None;
    }

    ws_conn.messages_received = ws_conn.messages_received.saturating_add(1);

    match frame.opcode {
        OPCODE_TEXT | OPCODE_BINARY => {
            if frame.fin {
                // Complete single-frame message
                let opcode = frame.opcode;
                let payload = frame.payload.clone();
                drop(conns);
                handle_message(conn_id, opcode, &payload)
            } else {
                // Start of fragmented message
                ws_conn.fragment_opcode = frame.opcode;
                ws_conn.fragment_buf = frame.payload;
                drop(conns);
                None
            }
        }
        OPCODE_CONTINUATION => {
            ws_conn.fragment_buf.extend_from_slice(&frame.payload);
            if frame.fin {
                // Final fragment — reassemble and deliver
                let opcode = ws_conn.fragment_opcode;
                let payload = ws_conn.fragment_buf.clone();
                ws_conn.fragment_buf.clear();
                ws_conn.fragment_opcode = 0;
                drop(conns);
                handle_message(conn_id, opcode, &payload)
            } else {
                drop(conns);
                None
            }
        }
        OPCODE_PING => {
            // Must respond with pong echoing the payload
            let pong = WsFrame::pong(&frame.payload);
            drop(conns);
            serial_println!("  [ws] ping from connection {}, sending pong", conn_id);
            Some(pong)
        }
        OPCODE_PONG => {
            // Update pong timestamp
            ws_conn.last_pong = ws_conn.last_pong.wrapping_add(1);
            drop(conns);
            None
        }
        OPCODE_CLOSE => {
            // Parse close code if present
            let close_code = if frame.payload.len() >= 2 {
                u16::from_be_bytes([frame.payload[0], frame.payload[1]])
            } else {
                CLOSE_NO_STATUS
            };

            serial_println!("  [ws] close from connection {} (code: {})", conn_id, close_code);

            if ws_conn.state == WsState::Open {
                // Respond with close frame
                ws_conn.state = WsState::Closed;
                drop(conns);
                Some(WsFrame::close(CLOSE_NORMAL, "goodbye"))
            } else {
                ws_conn.state = WsState::Closed;
                drop(conns);
                None
            }
        }
        _ => {
            // Unknown opcode — protocol error
            drop(conns);
            serial_println!("  [ws] unknown opcode 0x{:02X} from connection {}", frame.opcode, conn_id);
            Some(WsFrame::close(CLOSE_PROTOCOL_ERROR, "unknown opcode"))
        }
    }
}

/// Dispatch a complete WebSocket message to the registered handler
fn handle_message(conn_id: u32, opcode: u8, data: &[u8]) -> Option<Vec<u8>> {
    let handler = WS_HANDLER.lock();
    if let Some(handler_fn) = *handler {
        drop(handler);
        handler_fn(conn_id, opcode, data)
    } else {
        drop(handler);
        // Default echo behavior
        serial_println!("  [ws] echo {} bytes to connection {}", data.len(), conn_id);
        Some(WsFrame::build(opcode, data, true))
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Register a WebSocket message handler
pub fn set_message_handler(handler: WsMessageHandler) {
    *WS_HANDLER.lock() = Some(handler);
}

/// Send a text message to a specific WebSocket connection
pub fn send_text(conn_id: u32, message: &str) -> Option<Vec<u8>> {
    let mut conns = WS_CONNECTIONS.lock();
    if let Some(ws) = conns.get_mut(&conn_id) {
        if ws.state == WsState::Open {
            ws.messages_sent = ws.messages_sent.saturating_add(1);
            drop(conns);
            return Some(WsFrame::text(message));
        }
    }
    None
}

/// Send a binary message to a specific WebSocket connection
pub fn send_binary(conn_id: u32, data: &[u8]) -> Option<Vec<u8>> {
    let mut conns = WS_CONNECTIONS.lock();
    if let Some(ws) = conns.get_mut(&conn_id) {
        if ws.state == WsState::Open {
            ws.messages_sent = ws.messages_sent.saturating_add(1);
            drop(conns);
            return Some(WsFrame::binary(data));
        }
    }
    None
}

/// Build broadcast frames for all open WebSocket connections
pub fn broadcast_text(message: &str) -> Vec<(u32, Vec<u8>)> {
    let frame_data = WsFrame::text(message);
    let conns = WS_CONNECTIONS.lock();
    let mut result = Vec::new();
    for (id, ws) in conns.iter() {
        if ws.state == WsState::Open {
            result.push((*id, frame_data.clone()));
        }
    }
    result
}

/// Close a WebSocket connection
pub fn close_ws(conn_id: u32, code: u16, reason: &str) -> Option<Vec<u8>> {
    let mut conns = WS_CONNECTIONS.lock();
    if let Some(ws) = conns.get_mut(&conn_id) {
        ws.state = WsState::Closing;
        drop(conns);
        Some(WsFrame::close(code, reason))
    } else {
        None
    }
}

/// Remove a closed WebSocket connection from tracking
pub fn remove_connection(conn_id: u32) {
    WS_CONNECTIONS.lock().remove(&conn_id);
}

/// Get the number of active WebSocket connections
pub fn active_count() -> usize {
    WS_CONNECTIONS.lock().iter()
        .filter(|(_, ws)| ws.state == WsState::Open)
        .count()
}

/// Generate ping frames for all connections that need keepalive
pub fn generate_pings(current_tick: u64) -> Vec<(u32, Vec<u8>)> {
    let interval = *PING_INTERVAL.lock();
    let mut pings = Vec::new();
    let mut conns = WS_CONNECTIONS.lock();

    for (id, ws) in conns.iter_mut() {
        if ws.state == WsState::Open
            && current_tick.saturating_sub(ws.last_ping) >= interval
        {
            ws.last_ping = current_tick;
            let ping_payload = current_tick.to_be_bytes();
            pings.push((*id, WsFrame::ping(&ping_payload)));
        }
    }

    pings
}

/// Initialize the WebSocket subsystem
pub fn init() {
    serial_println!("  [ws] WebSocket protocol handler initialized (RFC 6455)");
    serial_println!("  [ws] SHA-1 handshake, frame parse/build, ping/pong, fragmentation");
}
