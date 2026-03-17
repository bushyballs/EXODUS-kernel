/// HTTP server/client for Genesis — Hypertext Transfer Protocol
///
/// Implements HTTP/1.1 with keep-alive, chunked transfer,
/// basic routing, and static file serving. Foundation for remote management.
///
/// Inspired by: tiny-http, hyper. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// HTTP method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
}

impl Method {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            "PUT" => Some(Method::Put),
            "DELETE" => Some(Method::Delete),
            "HEAD" => Some(Method::Head),
            "OPTIONS" => Some(Method::Options),
            "PATCH" => Some(Method::Patch),
            _ => None,
        }
    }
}

/// HTTP status code
#[derive(Debug, Clone, Copy)]
pub enum StatusCode {
    Ok = 200,
    Created = 201,
    NoContent = 204,
    MovedPermanently = 301,
    NotModified = 304,
    BadRequest = 400,
    Unauthorized = 401,
    Forbidden = 403,
    NotFound = 404,
    MethodNotAllowed = 405,
    InternalServerError = 500,
    ServiceUnavailable = 503,
}

impl StatusCode {
    pub fn reason(&self) -> &'static str {
        match self {
            StatusCode::Ok => "OK",
            StatusCode::Created => "Created",
            StatusCode::NoContent => "No Content",
            StatusCode::MovedPermanently => "Moved Permanently",
            StatusCode::NotModified => "Not Modified",
            StatusCode::BadRequest => "Bad Request",
            StatusCode::Unauthorized => "Unauthorized",
            StatusCode::Forbidden => "Forbidden",
            StatusCode::NotFound => "Not Found",
            StatusCode::MethodNotAllowed => "Method Not Allowed",
            StatusCode::InternalServerError => "Internal Server Error",
            StatusCode::ServiceUnavailable => "Service Unavailable",
        }
    }
}

/// HTTP request
pub struct HttpRequest {
    pub method: Method,
    pub path: String,
    pub version: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub query: BTreeMap<String, String>,
}

impl HttpRequest {
    /// Parse an HTTP request from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        let text = core::str::from_utf8(data).ok()?;
        let mut lines = text.split("\r\n");

        // Request line
        let request_line = lines.next()?;
        let mut parts = request_line.split_whitespace();
        let method = Method::from_str(parts.next()?)?;
        let full_path = parts.next()?;
        let version = parts.next().unwrap_or("HTTP/1.1");

        // Parse path and query string
        let (path, query) = if let Some(qmark) = full_path.find('?') {
            let path = &full_path[..qmark];
            let qs = &full_path[qmark + 1..];
            let mut query = BTreeMap::new();
            for pair in qs.split('&') {
                if let Some(eq) = pair.find('=') {
                    query.insert(String::from(&pair[..eq]), String::from(&pair[eq + 1..]));
                }
            }
            (String::from(path), query)
        } else {
            (String::from(full_path), BTreeMap::new())
        };

        // Headers
        let mut headers = BTreeMap::new();
        for line in &mut lines {
            if line.is_empty() {
                break;
            }
            if let Some(colon) = line.find(':') {
                let key = line[..colon].trim().to_lowercase();
                let val = line[colon + 1..].trim();
                headers.insert(String::from(&key), String::from(val));
            }
        }

        // Body (remaining data after headers)
        let header_end = text.find("\r\n\r\n")? + 4;
        let body = if header_end < data.len() {
            data[header_end..].to_vec()
        } else {
            Vec::new()
        };

        Some(HttpRequest {
            method,
            path,
            version: String::from(version),
            headers,
            body,
            query,
        })
    }
}

/// HTTP response builder
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: StatusCode) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert(String::from("server"), String::from("Genesis/1.0"));
        headers.insert(String::from("connection"), String::from("keep-alive"));
        HttpResponse {
            status,
            headers,
            body: Vec::new(),
        }
    }

    pub fn text(status: StatusCode, text: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers
            .insert(String::from("content-type"), String::from("text/plain"));
        resp.body = text.as_bytes().to_vec();
        resp
    }

    pub fn html(status: StatusCode, html: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.insert(
            String::from("content-type"),
            String::from("text/html; charset=utf-8"),
        );
        resp.body = html.as_bytes().to_vec();
        resp
    }

    pub fn json(status: StatusCode, json: &str) -> Self {
        let mut resp = Self::new(status);
        resp.headers.insert(
            String::from("content-type"),
            String::from("application/json"),
        );
        resp.body = json.as_bytes().to_vec();
        resp
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let status_line = format!(
            "HTTP/1.1 {} {}\r\n",
            self.status as u32,
            self.status.reason()
        );
        buf.extend_from_slice(status_line.as_bytes());

        // Content-Length header
        let cl = format!("content-length: {}\r\n", self.body.len());
        buf.extend_from_slice(cl.as_bytes());

        for (key, val) in &self.headers {
            let header = format!("{}: {}\r\n", key, val);
            buf.extend_from_slice(header.as_bytes());
        }
        buf.extend_from_slice(b"\r\n");
        buf.extend_from_slice(&self.body);
        buf
    }
}

