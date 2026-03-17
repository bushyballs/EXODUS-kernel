/// Webserver module for Genesis — lightweight HTTP server stack
///
/// Provides a complete HTTP/1.1 server with:
///   - Request parsing and response building (http.rs)
///   - URL routing with wildcards and middleware (router.rs)
///   - Static file serving with MIME detection (static_files.rs)
///   - WebSocket protocol support (websocket.rs)
///   - CGI/FastCGI interface for dynamic content (cgi.rs)
///
/// Built for bare-metal: no_std, no libc, no filesystem assumptions.
/// All code is original. Designed for the Genesis kernel.

pub mod http;
pub mod router;
pub mod static_files;
pub mod websocket;
pub mod cgi;

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use crate::sync::Mutex;

/// Server configuration
pub struct ServerConfig {
    /// Listening port
    pub port: u16,
    /// Maximum request body size in bytes
    pub max_body_size: u32,
    /// Keep-alive timeout in seconds (Q16 fixed-point)
    pub keepalive_timeout_q16: i32,
    /// Maximum concurrent connections
    pub max_connections: u16,
    /// Enable directory listing for static files
    pub directory_listing: bool,
    /// Server name header value
    pub server_name: String,
    /// Enable WebSocket support
    pub websocket_enabled: bool,
    /// Enable CGI support
    pub cgi_enabled: bool,
}

impl ServerConfig {
    pub const fn new() -> Self {
        ServerConfig {
            port: 8080,
            max_body_size: 1_048_576,  // 1 MB
            keepalive_timeout_q16: 30 << 16,  // 30 seconds in Q16
            max_connections: 64,
            directory_listing: true,
            server_name: String::new(),
            websocket_enabled: true,
            cgi_enabled: true,
        }
    }
}

/// Connection tracking entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// Awaiting request data
    Reading,
    /// Processing the request
    Processing,
    /// Sending response
    Writing,
    /// WebSocket upgraded
    WebSocket,
    /// Connection closing
    Closing,
    /// Closed / available for reuse
    Closed,
}

/// A tracked connection
pub struct Connection {
    pub id: u32,
    pub state: ConnState,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
    pub local_port: u16,
    /// Timestamp of last activity (kernel ticks)
    pub last_active: u64,
    /// Bytes received on this connection
    pub bytes_in: u64,
    /// Bytes sent on this connection
    pub bytes_out: u64,
    /// Requests served on this keep-alive connection
    pub request_count: u32,
    /// Receive buffer
    pub recv_buf: Vec<u8>,
    /// Send buffer
    pub send_buf: Vec<u8>,
    /// Whether this connection has been upgraded to WebSocket
    pub is_websocket: bool,
}

impl Connection {
    pub fn new(id: u32, remote_ip: [u8; 4], remote_port: u16, local_port: u16) -> Self {
        Connection {
            id,
            state: ConnState::Reading,
            remote_ip,
            remote_port,
            local_port,
            last_active: 0,
            bytes_in: 0,
            bytes_out: 0,
            request_count: 0,
            recv_buf: Vec::new(),
            send_buf: Vec::new(),
            is_websocket: false,
        }
    }

    /// Reset the connection for reuse (keep-alive)
    pub fn reset_for_reuse(&mut self) {
        self.state = ConnState::Reading;
        self.recv_buf.clear();
        self.send_buf.clear();
        self.request_count = self.request_count.saturating_add(1);
    }
}

/// Server statistics
pub struct ServerStats {
    pub total_requests: u64,
    pub active_connections: u32,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub websocket_upgrades: u32,
    pub cgi_invocations: u32,
    pub errors_4xx: u32,
    pub errors_5xx: u32,
    pub uptime_ticks: u64,
}

impl ServerStats {
    pub const fn new() -> Self {
        ServerStats {
            total_requests: 0,
            active_connections: 0,
            bytes_sent: 0,
            bytes_received: 0,
            websocket_upgrades: 0,
            cgi_invocations: 0,
            errors_4xx: 0,
            errors_5xx: 0,
            uptime_ticks: 0,
        }
    }
}

/// Global server configuration
static CONFIG: Mutex<Option<ServerConfig>> = Mutex::new(None);

/// Global server statistics
static STATS: Mutex<ServerStats> = Mutex::new(ServerStats::new());

/// Active connections table
static CONNECTIONS: Mutex<Vec<Connection>> = Mutex::new(Vec::new());

/// Next connection ID counter
static NEXT_CONN_ID: Mutex<u32> = Mutex::new(1);

/// Accept a new connection and register it
pub fn accept_connection(remote_ip: [u8; 4], remote_port: u16, local_port: u16) -> u32 {
    let mut id_lock = NEXT_CONN_ID.lock();
    let id = *id_lock;
    *id_lock = id.wrapping_add(1);
    drop(id_lock);

    let conn = Connection::new(id, remote_ip, remote_port, local_port);
    CONNECTIONS.lock().push(conn);

    let mut stats = STATS.lock();
    stats.active_connections = stats.active_connections.saturating_add(1);
    drop(stats);

    serial_println!("  [webserver] connection {} from {}.{}.{}.{}:{}",
        id, remote_ip[0], remote_ip[1], remote_ip[2], remote_ip[3], remote_port);
    id
}

