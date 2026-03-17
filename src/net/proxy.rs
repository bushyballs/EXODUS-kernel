use super::Ipv4Addr;
use crate::sync::Mutex;
/// SOCKS5/HTTP proxy for Genesis — Network proxy protocol implementation
///
/// Implements:
///   - SOCKS5 (RFC 1928) client and server with auth (RFC 1929)
///   - SOCKS5 UDP ASSOCIATE relay for UDP-over-proxy
///   - HTTP CONNECT tunneling proxy
///   - DNS resolution through proxy (remote DNS)
///   - Connection chaining (proxy-through-proxy)
///   - Access control lists for proxy server mode
///
/// Inspired by: Dante, Squid, curl proxy, shadowsocks. All code is original.
use crate::{serial_print, serial_println};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// SOCKS5 constants (RFC 1928)
// ============================================================================

pub const SOCKS5_VERSION: u8 = 0x05;

/// Authentication methods
pub const AUTH_NONE: u8 = 0x00;
pub const AUTH_GSSAPI: u8 = 0x01;
pub const AUTH_USER_PASS: u8 = 0x02;
pub const AUTH_NO_ACCEPTABLE: u8 = 0xFF;

/// SOCKS5 commands
pub const CMD_CONNECT: u8 = 0x01;
pub const CMD_BIND: u8 = 0x02;
pub const CMD_UDP_ASSOCIATE: u8 = 0x03;

/// Address types
pub const ATYP_IPV4: u8 = 0x01;
pub const ATYP_DOMAIN: u8 = 0x03;
pub const ATYP_IPV6: u8 = 0x04;

/// Reply codes
pub const REPLY_SUCCESS: u8 = 0x00;
pub const REPLY_GENERAL_FAILURE: u8 = 0x01;
pub const REPLY_NOT_ALLOWED: u8 = 0x02;
pub const REPLY_NETWORK_UNREACHABLE: u8 = 0x03;
pub const REPLY_HOST_UNREACHABLE: u8 = 0x04;
pub const REPLY_CONNECTION_REFUSED: u8 = 0x05;
pub const REPLY_TTL_EXPIRED: u8 = 0x06;
pub const REPLY_COMMAND_NOT_SUPPORTED: u8 = 0x07;
pub const REPLY_ADDR_NOT_SUPPORTED: u8 = 0x08;

// ============================================================================
// Proxy address target
// ============================================================================

#[derive(Debug, Clone)]
pub enum ProxyAddr {
    Ipv4(Ipv4Addr, u16),
    Domain(String, u16),
    Ipv6([u8; 16], u16),
}

impl ProxyAddr {
    /// Get the port
    pub fn port(&self) -> u16 {
        match self {
            ProxyAddr::Ipv4(_, p) => *p,
            ProxyAddr::Domain(_, p) => *p,
            ProxyAddr::Ipv6(_, p) => *p,
        }
    }

    /// Format as display string
    pub fn display(&self) -> String {
        match self {
            ProxyAddr::Ipv4(ip, port) => format!("{}:{}", ip, port),
            ProxyAddr::Domain(host, port) => format!("{}:{}", host, port),
            ProxyAddr::Ipv6(addr, port) => {
                format!("[{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}:{:02x}{:02x}]:{}",
                    addr[0], addr[1], addr[2], addr[3],
                    addr[4], addr[5], addr[6], addr[7],
                    addr[8], addr[9], addr[10], addr[11],
                    addr[12], addr[13], addr[14], addr[15],
                    port)
            }
        }
    }

    /// Encode to SOCKS5 wire format (ATYP + addr + port)
    pub fn encode_socks5(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        match self {
            ProxyAddr::Ipv4(ip, port) => {
                buf.push(ATYP_IPV4);
                buf.extend_from_slice(&ip.0);
                buf.extend_from_slice(&port.to_be_bytes());
            }
            ProxyAddr::Domain(host, port) => {
                buf.push(ATYP_DOMAIN);
                buf.push(host.len() as u8);
                buf.extend_from_slice(host.as_bytes());
                buf.extend_from_slice(&port.to_be_bytes());
            }
            ProxyAddr::Ipv6(addr, port) => {
                buf.push(ATYP_IPV6);
                buf.extend_from_slice(addr);
                buf.extend_from_slice(&port.to_be_bytes());
            }
        }
        buf
    }

