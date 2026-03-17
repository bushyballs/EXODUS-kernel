use crate::sync::Mutex;
/// Simple Service Discovery Protocol (SSDP)
///
/// UPnP device/service discovery via multicast HTTP-over-UDP. Supports
/// NOTIFY (alive/byebye), M-SEARCH requests and responses, device/
/// service caching with TTL, and search target matching.
///
/// Inspired by: UPnP Device Architecture 2.0, RFC draft-cai-ssdp-v1.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// SSDP multicast address (239.255.255.250)
pub const SSDP_MULTICAST: [u8; 4] = [239, 255, 255, 250];
/// SSDP port
pub const SSDP_PORT: u16 = 1900;
/// Maximum SSDP message size
const MAX_MSG_SIZE: usize = 2048;
/// Default cache-control max-age (seconds)
const DEFAULT_MAX_AGE: u32 = 1800;

// ---------------------------------------------------------------------------
// SSDP message types
// ---------------------------------------------------------------------------

/// SSDP message type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsdpMethod {
    /// NOTIFY ssdp:alive
    NotifyAlive,
    /// NOTIFY ssdp:byebye
    NotifyByeBye,
    /// NOTIFY ssdp:update
    NotifyUpdate,
    /// M-SEARCH request
    MSearch,
    /// M-SEARCH response (HTTP/1.1 200 OK)
    MSearchResponse,
}

// ---------------------------------------------------------------------------
// SSDP headers
// ---------------------------------------------------------------------------

/// SSDP header entry
#[derive(Debug, Clone)]
pub struct SsdpHeader {
    pub name: String,
    pub value: String,
}

/// SSDP message
#[derive(Debug, Clone)]
pub struct SsdpMessage {
    pub method: SsdpMethod,
    pub headers: Vec<SsdpHeader>,
}

impl SsdpMessage {
    /// Get a header value by name (case-insensitive)
    pub fn header(&self, name: &str) -> Option<&str> {
        for h in &self.headers {
            if eq_ignore_ascii_case(&h.name, name) {
                return Some(&h.value);
            }
        }
        None
    }

    /// Get the LOCATION header
    pub fn location(&self) -> Option<&str> {
        self.header("LOCATION")
    }

    /// Get the USN header
    pub fn usn(&self) -> Option<&str> {
        self.header("USN")
    }

    /// Get the ST or NT header (search/notification target)
    pub fn target(&self) -> Option<&str> {
        self.header("ST").or_else(|| self.header("NT"))
    }

    /// Get the SERVER header
    pub fn server(&self) -> Option<&str> {
        self.header("SERVER")
    }

    /// Get the max-age from CACHE-CONTROL header
    pub fn max_age(&self) -> u32 {
        if let Some(cc) = self.header("CACHE-CONTROL") {
            // Parse "max-age=1800"
            if let Some(pos) = find_substr(cc, "max-age=") {
                let num_str = &cc[pos + 8..];
                parse_u32(num_str).unwrap_or(DEFAULT_MAX_AGE)
            } else {
                DEFAULT_MAX_AGE
            }
        } else {
            DEFAULT_MAX_AGE
        }
    }
}

/// Case-insensitive ASCII string comparison
fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes()
        .iter()
        .zip(b.as_bytes().iter())
        .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