/// Close a connection by ID
pub fn close_connection(conn_id: u32) {
    let mut conns = CONNECTIONS.lock();
    if let Some(pos) = conns.iter().position(|c| c.id == conn_id) {
        conns.remove(pos);
        let mut stats = STATS.lock();
        if stats.active_connections > 0 {
            stats.active_connections -= 1;
        }
    }
}

/// Feed received data into a connection's buffer and attempt to process
pub fn feed_data(conn_id: u32, data: &[u8]) -> Option<Vec<u8>> {
    let mut conns = CONNECTIONS.lock();
    let conn = conns.iter_mut().find(|c| c.id == conn_id)?;

    conn.recv_buf.extend_from_slice(data);
    conn.bytes_in += data.len() as u64;

    // Check if connection is a WebSocket
    if conn.is_websocket {
        let buf = conn.recv_buf.clone();
        conn.recv_buf.clear();
        drop(conns);
        return websocket::process_frame_data(&buf, conn_id);
    }

    // Try to parse an HTTP request from the buffer
    let buf = conn.recv_buf.clone();
    drop(conns);

    if let Some(request) = http::HttpRequest::parse(&buf) {
        // Check for WebSocket upgrade
        if websocket::is_upgrade_request(&request) {
            let response = websocket::handle_upgrade(&request, conn_id);
            let response_bytes = response.to_bytes();

            let mut conns = CONNECTIONS.lock();
            if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
                conn.recv_buf.clear();
                conn.is_websocket = true;
                conn.state = ConnState::WebSocket;
                conn.bytes_out += response_bytes.len() as u64;
            }
            drop(conns);

            let mut stats = STATS.lock();
            stats.websocket_upgrades = stats.websocket_upgrades.saturating_add(1);
            stats.total_requests = stats.total_requests.saturating_add(1);

            return Some(response_bytes);
        }

        // Route the request
        let response = router::dispatch(&request);
        let response_bytes = response.to_bytes();

        let mut conns = CONNECTIONS.lock();
        if let Some(conn) = conns.iter_mut().find(|c| c.id == conn_id) {
            conn.recv_buf.clear();
            conn.bytes_out += response_bytes.len() as u64;
            conn.request_count = conn.request_count.saturating_add(1);
        }
        drop(conns);

        let mut stats = STATS.lock();
        stats.total_requests = stats.total_requests.saturating_add(1);
        stats.bytes_sent += response_bytes.len() as u64;
        let status_code = response.status as u32;
        if status_code >= 400 && status_code < 500 {
            stats.errors_4xx = stats.errors_4xx.saturating_add(1);
        } else if status_code >= 500 {
            stats.errors_5xx = stats.errors_5xx.saturating_add(1);
        }

        Some(response_bytes)
    } else {
        None  // Need more data
    }
}

/// Get current server statistics snapshot
pub fn get_stats() -> (u64, u32, u64, u64, u32) {
    let stats = STATS.lock();
    (stats.total_requests, stats.active_connections,
     stats.bytes_sent, stats.bytes_received,
     stats.websocket_upgrades)
}

/// Reap idle connections past the keepalive timeout
pub fn reap_idle_connections(current_tick: u64) {
    let timeout = {
        let cfg = CONFIG.lock();
        match cfg.as_ref() {
            Some(c) => (c.keepalive_timeout_q16 >> 16) as u64,
            None => 30,
        }
    };

    let mut conns = CONNECTIONS.lock();
    let mut to_remove = Vec::new();
    for conn in conns.iter() {
        if conn.state != ConnState::WebSocket
            && current_tick.saturating_sub(conn.last_active) > timeout
        {
            to_remove.push(conn.id);
        }
    }
    for id in &to_remove {
        if let Some(pos) = conns.iter().position(|c| c.id == *id) {
            conns.remove(pos);
            serial_println!("  [webserver] reaped idle connection {}", id);
        }
    }
    let removed = to_remove.len() as u32;
    drop(conns);

    if removed > 0 {
        let mut stats = STATS.lock();
        stats.active_connections = stats.active_connections.saturating_sub(removed);
    }
}

/// Initialize the webserver subsystem
pub fn init() {
    // Initialize sub-modules
    http::init();
    router::init();
    static_files::init();
    websocket::init();
    cgi::init();

    // Set up default configuration
    let mut cfg = ServerConfig::new();
    cfg.server_name = alloc::string::String::from("Genesis/1.0");
    *CONFIG.lock() = Some(cfg);

    serial_println!("  [webserver] Webserver subsystem initialized");
    serial_println!("  [webserver] HTTP/1.1, WebSocket, CGI, static files");
}