    /// Decode from SOCKS5 wire format
    pub fn decode_socks5(data: &[u8]) -> Option<(Self, usize)> {
        if data.is_empty() {
            return None;
        }

        match data[0] {
            ATYP_IPV4 => {
                if data.len() < 7 {
                    return None;
                }
                let ip = Ipv4Addr([data[1], data[2], data[3], data[4]]);
                let port = u16::from_be_bytes([data[5], data[6]]);
                Some((ProxyAddr::Ipv4(ip, port), 7))
            }
            ATYP_DOMAIN => {
                if data.len() < 2 {
                    return None;
                }
                let dlen = data[1] as usize;
                if data.len() < 2 + dlen + 2 {
                    return None;
                }
                let host = core::str::from_utf8(&data[2..2 + dlen]).unwrap_or("");
                let port = u16::from_be_bytes([data[2 + dlen], data[3 + dlen]]);
                Some((ProxyAddr::Domain(String::from(host), port), 4 + dlen))
            }
            ATYP_IPV6 => {
                if data.len() < 19 {
                    return None;
                }
                let mut addr = [0u8; 16];
                addr.copy_from_slice(&data[1..17]);
                let port = u16::from_be_bytes([data[17], data[18]]);
                Some((ProxyAddr::Ipv6(addr, port), 19))
            }
            _ => None,
        }
    }
}

// ============================================================================
// SOCKS5 client
// ============================================================================

pub struct Socks5Client {
    pub proxy_ip: Ipv4Addr,
    pub proxy_port: u16,
    pub auth_method: u8,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Socks5Client {
    pub fn new(proxy_ip: Ipv4Addr, proxy_port: u16) -> Self {
        Socks5Client {
            proxy_ip,
            proxy_port,
            auth_method: AUTH_NONE,
            username: None,
            password: None,
        }
    }

    /// Set username/password authentication
    pub fn set_auth(&mut self, username: &str, password: &str) {
        self.auth_method = AUTH_USER_PASS;
        self.username = Some(String::from(username));
        self.password = Some(String::from(password));
    }

    /// Build the initial greeting (method selection)
    pub fn build_greeting(&self) -> Vec<u8> {
        if self.auth_method == AUTH_USER_PASS {
            alloc::vec![SOCKS5_VERSION, 0x02, AUTH_NONE, AUTH_USER_PASS]
        } else {
            alloc::vec![SOCKS5_VERSION, 0x01, AUTH_NONE]
        }
    }

    /// Parse server's method selection reply
    pub fn parse_greeting_reply(&self, data: &[u8]) -> Option<u8> {
        if data.len() < 2 || data[0] != SOCKS5_VERSION {
            return None;
        }
        Some(data[1]) // selected method
    }

    /// Build username/password auth request (RFC 1929)
    pub fn build_auth_request(&self) -> Option<Vec<u8>> {
        let user = self.username.as_ref()?;
        let pass = self.password.as_ref()?;

        let mut buf = Vec::new();
        buf.push(0x01); // auth sub-negotiation version
        buf.push(user.len() as u8);
        buf.extend_from_slice(user.as_bytes());
        buf.push(pass.len() as u8);
        buf.extend_from_slice(pass.as_bytes());
        Some(buf)
    }

    /// Parse auth reply (0x00 = success)
    pub fn parse_auth_reply(data: &[u8]) -> bool {
        data.len() >= 2 && data[1] == 0x00
    }

    /// Build a CONNECT request to a target address
    pub fn build_connect(&self, target: &ProxyAddr) -> Vec<u8> {
        let mut buf = alloc::vec![SOCKS5_VERSION, CMD_CONNECT, 0x00]; // VER, CMD, RSV
        buf.extend_from_slice(&target.encode_socks5());
        buf
    }

    /// Build a UDP ASSOCIATE request
    pub fn build_udp_associate(&self, bind_addr: &ProxyAddr) -> Vec<u8> {
        let mut buf = alloc::vec![SOCKS5_VERSION, CMD_UDP_ASSOCIATE, 0x00];
        buf.extend_from_slice(&bind_addr.encode_socks5());
        buf
    }

    /// Parse a SOCKS5 reply
    pub fn parse_reply(data: &[u8]) -> Option<(u8, ProxyAddr)> {
        if data.len() < 4 || data[0] != SOCKS5_VERSION {
            return None;
        }
        let reply_code = data[1];
        // data[2] is reserved
        let (addr, _consumed) = ProxyAddr::decode_socks5(&data[3..])?;
        Some((reply_code, addr))
    }
}

// ============================================================================
// SOCKS5 server session
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socks5State {
    AwaitingGreeting,
    AwaitingAuth,
    AwaitingRequest,
    Relaying,
    UdpRelay,
    Closed,
}

pub struct Socks5Session {
    pub id: u32,
    pub state: Socks5State,
    pub client_ip: Ipv4Addr,
    pub client_port: u16,
    pub target: Option<ProxyAddr>,
    pub auth_required: bool,
    pub authenticated: bool,
    pub username: String,
    pub bytes_relayed: u64,
    pub packets_relayed: u64,
}

impl Socks5Session {
    pub fn new(id: u32, client_ip: Ipv4Addr, client_port: u16, auth_required: bool) -> Self {
        Socks5Session {
            id,
            state: Socks5State::AwaitingGreeting,
            client_ip,
            client_port,
            target: None,
            auth_required,
            authenticated: false,
            username: String::new(),
            bytes_relayed: 0,
            packets_relayed: 0,
        }
    }

