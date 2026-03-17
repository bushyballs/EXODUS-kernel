use super::Ipv4Addr;
use crate::sync::Mutex;
/// DNS resolver for Genesis
///
/// Resolves domain names to IP addresses using the DNS protocol (RFC 1035).
/// Sends queries over UDP port 53.
///
/// Features:
///   - A records (IPv4)
///   - AAAA records (IPv6)
///   - CNAME chain following (up to 8 hops)
///   - MX record parsing
///   - PTR (reverse DNS) queries
///   - DNS cache with TTL management
///   - Multiple DNS server fallback with timeout tracking
///   - Cache flush
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum CNAME hops to follow before giving up
const MAX_CNAME_HOPS: usize = 8;

/// Default cache capacity (number of entries)
const MAX_CACHE_ENTRIES: usize = 512;

/// Minimum TTL in seconds (clamp short TTLs)
const MIN_TTL_SECS: u32 = 30;

/// Maximum TTL in seconds (1 day)
const MAX_TTL_SECS: u32 = 86400;

/// Negative cache TTL in seconds (cache NXDOMAIN)
const NEGATIVE_TTL_SECS: u32 = 60;

/// DNS server timeout in ticks (ms) before trying the next server
const DNS_TIMEOUT_TICKS: u64 = 3000;

/// Maximum retries per server
const DNS_MAX_RETRIES: u32 = 2;

// ---------------------------------------------------------------------------
// DNS header
// ---------------------------------------------------------------------------

/// DNS header (12 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct DnsHeader {
    pub id: [u8; 2],
    pub flags: [u8; 2],
    pub qd_count: [u8; 2],
    pub an_count: [u8; 2],
    pub ns_count: [u8; 2],
    pub ar_count: [u8; 2],
}

// ---------------------------------------------------------------------------
// Record type constants
// ---------------------------------------------------------------------------

/// DNS record types
pub const TYPE_A: u16 = 1; // IPv4 address
pub const TYPE_NS: u16 = 2; // Name server
pub const TYPE_CNAME: u16 = 5; // Canonical name
pub const TYPE_SOA: u16 = 6; // Start of authority
pub const TYPE_PTR: u16 = 12; // Pointer (reverse DNS)
pub const TYPE_MX: u16 = 15; // Mail exchange
pub const TYPE_TXT: u16 = 16; // Text record
pub const TYPE_AAAA: u16 = 28; // IPv6 address
pub const TYPE_SRV: u16 = 33; // Service locator

/// DNS class
pub const CLASS_IN: u16 = 1; // Internet

/// DNS flags
pub const FLAG_QR: u16 = 0x8000; // Response
pub const FLAG_RD: u16 = 0x0100; // Recursion desired
pub const FLAG_RA: u16 = 0x0080; // Recursion available
pub const FLAG_TC: u16 = 0x0200; // Truncated

/// Response codes (RCODE in flags bits 0-3)
pub const RCODE_OK: u16 = 0;
pub const RCODE_FORMAT_ERROR: u16 = 1;
pub const RCODE_SERVER_FAILURE: u16 = 2;
pub const RCODE_NXDOMAIN: u16 = 3;
pub const RCODE_NOT_IMPLEMENTED: u16 = 4;
pub const RCODE_REFUSED: u16 = 5;

// ---------------------------------------------------------------------------
// DNS record types
// ---------------------------------------------------------------------------

/// A resolved DNS record
#[derive(Debug, Clone)]
pub enum DnsRecord {
    /// IPv4 address record
    A(Ipv4Addr),
    /// IPv6 address record (16 bytes)
    Aaaa([u8; 16]),
    /// Canonical name (alias)
    Cname(String),
    /// Mail exchange (priority, hostname)
    Mx(u16, String),
    /// Pointer (reverse DNS)
    Ptr(String),
    /// Name server
    Ns(String),
    /// Text record
    Txt(String),
}

// ---------------------------------------------------------------------------
// DNS resolver configuration
// ---------------------------------------------------------------------------

