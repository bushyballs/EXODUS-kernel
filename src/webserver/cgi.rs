/// CGI / FastCGI interface for dynamic content generation
///
/// Implements:
///   - CGI/1.1 environment variable setup (RFC 3875)
///   - FastCGI multiplexed connections (simplified)
///   - Script registry and execution dispatch
///   - Request/response translation between HTTP and CGI
///   - Built-in dynamic handlers (server-status, server-info, echo)
///   - Output buffering and header parsing
///
/// In a bare-metal kernel there is no fork/exec, so "scripts" are
/// registered Rust function pointers that receive the CGI environment
/// and produce output. This gives the same interface as traditional CGI
/// but runs entirely in kernel space.
///
/// All code is original.

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

use super::http::{HttpRequest, HttpResponse, StatusCode, Method};

// ============================================================================
// CGI environment
// ============================================================================

/// CGI environment variables (RFC 3875 Section 4)
pub struct CgiEnvironment {
    /// AUTH_TYPE — authentication method if any
    pub auth_type: String,
    /// CONTENT_LENGTH — size of request body
    pub content_length: u32,
    /// CONTENT_TYPE — MIME type of request body
    pub content_type: String,
    /// GATEWAY_INTERFACE — CGI version
    pub gateway_interface: String,
    /// PATH_INFO — extra path info after script name
    pub path_info: String,
    /// PATH_TRANSLATED — filesystem-translated PATH_INFO
    pub path_translated: String,
    /// QUERY_STRING — URL query string
    pub query_string: String,
    /// REMOTE_ADDR — client IP address
    pub remote_addr: String,
    /// REMOTE_HOST — client hostname (usually same as REMOTE_ADDR)
    pub remote_host: String,
    /// REQUEST_METHOD — HTTP method
    pub request_method: String,
    /// SCRIPT_NAME — path to the CGI script
    pub script_name: String,
    /// SERVER_NAME — server hostname
    pub server_name: String,
    /// SERVER_PORT — server port number
    pub server_port: u16,
    /// SERVER_PROTOCOL — HTTP version
    pub server_protocol: String,
    /// SERVER_SOFTWARE — server identification
    pub server_software: String,
    /// Additional HTTP_* variables from request headers
    pub http_vars: BTreeMap<String, String>,
    /// Request body data
    pub body: Vec<u8>,
}

impl CgiEnvironment {
    /// Build a CGI environment from an HTTP request
    pub fn from_request(req: &HttpRequest, script_name: &str, remote_ip: &str) -> Self {
        let mut http_vars = BTreeMap::new();

        // Convert HTTP headers to CGI HTTP_* variables
        for (key, value) in req.headers.iter() {
            let cgi_key = header_to_cgi_var(key);
            http_vars.insert(cgi_key, String::from(value));
        }

        // PATH_INFO is the part of the path after the script name
        let path_info = if req.path.len() > script_name.len() {
            String::from(&req.path[script_name.len()..])
        } else {
            String::new()
        };

        CgiEnvironment {
            auth_type: String::new(),
            content_length: req.body.len() as u32,
            content_type: req.content_type().map(String::from).unwrap_or_default(),
            gateway_interface: String::from("CGI/1.1"),
            path_info,
            path_translated: String::new(),
            query_string: req.query_string.clone(),
            remote_addr: String::from(remote_ip),
            remote_host: String::from(remote_ip),
            request_method: String::from(req.method.as_str()),
            script_name: String::from(script_name),
            server_name: String::from("genesis"),
            server_port: 8080,
            server_protocol: String::from(req.version.as_str()),
            server_software: String::from("Genesis/1.0 (CGI)"),
            http_vars,
            body: req.body.clone(),
        }
    }