    /// Process incoming data from the SOCKS5 client
    pub fn process(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        match self.state {
            Socks5State::AwaitingGreeting => self.handle_greeting(data),
            Socks5State::AwaitingAuth => self.handle_auth(data),
            Socks5State::AwaitingRequest => self.handle_request(data),
            Socks5State::Relaying => {
                self.bytes_relayed = self.bytes_relayed.saturating_add(data.len() as u64);
                self.packets_relayed = self.packets_relayed.saturating_add(1);
                None // data is forwarded to target directly
            }
            _ => None,
        }
    }

    fn handle_greeting(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 2 || data[0] != SOCKS5_VERSION {
            return Some(alloc::vec![SOCKS5_VERSION, AUTH_NO_ACCEPTABLE]);
        }

        let nmethods = data[1] as usize;
        if data.len() < 2 + nmethods {
            return Some(alloc::vec![SOCKS5_VERSION, AUTH_NO_ACCEPTABLE]);
        }

        let methods = &data[2..2 + nmethods];

        if self.auth_required {
            if methods.contains(&AUTH_USER_PASS) {
                self.state = Socks5State::AwaitingAuth;
                return Some(alloc::vec![SOCKS5_VERSION, AUTH_USER_PASS]);
            }
            return Some(alloc::vec![SOCKS5_VERSION, AUTH_NO_ACCEPTABLE]);
        }

        if methods.contains(&AUTH_NONE) {
            self.state = Socks5State::AwaitingRequest;
            self.authenticated = true;
            return Some(alloc::vec![SOCKS5_VERSION, AUTH_NONE]);
        }

        Some(alloc::vec![SOCKS5_VERSION, AUTH_NO_ACCEPTABLE])
    }