/// DNS resolver configuration
pub struct DnsConfig {
    /// DNS server addresses (in order of preference)
    pub servers: Vec<Ipv4Addr>,
    /// Search domains
    pub search: Vec<String>,
}

impl DnsConfig {
    /// Default config: Cloudflare + Google DNS
    pub fn default() -> Self {
        DnsConfig {
            servers: alloc::vec![
                Ipv4Addr::new(1, 1, 1, 1), // Cloudflare
                Ipv4Addr::new(8, 8, 8, 8), // Google
                Ipv4Addr::new(9, 9, 9, 9), // Quad9
            ],
            search: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// DNS cache
// ---------------------------------------------------------------------------

/// A cached DNS entry
#[derive(Clone)]
struct CacheEntry {
    /// The resolved records
    records: Vec<DnsRecord>,
    /// Tick (ms) when this entry expires
    expiry_tick: u64,
    /// Whether this is a negative cache entry (NXDOMAIN)
    negative: bool,
}

/// DNS cache: maps (name_lowercase, record_type) -> CacheEntry
/// We use a BTreeMap keyed by String for the name + type encoded.
static DNS_CACHE: Mutex<BTreeMap<String, CacheEntry>> = Mutex::new(BTreeMap::new());

/// DNS server health tracking: maps server IP (u32) -> (fail_count, last_fail_tick)
static DNS_SERVER_HEALTH: Mutex<BTreeMap<u32, (u32, u64)>> = Mutex::new(BTreeMap::new());

/// Build a cache key from name and record type
fn cache_key(name: &str, rtype: u16) -> String {
    let mut key = String::new();
    // Lowercase the name for case-insensitive lookup
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            key.push((c as u8 + 32) as char);
        } else {
            key.push(c);
        }
    }
    key.push(':');
    // Append type as decimal
    let mut tmp = [0u8; 5];
    let type_str = format_u16(rtype, &mut tmp);
    key.push_str(type_str);
    key
}

/// Format a u16 as a decimal string (no_std helper)
fn format_u16(val: u16, buf: &mut [u8; 5]) -> &str {
    let mut v = val;
    let mut i = 4;
    if v == 0 {
        buf[4] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[4..5]) };
    }
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if i == 0 {
            break;
        }
        i -= 1;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i + 1..5]) }
}

/// Look up a cached DNS entry.
pub fn cache_lookup(name: &str, rtype: u16) -> Option<Vec<DnsRecord>> {
    let key = cache_key(name, rtype);
    let now = crate::time::clock::uptime_ms();
    let cache = DNS_CACHE.lock();
    if let Some(entry) = cache.get(&key) {
        if now < entry.expiry_tick {
            if entry.negative {
                return None; // Negative cache hit — no records
            }
            return Some(entry.records.clone());
        }
    }
    None
}

/// Insert records into the DNS cache.
pub fn cache_insert(name: &str, rtype: u16, records: &[DnsRecord], ttl_secs: u32) {
    let ttl = clamp_ttl(ttl_secs);
    let now = crate::time::clock::uptime_ms();
    let key = cache_key(name, rtype);
    let mut cache = DNS_CACHE.lock();

    // Evict if cache is full
    if cache.len() >= MAX_CACHE_ENTRIES {
        // Remove the entry with the earliest expiry
        let mut earliest_key: Option<String> = None;
        let mut earliest_tick = u64::MAX;
        for (k, v) in cache.iter() {
            if v.expiry_tick < earliest_tick {
                earliest_tick = v.expiry_tick;
                earliest_key = Some(k.clone());
            }
        }
        if let Some(k) = earliest_key {
            cache.remove(&k);
        }
    }

    cache.insert(
        key,
        CacheEntry {
            records: Vec::from(records),
            expiry_tick: now + (ttl as u64) * 1000,
            negative: false,
        },
    );
}

/// Insert a negative cache entry (NXDOMAIN).
pub fn cache_insert_negative(name: &str, rtype: u16) {
    let now = crate::time::clock::uptime_ms();
    let key = cache_key(name, rtype);
    let mut cache = DNS_CACHE.lock();
    cache.insert(
        key,
        CacheEntry {
            records: Vec::new(),
            expiry_tick: now + (NEGATIVE_TTL_SECS as u64) * 1000,
            negative: true,
        },
    );
}

