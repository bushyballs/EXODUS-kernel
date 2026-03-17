/// HTTP/1.1 protocol implementation for the Genesis webserver
///
/// Full request parsing, response building, status codes, header management,
/// chunked transfer encoding, and content negotiation.
///
/// Conforms to RFC 7230-7235 (HTTP/1.1). All code is original.

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

// ============================================================================
// HTTP Methods
// ============================================================================

/// HTTP request method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Trace,
    Connect,
}

impl Method {
    /// Parse method from ASCII string
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(Method::Get),
            "HEAD" => Some(Method::Head),
            "POST" => Some(Method::Post),
            "PUT" => Some(Method::Put),
            "DELETE" => Some(Method::Delete),
            "PATCH" => Some(Method::Patch),
            "OPTIONS" => Some(Method::Options),
            "TRACE" => Some(Method::Trace),
            "CONNECT" => Some(Method::Connect),
            _ => None,
        }
    }

    /// Return the method as a static string
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
            Method::Connect => "CONNECT",
        }
    }

    /// Whether this method typically has a request body
    pub fn has_body(&self) -> bool {
        matches!(self, Method::Post | Method::Put | Method::Patch)
    }

    /// Whether this method is idempotent
    pub fn is_idempotent(&self) -> bool {
        !matches!(self, Method::Post | Method::Patch)
    }

    /// Whether this method is safe (read-only)
    pub fn is_safe(&self) -> bool {
        matches!(self, Method::Get | Method::Head | Method::Options | Method::Trace)
    }
}

// ============================================================================
// Status Codes
// ============================================================================

/// HTTP response status code
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StatusCode {
    // 1xx Informational
    Continue = 100,
    SwitchingProtocols = 101,

    // 2xx Success
    Ok = 200,
    Created = 201,
    Accepted = 202,
    NoContent = 204,
    ResetContent = 205,
    PartialContent = 206,

    // 3xx Redirection
    MovedPermanently = 301,
    Found = 302,
    SeeOther = 303,
    NotModified = 304,
    TemporaryRedirect = 307,
    PermanentRedirect = 308,

    // 4xx Client Error
    BadRequest = 400,
    Unauthorized = 401,
    Forbidden = 403,
    NotFound = 404,
    MethodNotAllowed = 405,
    NotAcceptable = 406,
    RequestTimeout = 408,
    Conflict = 409,
    Gone = 410,
    LengthRequired = 411,
    PayloadTooLarge = 413,
    UriTooLong = 414,
    UnsupportedMediaType = 415,
    RangeNotSatisfiable = 416,
    TooManyRequests = 429,

    // 5xx Server Error
    InternalServerError = 500,
    NotImplemented = 501,
    BadGateway = 502,
    ServiceUnavailable = 503,
    GatewayTimeout = 504,
    HttpVersionNotSupported = 505,
}