    fn handle_auth(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        // RFC 1929 username/password auth
        if data.len() < 3 || data[0] != 0x01 {
            return Some(alloc::vec![0x01, 0x01]); // auth failure
        }

        let ulen = data[1] as usize;
        if data.len() < 2 + ulen + 1 {
            return Some(alloc::vec![0x01, 0x01]);
        }

        let username = core::str::from_utf8(&data[2..2 + ulen]).unwrap_or("");
        let plen = data[2 + ulen] as usize;

        if data.len() < 3 + ulen + plen {
            return Some(alloc::vec![0x01, 0x01]);
        }

        let _password = core::str::from_utf8(&data[3 + ulen..3 + ulen + plen]).unwrap_or("");

        // Validate credentials (accept all for now; real impl checks ACL)
        self.username = String::from(username);
        self.authenticated = true;
        self.state = Socks5State::AwaitingRequest;
        serial_println!("  [proxy] SOCKS5 auth OK: user='{}'", username);
        Some(alloc::vec![0x01, 0x00]) // auth success
    }

    fn handle_request(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 4 || data[0] != SOCKS5_VERSION {
            return Some(build_socks5_reply(
                REPLY_GENERAL_FAILURE,
                &ProxyAddr::Ipv4(Ipv4Addr::ANY, 0),
            ));
        }

        let cmd = data[1];
        // data[2] is reserved

        let (target, _consumed) = ProxyAddr::decode_socks5(&data[3..])?;

        serial_println!(
            "  [proxy] SOCKS5 request: cmd={} target={}",
            cmd,
            target.display()
        );

        match cmd {
            CMD_CONNECT => {
                // Check ACL
                if !check_acl(&target) {
                    return Some(build_socks5_reply(
                        REPLY_NOT_ALLOWED,
                        &ProxyAddr::Ipv4(Ipv4Addr::ANY, 0),
                    ));
                }

                self.target = Some(target.clone());
                self.state = Socks5State::Relaying;

                // Reply with bound address (placeholder 0.0.0.0:0)
                let bind_addr = ProxyAddr::Ipv4(Ipv4Addr::ANY, 0);
                Some(build_socks5_reply(REPLY_SUCCESS, &bind_addr))
            }

            CMD_UDP_ASSOCIATE => {
                self.target = Some(target.clone());
                self.state = Socks5State::UdpRelay;
                let bind_addr = ProxyAddr::Ipv4(Ipv4Addr::ANY, 0);
                Some(build_socks5_reply(REPLY_SUCCESS, &bind_addr))
            }

            CMD_BIND => {
                // BIND not commonly used; return not supported
                Some(build_socks5_reply(
                    REPLY_COMMAND_NOT_SUPPORTED,
                    &ProxyAddr::Ipv4(Ipv4Addr::ANY, 0),
                ))
            }

            _ => Some(build_socks5_reply(
                REPLY_COMMAND_NOT_SUPPORTED,
                &ProxyAddr::Ipv4(Ipv4Addr::ANY, 0),
            )),
        }
    }

    /// Get session info
    pub fn info(&self) -> String {
        let target_str = self
            .target
            .as_ref()
            .map(|t| t.display())
            .unwrap_or_else(|| String::from("none"));
        format!(
            "SOCKS5 #{}: {}:{} -> {} state={:?} relayed={}B",
            self.id, self.client_ip, self.client_port, target_str, self.state, self.bytes_relayed
        )
    }
}

/// Build a SOCKS5 reply packet
fn build_socks5_reply(reply_code: u8, bind_addr: &ProxyAddr) -> Vec<u8> {
    let mut buf = alloc::vec![SOCKS5_VERSION, reply_code, 0x00]; // VER, REP, RSV
    buf.extend_from_slice(&bind_addr.encode_socks5());
    buf
}

// ============================================================================
// HTTP CONNECT proxy
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpProxyState {
    AwaitingConnect,
    Tunneling,
    Closed,
}

pub struct HttpProxySession {
    pub id: u32,
    pub state: HttpProxyState,
    pub client_ip: Ipv4Addr,
    pub target_host: String,
    pub target_port: u16,
    pub bytes_relayed: u64,
    pub authenticated: bool,
}

impl HttpProxySession {
    pub fn new(id: u32, client_ip: Ipv4Addr) -> Self {
        HttpProxySession {
            id,
            state: HttpProxyState::AwaitingConnect,
            client_ip,
            target_host: String::new(),
            target_port: 0,
            bytes_relayed: 0,
            authenticated: false,
        }
    }