/// Flush the entire DNS cache.
pub fn cache_flush() {
    DNS_CACHE.lock().clear();
}

/// Flush a specific name from the cache (all record types).
pub fn cache_flush_name(name: &str) {
    let mut cache = DNS_CACHE.lock();
    let prefix = {
        let mut p = String::new();
        for c in name.chars() {
            if c.is_ascii_uppercase() {
                p.push((c as u8 + 32) as char);
            } else {
                p.push(c);
            }
        }
        p.push(':');
        p
    };
    let keys_to_remove: Vec<String> = cache
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    for key in keys_to_remove {
        cache.remove(&key);
    }
}

/// Remove expired entries from the cache.
pub fn cache_gc() {
    let now = crate::time::clock::uptime_ms();
    DNS_CACHE.lock().retain(|_, entry| now < entry.expiry_tick);
}

/// Get cache statistics: (total_entries, expired_count)
pub fn cache_stats() -> (usize, usize) {
    let now = crate::time::clock::uptime_ms();
    let cache = DNS_CACHE.lock();
    let total = cache.len();
    let expired = cache.values().filter(|e| now >= e.expiry_tick).count();
    (total, expired)
}

/// Clamp TTL to [MIN_TTL_SECS, MAX_TTL_SECS]
fn clamp_ttl(ttl: u32) -> u32 {
    if ttl < MIN_TTL_SECS {
        MIN_TTL_SECS
    } else if ttl > MAX_TTL_SECS {
        MAX_TTL_SECS
    } else {
        ttl
    }
}

// ---------------------------------------------------------------------------
// DNS server health tracking
// ---------------------------------------------------------------------------

/// Record a failure for a DNS server.
pub fn server_fail(server: Ipv4Addr) {
    let now = crate::time::clock::uptime_ms();
    let key = server.to_u32();
    let mut health = DNS_SERVER_HEALTH.lock();
    let entry = health.entry(key).or_insert((0, 0));
    entry.0 = entry.0.saturating_add(1);
    entry.1 = now;
}

/// Record a success for a DNS server (reset fail count).
pub fn server_success(server: Ipv4Addr) {
    let key = server.to_u32();
    DNS_SERVER_HEALTH.lock().remove(&key);
}

/// Get the best DNS server to use (fewest recent failures).
pub fn pick_server(servers: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    if servers.is_empty() {
        return None;
    }
    let health = DNS_SERVER_HEALTH.lock();
    let now = crate::time::clock::uptime_ms();

    let mut best: Option<Ipv4Addr> = None;
    let mut best_fails: u32 = u32::MAX;

    for &server in servers {
        let key = server.to_u32();
        let fails = if let Some(&(count, last_tick)) = health.get(&key) {
            // Decay failures: if last failure was >30s ago, halve the count
            let elapsed = now.saturating_sub(last_tick);
            if elapsed > 30_000 {
                count / 2
            } else {
                count
            }
        } else {
            0
        };
        if fails < best_fails {
            best_fails = fails;
            best = Some(server);
        }
    }

    best
}

// ---------------------------------------------------------------------------
// DNS name encoding / decoding
// ---------------------------------------------------------------------------

/// Encode a domain name into DNS wire format
///
/// "www.hoagsinc.com" -> [3, w, w, w, 8, h, o, a, g, s, i, n, c, 3, c, o, m, 0]
pub fn encode_name(name: &str) -> Vec<u8> {
    let mut result = Vec::new();
    for label in name.split('.') {
        let len = label.len();
        if len > 63 || len == 0 {
            continue; // label too long or empty
        }
        result.push(len as u8);
        result.extend_from_slice(label.as_bytes());
    }
    result.push(0); // null terminator
    result
}