impl StatusCode {
    /// Standard reason phrase for this status code
    pub fn reason(&self) -> &'static str {
        match self {
            StatusCode::Continue => "Continue",
            StatusCode::SwitchingProtocols => "Switching Protocols",
            StatusCode::Ok => "OK",
            StatusCode::Created => "Created",
            StatusCode::Accepted => "Accepted",
            StatusCode::NoContent => "No Content",
            StatusCode::ResetContent => "Reset Content",
            StatusCode::PartialContent => "Partial Content",
            StatusCode::MovedPermanently => "Moved Permanently",
            StatusCode::Found => "Found",
            StatusCode::SeeOther => "See Other",
            StatusCode::NotModified => "Not Modified",
            StatusCode::TemporaryRedirect => "Temporary Redirect",
            StatusCode::PermanentRedirect => "Permanent Redirect",
            StatusCode::BadRequest => "Bad Request",
            StatusCode::Unauthorized => "Unauthorized",
            StatusCode::Forbidden => "Forbidden",
            StatusCode::NotFound => "Not Found",
            StatusCode::MethodNotAllowed => "Method Not Allowed",
            StatusCode::NotAcceptable => "Not Acceptable",
            StatusCode::RequestTimeout => "Request Timeout",
            StatusCode::Conflict => "Conflict",
            StatusCode::Gone => "Gone",
            StatusCode::LengthRequired => "Length Required",
            StatusCode::PayloadTooLarge => "Payload Too Large",
            StatusCode::UriTooLong => "URI Too Long",
            StatusCode::UnsupportedMediaType => "Unsupported Media Type",
            StatusCode::RangeNotSatisfiable => "Range Not Satisfiable",
            StatusCode::TooManyRequests => "Too Many Requests",
            StatusCode::InternalServerError => "Internal Server Error",
            StatusCode::NotImplemented => "Not Implemented",
            StatusCode::BadGateway => "Bad Gateway",
            StatusCode::ServiceUnavailable => "Service Unavailable",
            StatusCode::GatewayTimeout => "Gateway Timeout",
            StatusCode::HttpVersionNotSupported => "HTTP Version Not Supported",
        }
    }

    /// Whether this is an informational status (1xx)
    pub fn is_informational(&self) -> bool {
        (*self as u16) >= 100 && (*self as u16) < 200
    }

    /// Whether this is a success status (2xx)
    pub fn is_success(&self) -> bool {
        (*self as u16) >= 200 && (*self as u16) < 300
    }

    /// Whether this is a redirection status (3xx)
    pub fn is_redirection(&self) -> bool {
        (*self as u16) >= 300 && (*self as u16) < 400
    }

    /// Whether this is a client error (4xx)
    pub fn is_client_error(&self) -> bool {
        (*self as u16) >= 400 && (*self as u16) < 500
    }

    /// Whether this is a server error (5xx)
    pub fn is_server_error(&self) -> bool {
        (*self as u16) >= 500 && (*self as u16) < 600
    }
}

// ============================================================================
// Headers
// ============================================================================

/// Case-insensitive header storage
pub struct Headers {
    entries: BTreeMap<String, String>,
}

impl Headers {
    pub fn new() -> Self {
        Headers { entries: BTreeMap::new() }
    }

    /// Set a header (key is lowercased for case-insensitive matching)
    pub fn set(&mut self, key: &str, value: &str) {
        let lower = to_lowercase(key);
        self.entries.insert(lower, String::from(value));
    }

    /// Get a header value by name (case-insensitive)
    pub fn get(&self, key: &str) -> Option<&str> {
        let lower = to_lowercase(key);
        self.entries.get(&lower).map(|s| s.as_str())
    }

    /// Check if a header exists
    pub fn contains(&self, key: &str) -> bool {
        let lower = to_lowercase(key);
        self.entries.contains_key(&lower)
    }

    /// Remove a header
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let lower = to_lowercase(key);
        self.entries.remove(&lower)
    }

    /// Iterate over all headers
    pub fn iter(&self) -> alloc::collections::btree_map::Iter<String, String> {
        self.entries.iter()
    }

    /// Get content-length if present
    pub fn content_length(&self) -> Option<usize> {
        self.get("content-length").and_then(|v| parse_usize(v))
    }

    /// Check if connection should be kept alive
    pub fn is_keep_alive(&self) -> bool {
        if let Some(conn) = self.get("connection") {
            let lower = to_lowercase(conn);
            lower.as_str() != "close"
        } else {
            true  // HTTP/1.1 default is keep-alive
        }
    }

    /// Check if transfer-encoding is chunked
    pub fn is_chunked(&self) -> bool {
        if let Some(te) = self.get("transfer-encoding") {
            let lower = to_lowercase(te);
            lower.contains("chunked")
        } else {
            false
        }
    }

    /// Number of headers
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ============================================================================
// HTTP Request
// ============================================================================