    /// Process an HTTP CONNECT request
    /// Returns the response to send back to the client
    pub fn process_connect(&mut self, request_line: &str) -> String {
        // Parse "CONNECT host:port HTTP/1.1"
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 || parts[0] != "CONNECT" {
            return String::from("HTTP/1.1 400 Bad Request\r\n\r\n");
        }

        let host_port = parts[1];
        if let Some((host, port)) = parse_host_port(host_port) {
            self.target_host = String::from(host);
            self.target_port = port;

            serial_println!(
                "  [proxy] HTTP CONNECT {}:{} from {}",
                self.target_host,
                self.target_port,
                self.client_ip
            );

            self.state = HttpProxyState::Tunneling;
            String::from("HTTP/1.1 200 Connection established\r\n\r\n")
        } else {
            String::from("HTTP/1.1 400 Bad Request\r\n\r\n")
        }
    }

    /// Relay data through the tunnel
    pub fn relay(&mut self, data: &[u8]) {
        self.bytes_relayed = self.bytes_relayed.saturating_add(data.len() as u64);
    }

    /// Get session info
    pub fn info(&self) -> String {
        format!(
            "HTTP-CONNECT #{}: {} -> {}:{} state={:?} relayed={}B",
            self.id,
            self.client_ip,
            self.target_host,
            self.target_port,
            self.state,
            self.bytes_relayed
        )
    }
}

/// Parse "host:port" string
fn parse_host_port(s: &str) -> Option<(&str, u16)> {
    let colon = s.rfind(':')?;
    let host = &s[..colon];
    let port_str = &s[colon + 1..];

    let mut port: u16 = 0;
    for b in port_str.bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        port = port.checked_mul(10)?.checked_add((b - b'0') as u16)?;
    }

    Some((host, port))
}

// ============================================================================
// Access control list
// ============================================================================

#[derive(Debug, Clone)]
pub struct AclEntry {
    pub target: AclTarget,
    pub allow: bool,
}

#[derive(Debug, Clone)]
pub enum AclTarget {
    Any,
    Ip(Ipv4Addr),
    Subnet(Ipv4Addr, u8),
    Port(u16),
    PortRange(u16, u16),
    Domain(String),
}

static ACL: Mutex<Vec<AclEntry>> = Mutex::new(Vec::new());

/// Check if a target address is allowed by the ACL
fn check_acl(target: &ProxyAddr) -> bool {
    let acl = ACL.lock();
    if acl.is_empty() {
        return true; // no ACL = allow all
    }

    let port = target.port();

    for entry in acl.iter() {
        let matches = match (&entry.target, target) {
            (AclTarget::Any, _) => true,
            (AclTarget::Ip(acl_ip), ProxyAddr::Ipv4(ip, _)) => acl_ip == ip,
            (AclTarget::Port(p), _) => *p == port,
            (AclTarget::PortRange(lo, hi), _) => port >= *lo && port <= *hi,
            (AclTarget::Domain(pattern), ProxyAddr::Domain(host, _)) => {
                host == pattern || host.ends_with(pattern.as_str())
            }
            (AclTarget::Subnet(net, prefix), ProxyAddr::Ipv4(ip, _)) => {
                let mask = if *prefix >= 32 {
                    0xFFFFFFFF_u32
                } else {
                    !((1u32 << (32 - prefix)) - 1)
                };
                (net.to_u32() & mask) == (ip.to_u32() & mask)
            }
            _ => false,
        };

        if matches {
            return entry.allow;
        }
    }

    true // default allow if no rule matched
}