/// Decode a DNS name from wire format, handling compression pointers.
/// Returns (decoded_name, bytes_consumed).
fn decode_name(data: &[u8], start: usize) -> (String, usize) {
    let mut name = String::new();
    let mut pos = start;
    let mut bytes_consumed = 0;
    let mut followed_pointer = false;
    let mut hops: u32 = 0;

    loop {
        if pos >= data.len() || hops > 64 {
            break;
        }
        hops = hops.saturating_add(1);

        let len_byte = data[pos];
        if len_byte == 0 {
            if !followed_pointer {
                bytes_consumed = pos - start + 1;
            }
            break;
        }

        // Check for compression pointer (top 2 bits = 11)
        if len_byte >= 0xC0 {
            if pos + 1 >= data.len() {
                break;
            }
            if !followed_pointer {
                bytes_consumed = pos - start + 2;
                followed_pointer = true;
            }
            let offset = ((len_byte as usize & 0x3F) << 8) | data[pos + 1] as usize;
            pos = offset;
            continue;
        }

        let label_len = len_byte as usize;
        pos += 1;
        if pos + label_len > data.len() {
            break;
        }

        if !name.is_empty() {
            name.push('.');
        }
        if let Ok(label) = core::str::from_utf8(&data[pos..pos + label_len]) {
            name.push_str(label);
        }
        pos += label_len;
    }

    if bytes_consumed == 0 && !followed_pointer {
        bytes_consumed = pos - start + 1;
    }

    (name, bytes_consumed)
}

/// Encode an IP address for reverse DNS (PTR) lookup.
/// E.g., 192.168.1.1 -> "1.1.168.192.in-addr.arpa"
pub fn encode_reverse(ip: Ipv4Addr) -> String {
    alloc::format!(
        "{}.{}.{}.{}.in-addr.arpa",
        ip.0[3],
        ip.0[2],
        ip.0[1],
        ip.0[0]
    )
}

// ---------------------------------------------------------------------------
// Query building
// ---------------------------------------------------------------------------

/// Build a DNS query packet for a specific record type.
pub fn build_query(name: &str, id: u16) -> Vec<u8> {
    build_query_type(name, id, TYPE_A)
}

/// Build a DNS query for a specific record type.
pub fn build_query_type(name: &str, id: u16, rtype: u16) -> Vec<u8> {
    let mut packet = Vec::new();

    // Header (12 bytes)
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&FLAG_RD.to_be_bytes()); // flags: recursion desired
    packet.extend_from_slice(&1u16.to_be_bytes()); // 1 question
    packet.extend_from_slice(&0u16.to_be_bytes()); // 0 answers
    packet.extend_from_slice(&0u16.to_be_bytes()); // 0 authority
    packet.extend_from_slice(&0u16.to_be_bytes()); // 0 additional

    // Question
    packet.extend_from_slice(&encode_name(name));
    packet.extend_from_slice(&rtype.to_be_bytes());
    packet.extend_from_slice(&CLASS_IN.to_be_bytes());

    packet
}

/// Build a PTR query for reverse DNS.
pub fn build_ptr_query(ip: Ipv4Addr, id: u16) -> Vec<u8> {
    let name = encode_reverse(ip);
    build_query_type(&name, id, TYPE_PTR)
}

/// Build an AAAA (IPv6) query.
pub fn build_aaaa_query(name: &str, id: u16) -> Vec<u8> {
    build_query_type(name, id, TYPE_AAAA)
}