/// Parsed HTTP request
pub struct HttpRequest {
    pub method: Method,
    pub path: String,
    pub query_string: String,
    pub version: HttpVersion,
    pub headers: Headers,
    pub body: Vec<u8>,
    /// Parsed query parameters
    pub query_params: BTreeMap<String, String>,
    /// Path parameters from wildcard matching (filled by router)
    pub path_params: BTreeMap<String, String>,
}

/// HTTP protocol version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    Http10,
    Http11,
}

impl HttpVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpVersion::Http10 => "HTTP/1.0",
            HttpVersion::Http11 => "HTTP/1.1",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "HTTP/1.0" => Some(HttpVersion::Http10),
            "HTTP/1.1" => Some(HttpVersion::Http11),
            _ => None,
        }
    }
}

impl HttpRequest {
    /// Parse an HTTP request from raw bytes.
    /// Returns None if the data is incomplete or malformed.
    pub fn parse(data: &[u8]) -> Option<Self> {
        // Find end of headers
        let header_end = find_header_end(data)?;
        let header_bytes = &data[..header_end];
        let header_text = core::str::from_utf8(header_bytes).ok()?;

        let mut lines = header_text.split("\r\n");

        // Parse request line: METHOD PATH VERSION
        let request_line = lines.next()?;
        if request_line.is_empty() { return None; }

        let mut parts = request_line.splitn(3, ' ');
        let method_str = parts.next()?;
        let uri = parts.next()?;
        let version_str = parts.next()?;

        let method = Method::parse(method_str)?;
        let version = HttpVersion::parse(version_str)?;

        // Split URI into path and query string
        let (path, query_string, query_params) = parse_uri(uri);

        // Parse headers
        let mut headers = Headers::new();
        for line in lines {
            if line.is_empty() { break; }
            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim();
                let val = line[colon_pos + 1..].trim();
                headers.set(key, val);
            }
        }

        // Extract body
        let body_start = header_end + 4;  // skip \r\n\r\n
        let body = if body_start < data.len() {
            let content_len = headers.content_length().unwrap_or(data.len() - body_start);
            let end = (body_start + content_len).min(data.len());
            data[body_start..end].to_vec()
        } else {
            Vec::new()
        };

        Some(HttpRequest {
            method,
            path,
            query_string,
            version,
            headers,
            body,
            query_params,
            path_params: BTreeMap::new(),
        })
    }

    /// Get the Host header value
    pub fn host(&self) -> Option<&str> {
        self.headers.get("host")
    }

    /// Get the Content-Type header value
    pub fn content_type(&self) -> Option<&str> {
        self.headers.get("content-type")
    }

    /// Get the User-Agent header value
    pub fn user_agent(&self) -> Option<&str> {
        self.headers.get("user-agent")
    }

    /// Check if the client accepts a given content type
    pub fn accepts(&self, content_type: &str) -> bool {
        if let Some(accept) = self.headers.get("accept") {
            accept.contains(content_type) || accept.contains("*/*")
        } else {
            true  // No Accept header means accept anything
        }
    }

    /// Get a query parameter by key
    pub fn query_param(&self, key: &str) -> Option<&str> {
        self.query_params.get(key).map(|s| s.as_str())
    }

    /// Get a path parameter by name (set by router during wildcard matching)
    pub fn path_param(&self, key: &str) -> Option<&str> {
        self.path_params.get(key).map(|s| s.as_str())
    }

    /// Get the body as a UTF-8 string
    pub fn body_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.body).ok()
    }
}

// ============================================================================
// HTTP Response
// ============================================================================

/// HTTP response builder
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: Headers,
    pub body: Vec<u8>,
    pub version: HttpVersion,
}

impl HttpResponse {
    /// Create a new response with the given status code
    pub fn new(status: StatusCode) -> Self {
        let mut headers = Headers::new();
        headers.set("server", "Genesis/1.0");
        headers.set("connection", "keep-alive");
        HttpResponse {
            status,
            headers,
            body: Vec::new(),
            version: HttpVersion::Http11,
        }
    }