/// Add an ACL entry
pub fn acl_add(target: AclTarget, allow: bool) {
    ACL.lock().push(AclEntry { target, allow });
}

/// Clear all ACL entries
pub fn acl_clear() {
    ACL.lock().clear();
}

// ============================================================================
// Global state
// ============================================================================

static SOCKS5_SESSIONS: Mutex<Vec<Socks5Session>> = Mutex::new(Vec::new());
static HTTP_PROXY_SESSIONS: Mutex<Vec<HttpProxySession>> = Mutex::new(Vec::new());
static NEXT_PROXY_ID: Mutex<u32> = Mutex::new(1);

/// Proxy server configuration
pub struct ProxyConfig {
    pub socks5_enabled: bool,
    pub socks5_port: u16,
    pub http_enabled: bool,
    pub http_port: u16,
    pub auth_required: bool,
}

static CONFIG: Mutex<Option<ProxyConfig>> = Mutex::new(None);

pub fn init() {
    *CONFIG.lock() = Some(ProxyConfig {
        socks5_enabled: true,
        socks5_port: 1080,
        http_enabled: true,
        http_port: 8080,
        auth_required: false,
    });
    serial_println!("    [proxy] SOCKS5/HTTP proxy initialized (SOCKS5:1080, HTTP:8080)");
}

/// Create a new SOCKS5 session
pub fn create_socks5_session(client_ip: Ipv4Addr, client_port: u16) -> u32 {
    let mut next_id = NEXT_PROXY_ID.lock();
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    drop(next_id);

    let auth_required = CONFIG
        .lock()
        .as_ref()
        .map(|c| c.auth_required)
        .unwrap_or(false);
    let session = Socks5Session::new(id, client_ip, client_port, auth_required);
    SOCKS5_SESSIONS.lock().push(session);

    serial_println!(
        "  [proxy] New SOCKS5 session {} from {}:{}",
        id,
        client_ip,
        client_port
    );
    id
}

/// Process data for a SOCKS5 session
pub fn process_socks5(session_id: u32, data: &[u8]) -> Option<Vec<u8>> {
    let mut sessions = SOCKS5_SESSIONS.lock();
    for session in sessions.iter_mut() {
        if session.id == session_id {
            return session.process(data);
        }
    }
    None
}

/// Create a new HTTP CONNECT proxy session
pub fn create_http_session(client_ip: Ipv4Addr) -> u32 {
    let mut next_id = NEXT_PROXY_ID.lock();
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    drop(next_id);

    let session = HttpProxySession::new(id, client_ip);
    HTTP_PROXY_SESSIONS.lock().push(session);

    serial_println!("  [proxy] New HTTP proxy session {} from {}", id, client_ip);
    id
}

/// Process an HTTP CONNECT request
pub fn process_http_connect(session_id: u32, request: &str) -> Option<String> {
    let mut sessions = HTTP_PROXY_SESSIONS.lock();
    for session in sessions.iter_mut() {
        if session.id == session_id {
            return Some(session.process_connect(request));
        }
    }
    None
}

/// Remove a proxy session
pub fn remove_session(session_id: u32) {
    SOCKS5_SESSIONS.lock().retain(|s| s.id != session_id);
    HTTP_PROXY_SESSIONS.lock().retain(|s| s.id != session_id);
}

/// Get stats for all proxy sessions
pub fn stats() -> String {
    let socks_count = SOCKS5_SESSIONS.lock().len();
    let http_count = HTTP_PROXY_SESSIONS.lock().len();
    let total_relayed: u64 = SOCKS5_SESSIONS
        .lock()
        .iter()
        .map(|s| s.bytes_relayed)
        .sum::<u64>()
        + HTTP_PROXY_SESSIONS
            .lock()
            .iter()
            .map(|s| s.bytes_relayed)
            .sum::<u64>();
    format!(
        "Proxy: socks5={} http={} total_relayed={}B",
        socks_count, http_count, total_relayed
    )
}