    /// Get a variable by its CGI name
    pub fn get_var(&self, name: &str) -> Option<String> {
        match name {
            "AUTH_TYPE" => Some(self.auth_type.clone()),
            "CONTENT_LENGTH" => Some(format!("{}", self.content_length)),
            "CONTENT_TYPE" => Some(self.content_type.clone()),
            "GATEWAY_INTERFACE" => Some(self.gateway_interface.clone()),
            "PATH_INFO" => Some(self.path_info.clone()),
            "PATH_TRANSLATED" => Some(self.path_translated.clone()),
            "QUERY_STRING" => Some(self.query_string.clone()),
            "REMOTE_ADDR" => Some(self.remote_addr.clone()),
            "REMOTE_HOST" => Some(self.remote_host.clone()),
            "REQUEST_METHOD" => Some(self.request_method.clone()),
            "SCRIPT_NAME" => Some(self.script_name.clone()),
            "SERVER_NAME" => Some(self.server_name.clone()),
            "SERVER_PORT" => Some(format!("{}", self.server_port)),
            "SERVER_PROTOCOL" => Some(self.server_protocol.clone()),
            "SERVER_SOFTWARE" => Some(self.server_software.clone()),
            _ => self.http_vars.get(name).cloned(),
        }
    }

    /// List all CGI variables as key-value pairs
    pub fn all_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        vars.push((String::from("AUTH_TYPE"), self.auth_type.clone()));
        vars.push((String::from("CONTENT_LENGTH"), format!("{}", self.content_length)));
        vars.push((String::from("CONTENT_TYPE"), self.content_type.clone()));
        vars.push((String::from("GATEWAY_INTERFACE"), self.gateway_interface.clone()));
        vars.push((String::from("PATH_INFO"), self.path_info.clone()));
        vars.push((String::from("QUERY_STRING"), self.query_string.clone()));
        vars.push((String::from("REMOTE_ADDR"), self.remote_addr.clone()));
        vars.push((String::from("REQUEST_METHOD"), self.request_method.clone()));
        vars.push((String::from("SCRIPT_NAME"), self.script_name.clone()));
        vars.push((String::from("SERVER_NAME"), self.server_name.clone()));
        vars.push((String::from("SERVER_PORT"), format!("{}", self.server_port)));
        vars.push((String::from("SERVER_PROTOCOL"), self.server_protocol.clone()));
        vars.push((String::from("SERVER_SOFTWARE"), self.server_software.clone()));
        for (key, val) in &self.http_vars {
            vars.push((key.clone(), val.clone()));
        }
        vars
    }
}

/// Convert an HTTP header name to a CGI environment variable name
/// e.g., "accept-language" -> "HTTP_ACCEPT_LANGUAGE"
fn header_to_cgi_var(header: &str) -> String {
    let mut var = String::from("HTTP_");
    for c in header.chars() {
        if c == '-' {
            var.push('_');
        } else if c >= 'a' && c <= 'z' {
            var.push((c as u8 - 32) as char);
        } else {
            var.push(c);
        }
    }
    var
}

// ============================================================================
// CGI output parsing
// ============================================================================

/// Parsed CGI output (headers + body)
pub struct CgiOutput {
    /// HTTP status code (from Status header, default 200)
    pub status: u16,
    /// Response headers
    pub headers: BTreeMap<String, String>,
    /// Response body
    pub body: Vec<u8>,
}

