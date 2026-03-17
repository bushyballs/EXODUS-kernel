/// WebSocket handshake protocol (RFC 6455 Section 4)
///
/// Implements the HTTP Upgrade negotiation for WebSocket connections:
/// Sec-WebSocket-Key generation, SHA-1 based accept computation,
/// header parsing/generation, and version negotiation.
///
/// Inspired by: RFC 6455 Section 4, RFC 7230. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// WebSocket magic GUID for Sec-WebSocket-Accept computation
const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-5AB5DC11D455";

/// Supported WebSocket protocol version
const WS_VERSION: u8 = 13;

/// Maximum header line length
const MAX_HEADER_LINE: usize = 8192;

/// Maximum total headers
const MAX_HEADERS: usize = 64;

// ---------------------------------------------------------------------------
// Base64 encoder (minimal, for Sec-WebSocket-Key)
// ---------------------------------------------------------------------------

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes to base64
fn base64_encode(input: &[u8]) -> String {
    let mut output = Vec::new();
    let mut i = 0;
    while i + 2 < input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        output.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        output.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        output.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        output.push(BASE64_CHARS[(n & 0x3F) as usize]);
        i = i.saturating_add(3);
    }
    let remaining = input.len() - i;
    if remaining == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        output.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        output.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        output.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        output.push(b'=');
    } else if remaining == 1 {
        let n = (input[i] as u32) << 16;
        output.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        output.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        output.push(b'=');
        output.push(b'=');
    }
    String::from_utf8(output).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// SHA-1 (minimal implementation for Sec-WebSocket-Accept)
// ---------------------------------------------------------------------------

/// Compute SHA-1 hash (RFC 3174)
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    // Pre-processing: add padding
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit (64-byte) chunk
    for chunk_start in (0..padded.len()).step_by(64) {
        let chunk = &padded[chunk_start..chunk_start + 64];
        let mut w = [0u32; 80];

        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
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
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
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
    }

    let mut result = [0u8; 20];
    result[0..4].copy_from_slice(&h0.to_be_bytes());
    result[4..8].copy_from_slice(&h1.to_be_bytes());
    result[8..12].copy_from_slice(&h2.to_be_bytes());
    result[12..16].copy_from_slice(&h3.to_be_bytes());
    result[16..20].copy_from_slice(&h4.to_be_bytes());
    result
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

/// Compute Sec-WebSocket-Accept from Sec-WebSocket-Key
pub fn compute_accept_key(client_key: &str) -> String {
    let mut concat = Vec::new();
    concat.extend_from_slice(client_key.trim().as_bytes());
    concat.extend_from_slice(WS_GUID);
    let hash = sha1(&concat);
    base64_encode(&hash)
}

/// Generate a random-ish Sec-WebSocket-Key (16 bytes base64-encoded)
///
/// In a real implementation this would use a CSPRNG; we use a simple
/// deterministic sequence seeded by a counter.
pub fn generate_key(seed: u32) -> String {
    let mut key = [0u8; 16];
    let mut val = seed;
    for b in key.iter_mut() {
        val = val.wrapping_mul(1103515245).wrapping_add(12345);
        *b = (val >> 16) as u8;
    }
    base64_encode(&key)
}

/// Parsed HTTP header
#[derive(Debug, Clone)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

/// Parsed HTTP request line
#[derive(Debug, Clone)]
pub struct HttpRequestLine {
    pub method: String,
    pub uri: String,
    pub version: String,
}

/// Parsed HTTP status line
#[derive(Debug, Clone)]
pub struct HttpStatusLine {
    pub version: String,
    pub status_code: u16,
    pub reason: String,
}

/// Parse an HTTP request (request line + headers)
pub fn parse_http_request(data: &[u8]) -> Option<(HttpRequestLine, Vec<HttpHeader>, usize)> {
    let text = core::str::from_utf8(data).ok()?;
    let header_end = text.find("\r\n\r\n")?;
    let header_text = &text[..header_end];
    let consumed = header_end + 4;

    let mut lines = header_text.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.splitn(3, ' ');
    let method_str = parts.next()?;
    let method = String::from(method_str);
    let uri_str = parts.next()?;
    let uri = String::from(uri_str);
    let version_str = parts.next()?;
    let version = String::from(version_str);

    let req = HttpRequestLine {
        method,
        uri,
        version,
    };

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon_pos) = line.find(':') {
            let name = line[..colon_pos].trim();
            let value = line[colon_pos + 1..].trim();
            headers.push(HttpHeader {
                name: String::from(name),
                value: String::from(value),
            });
        }
    }

    Some((req, headers, consumed))
}

/// Parse an HTTP response (status line + headers)
pub fn parse_http_response(data: &[u8]) -> Option<(HttpStatusLine, Vec<HttpHeader>, usize)> {
    let text = core::str::from_utf8(data).ok()?;
    let header_end = text.find("\r\n\r\n")?;
    let header_text = &text[..header_end];
    let consumed = header_end + 4;

    let mut lines = header_text.split("\r\n");
    let status_line = lines.next()?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next()?;
    let code_str = parts.next()?;
    let reason = parts.next().unwrap_or("");

    let status = HttpStatusLine {
        version: String::from(version),
        status_code: code_str.parse().ok()?,
        reason: String::from(reason),
    };

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon_pos) = line.find(':') {
            let name = line[..colon_pos].trim();
            let value = line[colon_pos + 1..].trim();
            headers.push(HttpHeader {
                name: String::from(name),
                value: String::from(value),
            });
        }
    }

    Some((status, headers, consumed))
}