/// Build an MX query.
pub fn build_mx_query(name: &str, id: u16) -> Vec<u8> {
    build_query_type(name, id, TYPE_MX)
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// Parse a DNS response and extract records.
/// Follows CNAME chains up to MAX_CNAME_HOPS.
pub fn parse_response(data: &[u8]) -> Vec<DnsRecord> {
    if data.len() < 12 {
        return Vec::new();
    }

    let flags = u16::from_be_bytes([data[2], data[3]]);
    let rcode = flags & 0x000F;

    // Check for errors
    if rcode != RCODE_OK {
        return Vec::new();
    }

    let an_count = u16::from_be_bytes([data[6], data[7]]) as usize;

    // Skip header (12 bytes) and question section
    let mut pos = 12;

    // Skip question section
    let qd_count = u16::from_be_bytes([data[4], data[5]]) as usize;
    for _ in 0..qd_count {
        let (_, consumed) = decode_name(data, pos);
        pos += consumed;
        pos += 4; // skip QTYPE + QCLASS
        if pos > data.len() {
            return Vec::new();
        }
    }

    // Parse all answer records
    let mut records = Vec::new();
    let total_records = an_count
        + u16::from_be_bytes([data[8], data[9]]) as usize   // NS count
        + u16::from_be_bytes([data[10], data[11]]) as usize; // AR count

    for _ in 0..total_records {
        if pos >= data.len() {
            break;
        }

        // Decode record name
        let (_rr_name, name_consumed) = decode_name(data, pos);
        pos += name_consumed;

        if pos + 10 > data.len() {
            break;
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let _rclass = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
        let _ttl = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            break;
        }

        match rtype {
            TYPE_A => {
                if rdlength == 4 {
                    records.push(DnsRecord::A(Ipv4Addr([
                        data[pos],
                        data[pos + 1],
                        data[pos + 2],
                        data[pos + 3],
                    ])));
                }
            }
            TYPE_AAAA => {
                if rdlength == 16 {
                    let mut octets = [0u8; 16];
                    octets.copy_from_slice(&data[pos..pos + 16]);
                    records.push(DnsRecord::Aaaa(octets));
                }
            }
            TYPE_CNAME => {
                let (cname, _) = decode_name(data, pos);
                records.push(DnsRecord::Cname(cname));
            }
            TYPE_MX => {
                if rdlength >= 3 {
                    let preference = u16::from_be_bytes([data[pos], data[pos + 1]]);
                    let (exchange, _) = decode_name(data, pos + 2);
                    records.push(DnsRecord::Mx(preference, exchange));
                }
            }
            TYPE_PTR => {
                let (ptr_name, _) = decode_name(data, pos);
                records.push(DnsRecord::Ptr(ptr_name));
            }
            TYPE_NS => {
                let (ns_name, _) = decode_name(data, pos);
                records.push(DnsRecord::Ns(ns_name));
            }
            TYPE_TXT => {
                // TXT records: one or more length-prefixed strings
                let mut txt = String::new();
                let mut j = pos;
                while j < pos + rdlength {
                    if j >= data.len() {
                        break;
                    }
                    let slen = data[j] as usize;
                    j += 1;
                    if j + slen > data.len() {
                        break;
                    }
                    if let Ok(s) = core::str::from_utf8(&data[j..j + slen]) {
                        txt.push_str(s);
                    }
                    j += slen;
                }
                records.push(DnsRecord::Txt(txt));
            }
            _ => {
                // Unknown record type — skip
            }
        }

        pos += rdlength;
    }

    records
}

/// Parse a DNS response with full metadata: records with TTLs.
/// Returns Vec of (name, rtype, ttl, record).
pub fn parse_response_full(data: &[u8]) -> Vec<(String, u16, u32, DnsRecord)> {
    if data.len() < 12 {
        return Vec::new();
    }

    let flags = u16::from_be_bytes([data[2], data[3]]);
    let rcode = flags & 0x000F;
    if rcode != RCODE_OK {
        return Vec::new();
    }

    let an_count = u16::from_be_bytes([data[6], data[7]]) as usize;

    let mut pos = 12;

    // Skip questions
    let qd_count = u16::from_be_bytes([data[4], data[5]]) as usize;
    for _ in 0..qd_count {
        let (_, consumed) = decode_name(data, pos);
        pos += consumed;
        pos += 4;
        if pos > data.len() {
            return Vec::new();
        }
    }

    let mut results = Vec::new();

    for _ in 0..an_count {
        if pos >= data.len() {
            break;
        }

        let (rr_name, name_consumed) = decode_name(data, pos);
        pos += name_consumed;

        if pos + 10 > data.len() {
            break;
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let ttl = u32::from_be_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            break;
        }

        let record = match rtype {
            TYPE_A if rdlength == 4 => Some(DnsRecord::A(Ipv4Addr([
                data[pos],
                data[pos + 1],
                data[pos + 2],
                data[pos + 3],
            ]))),
            TYPE_AAAA if rdlength == 16 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[pos..pos + 16]);
                Some(DnsRecord::Aaaa(octets))
            }
            TYPE_CNAME => {
                let (cname, _) = decode_name(data, pos);
                Some(DnsRecord::Cname(cname))
            }
            TYPE_MX if rdlength >= 3 => {
                let pref = u16::from_be_bytes([data[pos], data[pos + 1]]);
                let (exchange, _) = decode_name(data, pos + 2);
                Some(DnsRecord::Mx(pref, exchange))
            }
            TYPE_PTR => {
                let (ptr_name, _) = decode_name(data, pos);
                Some(DnsRecord::Ptr(ptr_name))
            }
            _ => None,
        };

        if let Some(rec) = record {
            results.push((rr_name, rtype, ttl, rec));
        }

        pos += rdlength;
    }

    results
}