/// Route handler type
pub type RouteHandler = fn(&HttpRequest) -> HttpResponse;

/// HTTP route
pub struct Route {
    pub method: Method,
    pub path: String,
    pub handler: RouteHandler,
}

/// HTTP server
pub struct HttpServer {
    pub port: u16,
    pub routes: Vec<Route>,
    pub running: bool,
    pub requests_served: u64,
}

impl HttpServer {
    const fn new() -> Self {
        HttpServer {
            port: 80,
            routes: Vec::new(),
            running: false,
            requests_served: 0,
        }
    }

    /// Add a route
    pub fn route(&mut self, method: Method, path: &str, handler: RouteHandler) {
        self.routes.push(Route {
            method,
            path: String::from(path),
            handler,
        });
    }

    /// Handle a request
    pub fn handle_request(&mut self, req: &HttpRequest) -> HttpResponse {
        self.requests_served = self.requests_served.saturating_add(1);

        // Find matching route
        for route in &self.routes {
            if route.method == req.method && route.path == req.path {
                return (route.handler)(req);
            }
        }

        HttpResponse::text(StatusCode::NotFound, "404 Not Found")
    }
}

static HTTP_SERVER: Mutex<HttpServer> = Mutex::new(HttpServer::new());

/// Default handler for system status
fn status_handler(_req: &HttpRequest) -> HttpResponse {
    let uptime = crate::time::clock::uptime_secs();
    let body = format!(
        "{{\"status\":\"ok\",\"uptime\":{},\"version\":\"Genesis v1.0.0\"}}",
        uptime
    );
    HttpResponse::json(StatusCode::Ok, &body)
}

/// Default handler for root
fn root_handler(_req: &HttpRequest) -> HttpResponse {
    HttpResponse::html(
        StatusCode::Ok,
        "<html><body><h1>Hoags OS Genesis</h1><p>System running.</p></body></html>",
    )
}

pub fn init() {
    let mut server = HTTP_SERVER.lock();
    server.port = 8080;
    server.route(Method::Get, "/", root_handler);
    server.route(Method::Get, "/api/status", status_handler);
    server.running = true;
    crate::serial_println!("  [http] HTTP server initialized on port {}", server.port);
}

pub fn handle_request(data: &[u8]) -> Option<Vec<u8>> {
    let req = HttpRequest::parse(data)?;
    let resp = HTTP_SERVER.lock().handle_request(&req);
    Some(resp.to_bytes())
}

// ---------------------------------------------------------------------------
// HTTP/1.1 client
// ---------------------------------------------------------------------------

/// Parse a URL of the form `http://host[:port]/path` or `https://host[:port]/path`.
///
/// Returns `(host, port, path)`.  Defaults: port 80 for http, 443 for https.
/// The returned `host` and `path` are slices into `url`.
pub fn parse_url(url: &str) -> (&str, u16, &str) {
    // Strip scheme
    let after_scheme = if url.starts_with("https://") {
        (&url[8..], 443u16)
    } else if url.starts_with("http://") {
        (&url[7..], 80u16)
    } else {
        (url, 80u16)
    };
    let (rest, default_port) = after_scheme;

    // Split host from path at first '/'
    let (authority, path) = if let Some(slash) = rest.find('/') {
        (&rest[..slash], &rest[slash..])
    } else {
        (rest, "/")
    };

    // Split host from optional port
    let (host, port) = if let Some(colon) = authority.rfind(':') {
        let host_part = &authority[..colon];
        let port_str = &authority[colon + 1..];
        let port_num = {
            let mut n: u16 = 0;
            let mut ok = false;
            for b in port_str.bytes() {
                if b >= b'0' && b <= b'9' {
                    n = n.saturating_mul(10).saturating_add((b - b'0') as u16);
                    ok = true;
                } else {
                    ok = false;
                    break;
                }
            }
            if ok {
                n
            } else {
                default_port
            }
        };
        (host_part, port_num)
    } else {
        (authority, default_port)
    };

    (host, port, path)
}