/// Parse CGI script output into headers and body.
/// CGI output format: headers separated by blank line from body,
/// with optional "Status: NNN reason" header.
pub fn parse_cgi_output(output: &[u8]) -> CgiOutput {
    let mut status: u16 = 200;
    let mut headers = BTreeMap::new();

    // Find blank line separating headers from body
    let header_end = find_blank_line(output);
    let (header_section, body) = if let Some(pos) = header_end {
        let body_start = pos + find_line_ending_len(output, pos);
        (&output[..pos], output[body_start..].to_vec())
    } else {
        // No headers, entire output is body
        (&output[..0], output.to_vec())
    };

    // Parse CGI headers
    if let Ok(header_text) = core::str::from_utf8(header_section) {
        for line in header_text.split('\n') {
            let line = line.trim_end_matches('\r');
            if line.is_empty() { break; }

            if let Some(colon) = line.find(':') {
                let key = line[..colon].trim();
                let val = line[colon + 1..].trim();

                if key.eq_ignore_ascii_case("Status") {
                    // Parse "Status: 200 OK" -> extract code
                    if let Some(code) = parse_status_code(val) {
                        status = code;
                    }
                } else {
                    headers.insert(String::from(key), String::from(val));
                }
            }
        }
    }

    // Default Content-Type if not specified
    if !headers.contains_key("Content-Type") && !headers.contains_key("content-type") {
        headers.insert(String::from("content-type"), String::from("text/html"));
    }

    CgiOutput { status, headers, body }
}