/// Find a header by name (case-insensitive)
pub fn find_header<'a>(headers: &'a [HttpHeader], name: &str) -> Option<&'a str> {
    for h in headers {
        if h.name.eq_ignore_ascii_case(name) {
            return Some(&h.value);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Client handshake
// ---------------------------------------------------------------------------

/// Build a WebSocket client handshake request
pub fn build_client_handshake(host: &str, path: &str, key: &str, protocols: &[&str]) -> Vec<u8> {
    let mut req = String::new();
    req.push_str(&alloc::format!("GET {} HTTP/1.1\r\n", path));
    req.push_str(&alloc::format!("Host: {}\r\n", host));
    req.push_str("Upgrade: websocket\r\n");
    req.push_str("Connection: Upgrade\r\n");
    req.push_str(&alloc::format!("Sec-WebSocket-Key: {}\r\n", key));
    req.push_str(&alloc::format!("Sec-WebSocket-Version: {}\r\n", WS_VERSION));
    if !protocols.is_empty() {
        let proto_list: Vec<&str> = protocols.to_vec();
        req.push_str(&alloc::format!(
            "Sec-WebSocket-Protocol: {}\r\n",
            proto_list.join(", ")
        ));
    }
    req.push_str("\r\n");
    Vec::from(req.as_bytes())
}

/// Validate a server's handshake response
pub fn validate_server_handshake(
    data: &[u8],
    expected_key: &str,
) -> Result<HandshakeResult, HandshakeError> {
    let (status, headers, consumed) =
        parse_http_response(data).ok_or(HandshakeError::InvalidResponse)?;

    if status.status_code != 101 {
        return Err(HandshakeError::BadStatusCode(status.status_code));
    }

    // Verify Upgrade: websocket
    let upgrade = find_header(&headers, "Upgrade").ok_or(HandshakeError::MissingUpgrade)?;
    if !upgrade.eq_ignore_ascii_case("websocket") {
        return Err(HandshakeError::InvalidUpgrade);
    }

    // Verify Connection: Upgrade
    let connection =
        find_header(&headers, "Connection").ok_or(HandshakeError::MissingConnection)?;
    if !connection.to_ascii_lowercase().contains("upgrade") {
        return Err(HandshakeError::InvalidConnection);
    }

    // Verify Sec-WebSocket-Accept
    let accept =
        find_header(&headers, "Sec-WebSocket-Accept").ok_or(HandshakeError::MissingAccept)?;
    let expected_accept = compute_accept_key(expected_key);
    if accept != expected_accept {
        return Err(HandshakeError::InvalidAccept);
    }

    // Optional: parse selected protocol
    let protocol = find_header(&headers, "Sec-WebSocket-Protocol").map(|s| String::from(s));

    Ok(HandshakeResult {
        protocol,
        bytes_consumed: consumed,
    })
}

// ---------------------------------------------------------------------------
// Server handshake
// ---------------------------------------------------------------------------

/// Validate a client's handshake request and build the response
pub fn process_client_handshake(data: &[u8]) -> Result<(Vec<u8>, HandshakeResult), HandshakeError> {
    let (req, headers, consumed) =
        parse_http_request(data).ok_or(HandshakeError::InvalidResponse)?;

    if req.method != "GET" {
        return Err(HandshakeError::InvalidMethod);
    }

    // Check Upgrade header
    let upgrade = find_header(&headers, "Upgrade").ok_or(HandshakeError::MissingUpgrade)?;
    if !upgrade.eq_ignore_ascii_case("websocket") {
        return Err(HandshakeError::InvalidUpgrade);
    }

    // Check Connection header
    let connection =
        find_header(&headers, "Connection").ok_or(HandshakeError::MissingConnection)?;
    if !connection.to_ascii_lowercase().contains("upgrade") {
        return Err(HandshakeError::InvalidConnection);
    }

    // Get Sec-WebSocket-Key
    let key = find_header(&headers, "Sec-WebSocket-Key").ok_or(HandshakeError::MissingKey)?;

    // Check version
    let version =
        find_header(&headers, "Sec-WebSocket-Version").ok_or(HandshakeError::MissingVersion)?;
    if version != "13" {
        return Err(HandshakeError::UnsupportedVersion);
    }

    // Compute accept key
    let accept = compute_accept_key(key);

    // Optional: select protocol
    let client_protos = find_header(&headers, "Sec-WebSocket-Protocol");

    // Build response
    let mut resp = String::new();
    resp.push_str("HTTP/1.1 101 Switching Protocols\r\n");
    resp.push_str("Upgrade: websocket\r\n");
    resp.push_str("Connection: Upgrade\r\n");
    resp.push_str(&alloc::format!("Sec-WebSocket-Accept: {}\r\n", accept));
    resp.push_str("\r\n");

    Ok((
        Vec::from(resp.as_bytes()),
        HandshakeResult {
            protocol: client_protos.map(|s| String::from(s)),
            bytes_consumed: consumed,
        },
    ))
}

// ---------------------------------------------------------------------------
// Handshake result / errors
// ---------------------------------------------------------------------------

/// Result of a successful handshake
#[derive(Debug, Clone)]
pub struct HandshakeResult {
    /// Selected sub-protocol (if any)
    pub protocol: Option<String>,
    /// Number of bytes consumed from the input buffer
    pub bytes_consumed: usize,
}

/// Handshake errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    InvalidResponse,
    BadStatusCode(u16),
    MissingUpgrade,
    InvalidUpgrade,
    MissingConnection,
    InvalidConnection,
    MissingAccept,
    InvalidAccept,
    MissingKey,
    MissingVersion,
    UnsupportedVersion,
    InvalidMethod,
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  Net: WebSocket handshake protocol ready");
}