    /// Create a plain text response
    pub fn text(status: StatusCode, text: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.set("content-type", "text/plain; charset=utf-8");
        resp.body = text.as_bytes().to_vec();
        resp
    }

    /// Create an HTML response
    pub fn html(status: StatusCode, html: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.set("content-type", "text/html; charset=utf-8");
        resp.body = html.as_bytes().to_vec();
        resp
    }

    /// Create a JSON response
    pub fn json(status: StatusCode, json: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.set("content-type", "application/json");
        resp.body = json.as_bytes().to_vec();
        resp
    }

    /// Create a binary response with explicit content type
    pub fn binary(status: StatusCode, content_type: &str, data: Vec<u8>) -> Self {
        let mut resp = Self::new(status);
        resp.headers.set("content-type", content_type);
        resp.body = data;
        resp
    }

    /// Create a redirect response
    pub fn redirect(status: StatusCode, location: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.set("location", location);
        resp
    }

    /// Add a header to the response
    pub fn with_header(mut self, key: &str, value: &str) -> Self {
        self.headers.set(key, value);
        self
    }

    /// Set the response body
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// Serialize the response to wire bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256 + self.body.len());

        // Status line
        let status_line = format!("{} {} {}\r\n",
            self.version.as_str(),
            self.status as u16,
            self.status.reason());
        buf.extend_from_slice(status_line.as_bytes());

        // Content-Length (always include unless 204/304)
        if self.status != StatusCode::NoContent && self.status != StatusCode::NotModified {
            let cl = format!("content-length: {}\r\n", self.body.len());
            buf.extend_from_slice(cl.as_bytes());
        }

        // Headers
        for (key, val) in self.headers.iter() {
            let line = format!("{}: {}\r\n", key, val);
            buf.extend_from_slice(line.as_bytes());
        }

        // Header terminator
        buf.extend_from_slice(b"\r\n");

        // Body
        if !self.body.is_empty() {
            buf.extend_from_slice(&self.body);
        }

        buf
    }

    /// Encode body using chunked transfer encoding
    pub fn to_chunked_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256 + self.body.len() + 32);

        // Status line
        let status_line = format!("{} {} {}\r\n",
            self.version.as_str(),
            self.status as u16,
            self.status.reason());
        buf.extend_from_slice(status_line.as_bytes());

        // Headers (with transfer-encoding: chunked)
        for (key, val) in self.headers.iter() {
            let line = format!("{}: {}\r\n", key, val);
            buf.extend_from_slice(line.as_bytes());
        }
        buf.extend_from_slice(b"transfer-encoding: chunked\r\n");
        buf.extend_from_slice(b"\r\n");

        // Chunked body
        if !self.body.is_empty() {
            let chunk_header = format!("{:x}\r\n", self.body.len());
            buf.extend_from_slice(chunk_header.as_bytes());
            buf.extend_from_slice(&self.body);
            buf.extend_from_slice(b"\r\n");
        }

        // Final chunk
        buf.extend_from_slice(b"0\r\n\r\n");
        buf
    }
}

// ============================================================================
// Request rate limiter (per-IP)
// ============================================================================

/// Simple per-IP rate limit tracker
struct RateLimitEntry {
    ip: [u8; 4],
    request_count: u32,
    window_start: u64,
}

/// Rate limit configuration
static RATE_LIMIT: Mutex<Vec<RateLimitEntry>> = Mutex::new(Vec::new());

/// Maximum requests per window
const RATE_LIMIT_MAX: u32 = 100;
/// Window duration in ticks (approximately 60 seconds)
const RATE_LIMIT_WINDOW: u64 = 60;