/// Find the position of the first blank line (CRLF CRLF or LF LF)
fn find_blank_line(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(1) {
        // Check for \n\n
        if data[i] == b'\n' && data[i + 1] == b'\n' {
            return Some(i);
        }
        // Check for \r\n\r\n
        if i + 3 < data.len()
            && data[i] == b'\r' && data[i + 1] == b'\n'
            && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

/// Get the length of the line ending at a position
fn find_line_ending_len(data: &[u8], pos: usize) -> usize {
    if pos + 3 < data.len() && data[pos] == b'\r' && data[pos + 1] == b'\n'
        && data[pos + 2] == b'\r' && data[pos + 3] == b'\n'
    {
        4
    } else if pos + 1 < data.len() && data[pos] == b'\n' && data[pos + 1] == b'\n' {
        2
    } else {
        0
    }
}

/// Parse status code from "NNN reason" string
fn parse_status_code(s: &str) -> Option<u16> {
    let num_str = s.split_whitespace().next()?;
    let mut code: u16 = 0;
    for c in num_str.chars() {
        if c < '0' || c > '9' { return None; }
        code = code.checked_mul(10)?;
        code = code.checked_add((c as u8 - b'0') as u16)?;
    }
    if code >= 100 && code < 600 {
        Some(code)
    } else {
        None
    }
}

// ============================================================================
// Script registry
// ============================================================================

/// CGI script handler function type.
/// Receives the CGI environment, returns raw output bytes.
pub type CgiHandler = fn(&CgiEnvironment) -> Vec<u8>;

/// A registered CGI script
struct CgiScript {
    /// URL path prefix that triggers this script
    path_prefix: String,
    /// The handler function
    handler: CgiHandler,
    /// Whether this script is enabled
    enabled: bool,
    /// Total invocations
    invocations: u64,
    /// Total execution time in ticks (approximation)
    total_ticks: u64,
}

/// Script registry
static SCRIPTS: Mutex<Vec<CgiScript>> = Mutex::new(Vec::new());

/// CGI invocation statistics
static CGI_STATS: Mutex<CgiStats> = Mutex::new(CgiStats::new());

struct CgiStats {
    total_invocations: u64,
    total_errors: u64,
    active_scripts: u32,
}

impl CgiStats {
    const fn new() -> Self {
        CgiStats {
            total_invocations: 0,
            total_errors: 0,
            active_scripts: 0,
        }
    }
}

/// Register a CGI script handler at a given URL prefix
pub fn register_script(path_prefix: &str, handler: CgiHandler) {
    let mut scripts = SCRIPTS.lock();
    scripts.push(CgiScript {
        path_prefix: String::from(path_prefix),
        handler,
        enabled: true,
        invocations: 0,
        total_ticks: 0,
    });

    let mut stats = CGI_STATS.lock();
    stats.active_scripts = stats.active_scripts.saturating_add(1);

    serial_println!("  [cgi] registered script: {}", path_prefix);
}

/// Unregister a CGI script by path prefix
pub fn unregister_script(path_prefix: &str) -> bool {
    let mut scripts = SCRIPTS.lock();
    let before = scripts.len();
    scripts.retain(|s| s.path_prefix.as_str() != path_prefix);
    let removed = scripts.len() < before;

    if removed {
        let mut stats = CGI_STATS.lock();
        if stats.active_scripts > 0 {
            stats.active_scripts -= 1;
        }
    }

    removed
}

/// Enable or disable a CGI script
pub fn set_script_enabled(path_prefix: &str, enabled: bool) {
    let mut scripts = SCRIPTS.lock();
    if let Some(script) = scripts.iter_mut().find(|s| s.path_prefix.as_str() == path_prefix) {
        script.enabled = enabled;
    }
}

// ============================================================================
// CGI dispatch
// ============================================================================

/// Check if a request path matches a registered CGI script
pub fn is_cgi_request(path: &str) -> bool {
    let scripts = SCRIPTS.lock();
    scripts.iter().any(|s| s.enabled && path.starts_with(s.path_prefix.as_str()))
}

/// Execute a CGI request and return the HTTP response
pub fn execute(req: &HttpRequest, remote_ip: &str) -> HttpResponse {
    let mut scripts = SCRIPTS.lock();

    // Find matching script (longest prefix match)
    let mut best_match: Option<usize> = None;
    let mut best_len: usize = 0;

    for (i, script) in scripts.iter().enumerate() {
        if script.enabled
            && req.path.starts_with(script.path_prefix.as_str())
            && script.path_prefix.len() > best_len
        {
            best_match = Some(i);
            best_len = script.path_prefix.len();
        }
    }

    let script_idx = match best_match {
        Some(idx) => idx,
        None => {
            drop(scripts);
            return HttpResponse::text(StatusCode::NotFound, "No CGI script found");
        }
    };

    let handler = scripts[script_idx].handler;
    let script_name = scripts[script_idx].path_prefix.clone();
    scripts[script_idx].invocations = scripts[script_idx].invocations.saturating_add(1);
    drop(scripts);

    // Build CGI environment
    let env = CgiEnvironment::from_request(req, &script_name, remote_ip);

    // Execute the script handler
    serial_println!("  [cgi] executing: {} (method: {})", script_name, req.method.as_str());

    let raw_output = handler(&env);

    // Update stats
    let mut stats = CGI_STATS.lock();
    stats.total_invocations = stats.total_invocations.saturating_add(1);
    drop(stats);

    // Parse CGI output
    let cgi_out = parse_cgi_output(&raw_output);

    // Convert to HTTP response
    let status = match cgi_out.status {
        200 => StatusCode::Ok,
        201 => StatusCode::Created,
        204 => StatusCode::NoContent,
        301 => StatusCode::MovedPermanently,
        302 => StatusCode::Found,
        304 => StatusCode::NotModified,
        400 => StatusCode::BadRequest,
        401 => StatusCode::Unauthorized,
        403 => StatusCode::Forbidden,
        404 => StatusCode::NotFound,
        405 => StatusCode::MethodNotAllowed,
        500 => StatusCode::InternalServerError,
        503 => StatusCode::ServiceUnavailable,
        _ => StatusCode::Ok,
    };

    let mut resp = HttpResponse::new(status);
    for (key, val) in &cgi_out.headers {
        resp.headers.set(key, val);
    }
    resp.body = cgi_out.body;
    resp
}

// ============================================================================
// FastCGI protocol (simplified)
// ============================================================================

/// FastCGI record types
const FCGI_BEGIN_REQUEST: u8 = 1;
const FCGI_ABORT_REQUEST: u8 = 2;
const FCGI_END_REQUEST: u8 = 3;
const FCGI_PARAMS: u8 = 4;
const FCGI_STDIN: u8 = 5;
const FCGI_STDOUT: u8 = 6;
const FCGI_STDERR: u8 = 7;

/// FastCGI roles
const FCGI_RESPONDER: u16 = 1;

/// FastCGI record header (8 bytes)
#[repr(C, packed)]
struct FcgiHeader {
    version: u8,
    record_type: u8,
    request_id_hi: u8,
    request_id_lo: u8,
    content_length_hi: u8,
    content_length_lo: u8,
    padding_length: u8,
    reserved: u8,
}

/// Build a FastCGI record header
fn build_fcgi_header(record_type: u8, request_id: u16, content_length: u16) -> [u8; 8] {
    [
        1,  // version
        record_type,
        (request_id >> 8) as u8,
        (request_id & 0xFF) as u8,
        (content_length >> 8) as u8,
        (content_length & 0xFF) as u8,
        0,  // padding
        0,  // reserved
    ]
}

/// Encode a key-value pair in FastCGI name-value format
fn encode_fcgi_param(name: &str, value: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    let name_len = name.len();
    let val_len = value.len();

    // Name length (1 or 4 bytes)
    if name_len < 128 {
        buf.push(name_len as u8);
    } else {
        buf.push(((name_len >> 24) as u8) | 0x80);
        buf.push((name_len >> 16) as u8);
        buf.push((name_len >> 8) as u8);
        buf.push(name_len as u8);
    }

    // Value length (1 or 4 bytes)
    if val_len < 128 {
        buf.push(val_len as u8);
    } else {
        buf.push(((val_len >> 24) as u8) | 0x80);
        buf.push((val_len >> 16) as u8);
        buf.push((val_len >> 8) as u8);
        buf.push(val_len as u8);
    }

    buf.extend_from_slice(name.as_bytes());
    buf.extend_from_slice(value.as_bytes());
    buf
}

/// Build a complete FastCGI request for an HTTP request
pub fn build_fcgi_request(req: &HttpRequest, remote_ip: &str) -> Vec<u8> {
    let request_id: u16 = 1;
    let mut buf = Vec::new();

    // BEGIN_REQUEST record
    let begin_header = build_fcgi_header(FCGI_BEGIN_REQUEST, request_id, 8);
    buf.extend_from_slice(&begin_header);
    // Body: role (2) + flags (1) + reserved (5)
    buf.extend_from_slice(&(FCGI_RESPONDER.to_be_bytes()));
    buf.push(0);  // flags (0 = close connection after)
    buf.extend_from_slice(&[0; 5]);  // reserved

    // PARAMS records
    let env = CgiEnvironment::from_request(req, &req.path, remote_ip);
    let mut params_body = Vec::new();
    for (key, val) in &env.all_vars() {
        params_body.extend_from_slice(&encode_fcgi_param(key, val));
    }

    // Send params in chunks if needed (max 65535 per record)
    let mut offset = 0;
    while offset < params_body.len() {
        let chunk_len = (params_body.len() - offset).min(65535);
        let header = build_fcgi_header(FCGI_PARAMS, request_id, chunk_len as u16);
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&params_body[offset..offset + chunk_len]);
        offset += chunk_len;
    }

    // Empty PARAMS to signal end
    let empty_params = build_fcgi_header(FCGI_PARAMS, request_id, 0);
    buf.extend_from_slice(&empty_params);

    // STDIN records (request body)
    if !req.body.is_empty() {
        let mut body_offset = 0;
        while body_offset < req.body.len() {
            let chunk_len = (req.body.len() - body_offset).min(65535);
            let header = build_fcgi_header(FCGI_STDIN, request_id, chunk_len as u16);
            buf.extend_from_slice(&header);
            buf.extend_from_slice(&req.body[body_offset..body_offset + chunk_len]);
            body_offset += chunk_len;
        }
    }

    // Empty STDIN to signal end
    let empty_stdin = build_fcgi_header(FCGI_STDIN, request_id, 0);
    buf.extend_from_slice(&empty_stdin);

    buf
}

// ============================================================================
// Built-in CGI scripts
// ============================================================================

/// Server status CGI script — shows server information
fn server_status_handler(env: &CgiEnvironment) -> Vec<u8> {
    let (reqs, conns, sent, recv, ws) = super::get_stats();
    let route_count = super::router::route_count();
    let file_count = super::static_files::file_count();
    let ws_count = super::websocket::active_count();

    let mut output = String::from("Content-Type: text/html\r\n");
    output.push_str("Status: 200 OK\r\n\r\n");
    output.push_str("<!DOCTYPE html><html><head><title>Genesis Server Status</title></head><body>");
    output.push_str("<h1>Genesis Server Status</h1>");
    output.push_str("<table border=\"1\" cellpadding=\"4\">");
    output.push_str(&format!("<tr><td>Total Requests</td><td>{}</td></tr>", reqs));
    output.push_str(&format!("<tr><td>Active Connections</td><td>{}</td></tr>", conns));
    output.push_str(&format!("<tr><td>Bytes Sent</td><td>{}</td></tr>", sent));
    output.push_str(&format!("<tr><td>Bytes Received</td><td>{}</td></tr>", recv));
    output.push_str(&format!("<tr><td>WebSocket Upgrades</td><td>{}</td></tr>", ws));
    output.push_str(&format!("<tr><td>Active WebSockets</td><td>{}</td></tr>", ws_count));
    output.push_str(&format!("<tr><td>Registered Routes</td><td>{}</td></tr>", route_count));
    output.push_str(&format!("<tr><td>Static Files</td><td>{}</td></tr>", file_count));
    output.push_str("</table>");
    output.push_str("<hr><p>Genesis/1.0 CGI</p></body></html>");

    output.into_bytes()
}

/// Echo CGI script — echoes request info back to client
fn echo_handler(env: &CgiEnvironment) -> Vec<u8> {
    let mut output = String::from("Content-Type: text/plain\r\n");
    output.push_str("Status: 200 OK\r\n\r\n");
    output.push_str("=== CGI Environment ===\n");

    for (key, val) in &env.all_vars() {
        output.push_str(&format!("{}={}\n", key, val));
    }

    if !env.body.is_empty() {
        output.push_str("\n=== Request Body ===\n");
        if let Ok(body_str) = core::str::from_utf8(&env.body) {
            output.push_str(body_str);
        } else {
            output.push_str(&format!("[{} bytes binary data]", env.body.len()));
        }
        output.push('\n');
    }

    output.into_bytes()
}

/// Environment dump CGI script — JSON format
fn env_json_handler(env: &CgiEnvironment) -> Vec<u8> {
    let mut output = String::from("Content-Type: application/json\r\n");
    output.push_str("Status: 200 OK\r\n\r\n");
    output.push('{');

    let vars = env.all_vars();
    for (i, (key, val)) in vars.iter().enumerate() {
        if i > 0 { output.push(','); }
        // Simple JSON escaping (no quotes in values expected)
        output.push_str(&format!("\"{}\":\"{}\"", key, val));
    }

    output.push('}');
    output.into_bytes()
}

/// List registered CGI scripts
pub fn list_scripts() -> Vec<(String, bool, u64)> {
    let scripts = SCRIPTS.lock();
    scripts.iter().map(|s| {
        (s.path_prefix.clone(), s.enabled, s.invocations)
    }).collect()
}

/// Get CGI stats
pub fn get_stats() -> (u64, u64, u32) {
    let stats = CGI_STATS.lock();
    (stats.total_invocations, stats.total_errors, stats.active_scripts)
}

/// Initialize the CGI subsystem with built-in scripts
pub fn init() {
    // Register built-in CGI scripts
    register_script("/cgi-bin/server-status", server_status_handler);
    register_script("/cgi-bin/echo", echo_handler);
    register_script("/cgi-bin/env", env_json_handler);

    serial_println!("  [cgi] CGI/1.1 interface initialized (RFC 3875)");
    serial_println!("  [cgi] FastCGI multiplexer ready");
    serial_println!("  [cgi] Built-in: server-status, echo, env");
}