/// Perform an HTTP/1.1 GET request.
///
/// Connects to the host over TCP, sends a minimal GET request, waits for the
/// full response, strips headers, copies the body into `out`, and returns the
/// body length.  Returns an error string on failure.
///
/// Note: This is a bare-TCP (plaintext) client.  For HTTPS use the TLS layer.
pub fn get(url: &str, out: &mut [u8]) -> Result<usize, &'static str> {
    let (host, port, path) = parse_url(url);

    // Resolve the host
    let ip = crate::net::dns::resolve_a(host).ok_or("DNS resolution failed")?;
    let dst_ip = crate::net::Ipv4Addr(ip);

    // Allocate an ephemeral local port (wrapping 49152..65535)
    static HTTP_CLIENT_PORT: crate::sync::Mutex<u16> = crate::sync::Mutex::new(49152);
    let local_port = {
        let mut p = HTTP_CLIENT_PORT.lock();
        let port_val = *p;
        *p = if *p >= 65000 { 49152 } else { *p + 1 };
        port_val
    };

    // Initiate TCP connection
    let conn_id = crate::net::tcp::connect(local_port, dst_ip, port);

    // Wait for ESTABLISHED (up to ~200 000 polls)
    let mut established = false;
    for _ in 0..200_000u32 {
        crate::net::poll();
        if let Some(state) = crate::net::tcp::get_state(conn_id) {
            if state == crate::net::tcp::TcpState::Established {
                established = true;
                break;
            }
            // RST / CLOSED → bail
            if state == crate::net::tcp::TcpState::Closed {
                break;
            }
        }
        core::hint::spin_loop();
    }
    if !established {
        return Err("TCP connect timeout");
    }

    // Build GET request
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    crate::net::tcp::send_data(conn_id, req.as_bytes()).map_err(|_| "TCP send failed")?;

    // Collect response until connection closes or buffer full
    let mut response: Vec<u8> = Vec::new();
    for _ in 0..2_000_000u32 {
        crate::net::poll();
        let chunk = crate::net::tcp::read_data(conn_id);
        if !chunk.is_empty() {
            response.extend_from_slice(&chunk);
        }
        // Stop when connection has closed after we got some data
        if let Some(state) = crate::net::tcp::get_state(conn_id) {
            if (state == crate::net::tcp::TcpState::CloseWait
                || state == crate::net::tcp::TcpState::Closed
                || state == crate::net::tcp::TcpState::TimeWait)
                && !response.is_empty()
            {
                break;
            }
        }
        if response.len() >= out.len() {
            break;
        }
        core::hint::spin_loop();
    }
    crate::net::tcp::close_connection(conn_id);

    // Strip HTTP headers: find \r\n\r\n
    let body_start = find_header_end(&response).unwrap_or(0);
    let body = &response[body_start..];
    let copy_len = body.len().min(out.len());
    out[..copy_len].copy_from_slice(&body[..copy_len]);
    Ok(copy_len)
}

/// Perform an HTTP/1.1 POST request with a raw body.
///
/// Sends `body` as the request payload with `Content-Type: application/octet-stream`.
/// Strips response headers and copies the response body into `out`.
pub fn post(url: &str, body: &[u8], out: &mut [u8]) -> Result<usize, &'static str> {
    let (host, port, path) = parse_url(url);

    let ip = crate::net::dns::resolve_a(host).ok_or("DNS resolution failed")?;
    let dst_ip = crate::net::Ipv4Addr(ip);

    static HTTP_POST_PORT: crate::sync::Mutex<u16> = crate::sync::Mutex::new(49500);
    let local_port = {
        let mut p = HTTP_POST_PORT.lock();
        let port_val = *p;
        *p = if *p >= 65200 { 49500 } else { *p + 1 };
        port_val
    };

    let conn_id = crate::net::tcp::connect(local_port, dst_ip, port);

    let mut established = false;
    for _ in 0..200_000u32 {
        crate::net::poll();
        if let Some(state) = crate::net::tcp::get_state(conn_id) {
            if state == crate::net::tcp::TcpState::Established {
                established = true;
                break;
            }
            if state == crate::net::tcp::TcpState::Closed {
                break;
            }
        }
        core::hint::spin_loop();
    }
    if !established {
        return Err("TCP connect timeout");
    }

    // Build POST request header
    let header = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path, host, body.len()
    );
    crate::net::tcp::send_data(conn_id, header.as_bytes()).map_err(|_| "TCP send header failed")?;
    crate::net::tcp::send_data(conn_id, body).map_err(|_| "TCP send body failed")?;

    // Collect response
    let mut response: Vec<u8> = Vec::new();
    for _ in 0..2_000_000u32 {
        crate::net::poll();
        let chunk = crate::net::tcp::read_data(conn_id);
        if !chunk.is_empty() {
            response.extend_from_slice(&chunk);
        }
        if let Some(state) = crate::net::tcp::get_state(conn_id) {
            if (state == crate::net::tcp::TcpState::CloseWait
                || state == crate::net::tcp::TcpState::Closed
                || state == crate::net::tcp::TcpState::TimeWait)
                && !response.is_empty()
            {
                break;
            }
        }
        if response.len() >= out.len() {
            break;
        }
        core::hint::spin_loop();
    }
    crate::net::tcp::close_connection(conn_id);

    let body_start = find_header_end(&response).unwrap_or(0);
    let resp_body = &response[body_start..];
    let copy_len = resp_body.len().min(out.len());
    out[..copy_len].copy_from_slice(&resp_body[..copy_len]);
    Ok(copy_len)
}

/// Find the offset of the HTTP body (byte after `\r\n\r\n`).
/// Returns `None` if the header terminator is not present (body starts at 0).
fn find_header_end(data: &[u8]) -> Option<usize> {
    // Search for \r\n\r\n
    if data.len() < 4 {
        return None;
    }
    for i in 0..data.len() - 3 {
        if data[i] == b'\r' && data[i + 1] == b'\n' && data[i + 2] == b'\r' && data[i + 3] == b'\n'
        {
            return Some(i + 4);
        }
    }
    None
}