/// Extract the RCODE from a DNS response.
pub fn response_rcode(data: &[u8]) -> Option<u16> {
    if data.len() < 4 {
        return None;
    }
    let flags = u16::from_be_bytes([data[2], data[3]]);
    Some(flags & 0x000F)
}

// ---------------------------------------------------------------------------
// CNAME following
// ---------------------------------------------------------------------------

/// Follow CNAME chains in a set of records.
/// Given a set of records from a DNS response, resolve CNAMEs to find
/// the final A/AAAA records. Returns the terminal records.
pub fn follow_cnames(records: &[DnsRecord]) -> Vec<DnsRecord> {
    let mut result = Vec::new();
    let mut cname_chain: Vec<String> = Vec::new();

    // First pass: collect all CNAMEs and terminal records
    for record in records {
        match record {
            DnsRecord::A(_) | DnsRecord::Aaaa(_) => {
                result.push(record.clone());
            }
            DnsRecord::Cname(name) => {
                if cname_chain.len() < MAX_CNAME_HOPS {
                    cname_chain.push(name.clone());
                }
            }
            _ => {}
        }
    }

    // If we have terminal records, return them
    if !result.is_empty() {
        return result;
    }

    // If we only have CNAMEs, the last CNAME target is what we need to resolve next
    // Return the CNAME records so the caller can issue follow-up queries
    for cname in &cname_chain {
        result.push(DnsRecord::Cname(cname.clone()));
    }

    result
}

// ---------------------------------------------------------------------------
// Transaction ID generation
// ---------------------------------------------------------------------------

/// Next DNS transaction ID
static NEXT_DNS_ID: Mutex<u16> = Mutex::new(0x1234);

/// Generate a unique DNS transaction ID.
pub fn next_id() -> u16 {
    let mut id = NEXT_DNS_ID.lock();
    let current = *id;
    *id = id.wrapping_add(1);
    if *id == 0 {
        *id = 1;
    } // Avoid 0
    current
}

// ---------------------------------------------------------------------------
// High-level resolver API
// ---------------------------------------------------------------------------

/// Default DNS server configuration.
/// Starts with Cloudflare (1.1.1.1); overridable via `set_dns_server()`.
static DNS_SERVER_IP: Mutex<[u8; 4]> = Mutex::new([1, 1, 1, 1]);

/// Override the DNS server used by `resolve_a` and `resolve_aaaa`.
pub fn set_dns_server(ip: [u8; 4]) {
    *DNS_SERVER_IP.lock() = ip;
}