/// Check rate limit for a given IP. Returns true if allowed.
pub fn check_rate_limit(ip: [u8; 4], current_tick: u64) -> bool {
    let mut entries = RATE_LIMIT.lock();

    // Find or create entry for this IP
    if let Some(entry) = entries.iter_mut().find(|e| e.ip == ip) {
        if current_tick.saturating_sub(entry.window_start) > RATE_LIMIT_WINDOW {
            // Window expired, reset
            entry.request_count = 1;
            entry.window_start = current_tick;
            true
        } else if entry.request_count < RATE_LIMIT_MAX {
            entry.request_count = entry.request_count.saturating_add(1);
            true
        } else {
            serial_println!("  [http] rate limited {}.{}.{}.{}",
                ip[0], ip[1], ip[2], ip[3]);
            false
        }
    } else {
        entries.push(RateLimitEntry {
            ip,
            request_count: 1,
            window_start: current_tick,
        });
        true
    }
}

/// Clean up expired rate limit entries
pub fn cleanup_rate_limits(current_tick: u64) {
    let mut entries = RATE_LIMIT.lock();
    entries.retain(|e| current_tick.saturating_sub(e.window_start) <= RATE_LIMIT_WINDOW * 2);
}

// ============================================================================
// Utility functions
// ============================================================================

/// Convert a string to lowercase (ASCII only, no_std compatible)
fn to_lowercase(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if c >= 'A' && c <= 'Z' {
            result.push((c as u8 + 32) as char);
        } else {
            result.push(c);
        }
    }
    result
}

/// Parse a usize from a string (no_std compatible)
fn parse_usize(s: &str) -> Option<usize> {
    let s = s.trim();
    let mut result: usize = 0;
    for c in s.chars() {
        if c < '0' || c > '9' { return None; }
        result = result.checked_mul(10)?;
        result = result.checked_add((c as u8 - b'0') as usize)?;
    }
    Some(result)
}

/// Find the end of HTTP headers (\r\n\r\n)
fn find_header_end(data: &[u8]) -> Option<usize> {
    if data.len() < 4 { return None; }
    for i in 0..data.len() - 3 {
        if data[i] == b'\r' && data[i + 1] == b'\n'
            && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

/// Parse a URI into (path, query_string, query_params)
fn parse_uri(uri: &str) -> (String, String, BTreeMap<String, String>) {
    let mut params = BTreeMap::new();

    if let Some(qmark) = uri.find('?') {
        let path = String::from(&uri[..qmark]);
        let qs = String::from(&uri[qmark + 1..]);

        for pair in qs.split('&') {
            if pair.is_empty() { continue; }
            if let Some(eq) = pair.find('=') {
                let key = percent_decode(&pair[..eq]);
                let val = percent_decode(&pair[eq + 1..]);
                params.insert(key, val);
            } else {
                params.insert(percent_decode(pair), String::new());
            }
        }

        (path, qs, params)
    } else {
        (String::from(uri), String::new(), params)
    }
}

/// Decode percent-encoded characters in a string
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_digit(bytes[i + 1]);
            let lo = hex_digit(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            result.push(b' ');
        } else {
            result.push(bytes[i]);
        }
        i += 1;
    }

    String::from_utf8(result).unwrap_or_else(|_| String::from(s))
}

/// Convert a hex ASCII digit to its numeric value
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-encode a string for use in URLs
pub fn percent_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push('%');
                let hi = b >> 4;
                let lo = b & 0x0F;
                result.push(HEX_CHARS[hi as usize] as char);
                result.push(HEX_CHARS[lo as usize] as char);
            }
        }
    }
    result
}

/// Hex character lookup table
const HEX_CHARS: &[u8; 16] = b"0123456789ABCDEF";

/// Initialize the HTTP protocol module
pub fn init() {
    serial_println!("  [http] HTTP/1.1 protocol parser initialized");
    serial_println!("  [http] Methods: GET HEAD POST PUT DELETE PATCH OPTIONS");
    serial_println!("  [http] Features: keep-alive, chunked, rate-limit, percent-encoding");
}