/// Find substring position
fn find_substr(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    for i in 0..=h.len() - n.len() {
        if h[i..i + n.len()]
            .iter()
            .zip(n.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
        {
            return Some(i);
        }
    }
    None
}

/// Simple u32 parser
fn parse_u32(s: &str) -> Option<u32> {
    let mut result: u32 = 0;
    for &b in s.as_bytes() {
        if b < b'0' || b > b'9' {
            break;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    if result > 0 {
        Some(result)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Message building
// ---------------------------------------------------------------------------

/// Build a NOTIFY ssdp:alive message
pub fn build_notify_alive(
    location: &str,
    nt: &str,
    usn: &str,
    server: &str,
    max_age: u32,
) -> Vec<u8> {
    let mut msg = String::from("NOTIFY * HTTP/1.1\r\n");
    msg.push_str("HOST: 239.255.255.250:1900\r\n");
    push_header(
        &mut msg,
        "CACHE-CONTROL",
        &alloc::format!("max-age={}", max_age),
    );
    push_header(&mut msg, "LOCATION", location);
    push_header(&mut msg, "NT", nt);
    push_header(&mut msg, "NTS", "ssdp:alive");
    push_header(&mut msg, "SERVER", server);
    push_header(&mut msg, "USN", usn);
    msg.push_str("\r\n");
    msg.into_bytes()
}

/// Build a NOTIFY ssdp:byebye message
pub fn build_notify_byebye(nt: &str, usn: &str) -> Vec<u8> {
    let mut msg = String::from("NOTIFY * HTTP/1.1\r\n");
    msg.push_str("HOST: 239.255.255.250:1900\r\n");
    push_header(&mut msg, "NT", nt);
    push_header(&mut msg, "NTS", "ssdp:byebye");
    push_header(&mut msg, "USN", usn);
    msg.push_str("\r\n");
    msg.into_bytes()
}

/// Build an M-SEARCH request
pub fn build_msearch(target: &str, mx: u8) -> Vec<u8> {
    let mut msg = String::from("M-SEARCH * HTTP/1.1\r\n");
    msg.push_str("HOST: 239.255.255.250:1900\r\n");
    push_header(&mut msg, "MAN", "\"ssdp:discover\"");
    push_header(&mut msg, "MX", &alloc::format!("{}", mx));
    push_header(&mut msg, "ST", target);
    msg.push_str("\r\n");
    msg.into_bytes()
}

/// Build an M-SEARCH response
pub fn build_msearch_response(
    location: &str,
    st: &str,
    usn: &str,
    server: &str,
    max_age: u32,
) -> Vec<u8> {
    let mut msg = String::from("HTTP/1.1 200 OK\r\n");
    push_header(
        &mut msg,
        "CACHE-CONTROL",
        &alloc::format!("max-age={}", max_age),
    );
    push_header(&mut msg, "LOCATION", location);
    push_header(&mut msg, "ST", st);
    push_header(&mut msg, "SERVER", server);
    push_header(&mut msg, "USN", usn);
    msg.push_str("EXT:\r\n");
    msg.push_str("\r\n");
    msg.into_bytes()
}

fn push_header(msg: &mut String, name: &str, value: &str) {
    msg.push_str(name);
    msg.push_str(": ");
    msg.push_str(value);
    msg.push_str("\r\n");
}

// ---------------------------------------------------------------------------
// Message parsing
// ---------------------------------------------------------------------------

/// Parse an SSDP message from raw bytes
pub fn parse_message(data: &[u8]) -> Option<SsdpMessage> {
    let text = core::str::from_utf8(data).ok()?;
    let mut lines = SsdpLineIter::new(text);
    let first_line = lines.next()?;

    let method = if first_line.starts_with("NOTIFY") {
        // Will refine below based on NTS header
        SsdpMethod::NotifyAlive
    } else if first_line.starts_with("M-SEARCH") {
        SsdpMethod::MSearch
    } else if first_line.starts_with("HTTP/1.1 200") {
        SsdpMethod::MSearchResponse
    } else {
        return None;
    };

    let mut headers = Vec::new();
    while let Some(line) = lines.next() {
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim();
            let value = line[colon + 1..].trim();
            headers.push(SsdpHeader {
                name: String::from(name),
                value: String::from(value),
            });
        }
    }

    // Refine NOTIFY method based on NTS
    let method = if method == SsdpMethod::NotifyAlive {
        let nts = headers
            .iter()
            .find(|h| eq_ignore_ascii_case(&h.name, "NTS"))
            .map(|h| h.value.as_str());
        match nts {
            Some("ssdp:byebye") => SsdpMethod::NotifyByeBye,
            Some("ssdp:update") => SsdpMethod::NotifyUpdate,
            _ => SsdpMethod::NotifyAlive,
        }
    } else {
        method
    };

    Some(SsdpMessage { method, headers })
}

/// Simple line iterator for SSDP messages
struct SsdpLineIter<'a> {
    remaining: &'a str,
}

impl<'a> SsdpLineIter<'a> {
    fn new(s: &'a str) -> Self {
        SsdpLineIter { remaining: s }
    }

    fn next(&mut self) -> Option<&'a str> {
        if self.remaining.is_empty() {
            return None;
        }
        // Find \r\n or \n
        let end = self.remaining.find('\n').unwrap_or(self.remaining.len());
        let line = if end > 0 && self.remaining.as_bytes().get(end - 1) == Some(&b'\r') {
            &self.remaining[..end - 1]
        } else {
            &self.remaining[..end]
        };
        self.remaining = if end < self.remaining.len() {
            &self.remaining[end + 1..]
        } else {
            ""
        };
        Some(line)
    }
}

// ---------------------------------------------------------------------------
// Device cache
// ---------------------------------------------------------------------------

/// Cached SSDP device/service entry
#[derive(Debug, Clone)]
pub struct SsdpDevice {
    pub location: String,
    pub usn: String,
    pub target: String,
    pub server: String,
    /// Time-to-live in ticks
    pub ttl: u32,
    /// Remaining ticks before expiry
    pub remaining: u32,
}

/// SSDP service (listener + cache)
pub struct SsdpService {
    /// Cached devices/services discovered
    pub cache: Vec<SsdpDevice>,
    /// Our own advertised services
    pub advertised: Vec<SsdpDevice>,
    /// Pending outgoing messages
    pub outbox: Vec<Vec<u8>>,
    /// Tick counter
    pub tick: u64,
}

impl SsdpService {
    fn new() -> Self {
        SsdpService {
            cache: Vec::new(),
            advertised: Vec::new(),
            outbox: Vec::new(),
            tick: 0,
        }
    }

    /// Process a received SSDP message
    pub fn process_message(&mut self, msg: &SsdpMessage) {
        match msg.method {
            SsdpMethod::NotifyAlive | SsdpMethod::MSearchResponse => {
                let location = msg.location().unwrap_or("").into();
                let usn = msg.usn().unwrap_or("").into();
                let target = msg.target().unwrap_or("").into();
                let server = msg.server().unwrap_or("").into();
                let max_age = msg.max_age();

                // Update existing or add new
                if let Some(dev) = self.cache.iter_mut().find(|d| d.usn == usn) {
                    dev.location = location;
                    dev.target = target;
                    dev.server = server;
                    dev.ttl = max_age;
                    dev.remaining = max_age;
                } else {
                    self.cache.push(SsdpDevice {
                        location,
                        usn,
                        target,
                        server,
                        ttl: max_age,
                        remaining: max_age,
                    });
                }
            }
            SsdpMethod::NotifyByeBye => {
                if let Some(usn) = msg.usn() {
                    self.cache.retain(|d| d.usn != usn);
                }
            }
            SsdpMethod::MSearch => {
                // Respond with matching advertised services
                if let Some(st) = msg.target() {
                    for adv in &self.advertised {
                        if st == "ssdp:all" || st == adv.target {
                            let resp = build_msearch_response(
                                &adv.location,
                                &adv.target,
                                &adv.usn,
                                &adv.server,
                                adv.ttl,
                            );
                            self.outbox.push(resp);
                        }
                    }
                }
            }
            SsdpMethod::NotifyUpdate => {
                // Treat like alive
                if let Some(usn) = msg.usn() {
                    if let Some(dev) = self.cache.iter_mut().find(|d| d.usn == usn) {
                        if let Some(loc) = msg.location() {
                            dev.location = String::from(loc);
                        }
                        dev.remaining = msg.max_age();
                    }
                }
            }
        }
    }

    /// Send an M-SEARCH for a given target
    pub fn search(&mut self, target: &str, mx: u8) {
        let msg = build_msearch(target, mx);
        self.outbox.push(msg);
    }

    /// Advertise a service (sends NOTIFY alive)
    pub fn advertise(&mut self, location: &str, nt: &str, usn: &str, server: &str, max_age: u32) {
        self.advertised.push(SsdpDevice {
            location: String::from(location),
            usn: String::from(usn),
            target: String::from(nt),
            server: String::from(server),
            ttl: max_age,
            remaining: max_age,
        });
        let msg = build_notify_alive(location, nt, usn, server, max_age);
        self.outbox.push(msg);
    }

    /// Send byebye for all advertised services
    pub fn unadvertise_all(&mut self) {
        for adv in &self.advertised {
            let msg = build_notify_byebye(&adv.target, &adv.usn);
            self.outbox.push(msg);
        }
        self.advertised.clear();
    }

    /// Dequeue next outgoing message
    pub fn dequeue_outgoing(&mut self) -> Option<Vec<u8>> {
        if self.outbox.is_empty() {
            None
        } else {
            Some(self.outbox.remove(0))
        }
    }

    /// Tick: age cache entries, re-advertise as needed
    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1);
        // Expire stale entries
        self.cache.retain_mut(|d| {
            if d.remaining > 0 {
                d.remaining = d.remaining.saturating_sub(1);
                true
            } else {
                false
            }
        });
    }

    /// Find cached devices matching a target
    pub fn find(&self, target: &str) -> Vec<&SsdpDevice> {
        self.cache
            .iter()
            .filter(|d| target == "ssdp:all" || d.target == target)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Global subsystem
// ---------------------------------------------------------------------------

static SSDP: Mutex<Option<SsdpService>> = Mutex::new(None);

pub fn init() {
    *SSDP.lock() = Some(SsdpService::new());
    serial_println!("  Net: SSDP subsystem initialized");
}