/// Resolve a hostname to an IPv4 address (A record).
///
/// Checks the cache first.  On a cache miss, builds a DNS/A query, sends it
/// over UDP to the configured DNS server on port 53, polls for a response up
/// to ~100 000 spin iterations, and returns the first A record found.
///
/// Returns `None` on timeout or resolution failure.
pub fn resolve_a(hostname: &str) -> Option<[u8; 4]> {
    // --- Cache check ---
    if let Some(records) = cache_lookup(hostname, TYPE_A) {
        for rec in records {
            if let DnsRecord::A(ip) = rec {
                return Some(ip.0);
            }
        }
    }

    let tx_id = next_id();
    let query = build_query_type(hostname, tx_id, TYPE_A);
    let server_bytes = *DNS_SERVER_IP.lock();
    let dns_server = Ipv4Addr(server_bytes);

    // Bind a transient source port (ephemeral: 40000 + tx_id)
    let src_port: u16 = 40000u16.wrapping_add(tx_id);
    crate::net::udp_bind(src_port);

    if crate::net::send_udp(src_port, dns_server, 53, &query).is_err() {
        crate::serial_println!("  DNS: send_udp failed for A query ({})", hostname);
        return None;
    }

    // Poll for response
    for _ in 0..100_000u32 {
        crate::net::poll();
        if let Some((_src_ip, _src_port, data)) = crate::net::udp_recv(src_port) {
            if data.len() < 12 {
                continue;
            }
            // Validate transaction ID
            let resp_id = u16::from_be_bytes([data[0], data[1]]);
            if resp_id != tx_id {
                continue;
            }
            let records = parse_response(&data);
            for rec in &records {
                if let DnsRecord::A(ip) = rec {
                    // Cache the result using TTL from a full parse
                    let results = parse_response_full(&data);
                    let ttl = results
                        .iter()
                        .find(|(_, rtype, _, _)| *rtype == TYPE_A)
                        .map(|(_, _, ttl, _)| *ttl)
                        .unwrap_or(300);
                    cache_insert(hostname, TYPE_A, &records, ttl);
                    return Some(ip.0);
                }
            }
            // NXDOMAIN or no A record
            if let Some(rcode) = response_rcode(&data) {
                if rcode == RCODE_NXDOMAIN {
                    cache_insert_negative(hostname, TYPE_A);
                }
            }
            return None;
        }
        core::hint::spin_loop();
    }

    crate::serial_println!("  DNS: timeout resolving A record for {}", hostname);
    None
}

/// Resolve a hostname to an IPv6 address (AAAA record).
///
/// Follows the same cache-check + UDP send + poll loop as `resolve_a`.
/// Returns the raw 16-byte IPv6 address on success.
pub fn resolve_aaaa(hostname: &str) -> Option<[u8; 16]> {
    // --- Cache check ---
    if let Some(records) = cache_lookup(hostname, TYPE_AAAA) {
        for rec in records {
            if let DnsRecord::Aaaa(addr) = rec {
                return Some(addr);
            }
        }
    }

    let tx_id = next_id();
    let query = build_query_type(hostname, tx_id, TYPE_AAAA);
    let server_bytes = *DNS_SERVER_IP.lock();
    let dns_server = Ipv4Addr(server_bytes);

    let src_port: u16 = 41000u16.wrapping_add(tx_id);
    crate::net::udp_bind(src_port);

    if crate::net::send_udp(src_port, dns_server, 53, &query).is_err() {
        crate::serial_println!("  DNS: send_udp failed for AAAA query ({})", hostname);
        return None;
    }

    for _ in 0..100_000u32 {
        crate::net::poll();
        if let Some((_src_ip, _src_port, data)) = crate::net::udp_recv(src_port) {
            if data.len() < 12 {
                continue;
            }
            let resp_id = u16::from_be_bytes([data[0], data[1]]);
            if resp_id != tx_id {
                continue;
            }
            let records = parse_response(&data);
            for rec in &records {
                if let DnsRecord::Aaaa(addr) = rec {
                    let results = parse_response_full(&data);
                    let ttl = results
                        .iter()
                        .find(|(_, rtype, _, _)| *rtype == TYPE_AAAA)
                        .map(|(_, _, ttl, _)| *ttl)
                        .unwrap_or(300);
                    cache_insert(hostname, TYPE_AAAA, &records, ttl);
                    return Some(*addr);
                }
            }
            if let Some(rcode) = response_rcode(&data) {
                if rcode == RCODE_NXDOMAIN {
                    cache_insert_negative(hostname, TYPE_AAAA);
                }
            }
            return None;
        }
        core::hint::spin_loop();
    }

    crate::serial_println!("  DNS: timeout resolving AAAA record for {}", hostname);
    None
}
