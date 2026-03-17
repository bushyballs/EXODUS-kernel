/// Multicast DNS (RFC 6762) and DNS-SD (RFC 6763)
///
/// Zero-configuration name resolution for the .local domain.
/// Operates on multicast group 224.0.0.251, port 5353.
///
/// Design constraints (bare-metal kernel rules):
///   - no_std — no standard library
///   - No heap — no Vec / Box / String — all fixed-size static arrays
///   - No float casts (as f32 / as f64)
///   - Saturating arithmetic on counters, wrapping_add on sequences
///   - No panic — all fallible paths return early or log + return
///
/// Inspired by: RFC 6762 (mDNS), RFC 6763 (DNS-SD). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants — RFC 6762 §11, §15
// ---------------------------------------------------------------------------

/// mDNS multicast group address 224.0.0.251
pub const MDNS_MULTICAST_ADDR: [u8; 4] = [224, 0, 0, 251];

/// mDNS well-known port (UDP)
pub const MDNS_PORT: u16 = 5353;

/// IP TTL for mDNS packets MUST be 255 (RFC 6762 §11.4)
pub const MDNS_TTL: u32 = 255;

/// Default record TTL: 4500 s (75 minutes, RFC 6762 §11.3)
pub const DEFAULT_RECORD_TTL: u32 = 4500;

// DNS record types (RFC 1035 §3.2.2)
pub const RR_A: u16 = 1; // IPv4 address
pub const RR_PTR: u16 = 12; // Pointer / reverse lookup
pub const RR_TXT: u16 = 16; // Text record
pub const RR_AAAA: u16 = 28; // IPv6 address
pub const RR_SRV: u16 = 33; // Service location
pub const RR_ANY: u16 = 255; // Any record

/// DNS Internet class
pub const CLASS_IN: u16 = 1;

/// Cache-flush bit OR'd into class field for unique records (RFC 6762 §11.3)
pub const CLASS_IN_FLUSH: u16 = 0x8001;

// DNS header flags
const FLAGS_QR_RESPONSE: u16 = 0x8400; // QR=1, AA=1

// Capacity limits — adjust here if needed
const MAX_RECORDS: usize = 32; // owned/advertised records
const CACHE_SIZE: usize = 64; // received record cache
const MAX_HOSTNAME: usize = 128; // bytes (null-terminated DNS wire)
const MAX_RDATA: usize = 256; // bytes

// ---------------------------------------------------------------------------
// DNS name encoder / decoder
// ---------------------------------------------------------------------------

/// Encode a dot-separated name into DNS wire-format label sequence.
///
/// "myhost.local" → `\x06myhost\x05local\x00`
///
/// Writes into `buf[..out_len]` and returns the number of bytes written.
/// Returns 0 if the name is too long to fit in `buf`.
///
/// # Safety
/// No unsafe code; all bounds are checked.
pub fn encode_name(name: &str, buf: &mut [u8]) -> usize {
    let mut pos = 0usize;
    let name_bytes = name.as_bytes();
    let mut label_start = 0usize;

    let mut i = 0usize;
    while i <= name_bytes.len() {
        let is_dot_or_end = i == name_bytes.len() || name_bytes[i] == b'.';
        if is_dot_or_end {
            let label_len = i - label_start;
            // Skip empty labels (e.g. trailing dot, double dot)
            if label_len > 0 {
                if label_len > 63 {
                    // Label too long — truncate the whole encoding
                    return 0;
                }
                if pos.saturating_add(1 + label_len) >= buf.len() {
                    return 0;
                }
                buf[pos] = label_len as u8;
                pos = pos.saturating_add(1);
                buf[pos..pos + label_len]
                    .copy_from_slice(&name_bytes[label_start..label_start + label_len]);
                pos = pos.saturating_add(label_len);
            }
            label_start = i.saturating_add(1);
        }
        i = i.saturating_add(1);
    }

    // Root label terminator
    if pos >= buf.len() {
        return 0;
    }
    buf[pos] = 0;
    pos.saturating_add(1)
}

/// Decode a DNS wire-format name from `buf` starting at `offset`.
///
/// Follows RFC 1035 §3.1 compression pointers (up to 64 hops).
/// Writes a null-terminated ASCII string into `out` and returns the number of
/// bytes consumed in `buf` at `offset` (does NOT count pointer destinations).
/// Returns 0 on error.
pub fn decode_name(buf: &[u8], offset: usize, out: &mut [u8]) -> usize {
    let mut pos = offset;
    let mut out_pos = 0usize;
    let mut consumed = 0usize; // bytes consumed at the original offset
    let mut jumped = false;
    let mut depth = 0u8;

    loop {
        if pos >= buf.len() || depth > 64 {
            return 0;
        }
        depth = depth.saturating_add(1);

        let byte = buf[pos];

        if byte == 0 {
            // Root label — end of name
            if !jumped {
                consumed = pos.saturating_sub(offset).saturating_add(1);
            }
            break;
        }

        // Compression pointer: top two bits = 11
        if byte & 0xC0 == 0xC0 {
            if pos.saturating_add(1) >= buf.len() {
                return 0;
            }
            if !jumped {
                consumed = pos.saturating_sub(offset).saturating_add(2);
                jumped = true;
            }
            let ptr = (((byte & 0x3F) as usize) << 8) | (buf[pos.saturating_add(1)] as usize);
            pos = ptr;
            continue;
        }

        // Normal label
        let label_len = byte as usize;
        pos = pos.saturating_add(1);
        if pos.saturating_add(label_len) > buf.len() {
            return 0;
        }

        // Insert dot separator (except before the first label)
        if out_pos > 0 {
            if out_pos >= out.len().saturating_sub(1) {
                return 0;
            }
            out[out_pos] = b'.';
            out_pos = out_pos.saturating_add(1);
        }

        // Copy label bytes
        if out_pos.saturating_add(label_len) >= out.len() {
            return 0;
        }
        out[out_pos..out_pos + label_len].copy_from_slice(&buf[pos..pos + label_len]);
        out_pos = out_pos.saturating_add(label_len);
        pos = pos.saturating_add(label_len);
    }

    // Null-terminate
    if out_pos < out.len() {
        out[out_pos] = 0;
    }

    if consumed == 0 && !jumped {
        consumed = pos.saturating_sub(offset).saturating_add(1);
    }
    consumed
}

// ---------------------------------------------------------------------------
// Compare a decoded (null-terminated) name to a str (case-insensitive)
// ---------------------------------------------------------------------------

/// Compare a null-terminated byte slice to a &str, case-insensitive ASCII.
fn name_eq(decoded: &[u8], other: &str) -> bool {
    let other_b = other.as_bytes();
    // Find the null terminator length
    let mut dec_len = 0usize;
    while dec_len < decoded.len() && decoded[dec_len] != 0 {
        dec_len = dec_len.saturating_add(1);
    }
    if dec_len != other_b.len() {
        return false;
    }
    let mut i = 0usize;
    while i < dec_len {
        let a = if decoded[i].is_ascii_uppercase() {
            decoded[i] | 0x20
        } else {
            decoded[i]
        };
        let b = if other_b[i].is_ascii_uppercase() {
            other_b[i] | 0x20
        } else {
            other_b[i]
        };
        if a != b {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// DNS packet builder helpers
// ---------------------------------------------------------------------------

/// Write a big-endian u16 into buf at pos; returns pos + 2 or 0 on overflow.
#[inline]
fn write_u16(buf: &mut [u8], pos: usize, val: u16) -> usize {
    if pos.saturating_add(2) > buf.len() {
        return 0;
    }
    buf[pos] = (val >> 8) as u8;
    buf[pos + 1] = val as u8;
    pos.saturating_add(2)
}

/// Write a big-endian u32 into buf at pos; returns pos + 4 or 0 on overflow.
#[inline]
fn write_u32(buf: &mut [u8], pos: usize, val: u32) -> usize {
    if pos.saturating_add(4) > buf.len() {
        return 0;
    }
    buf[pos] = (val >> 24) as u8;
    buf[pos + 1] = (val >> 16) as u8;
    buf[pos + 2] = (val >> 8) as u8;
    buf[pos + 3] = val as u8;
    pos.saturating_add(4)
}

/// Read a big-endian u16 from buf at pos. Returns 0 on bounds error.
#[inline]
fn read_u16(buf: &[u8], pos: usize) -> u16 {
    if pos.saturating_add(2) > buf.len() {
        return 0;
    }
    u16::from_be_bytes([buf[pos], buf[pos + 1]])
}

/// Read a big-endian u32 from buf at pos. Returns 0 on bounds error.
#[inline]
fn read_u32(buf: &[u8], pos: usize) -> u32 {
    if pos.saturating_add(4) > buf.len() {
        return 0;
    }
    u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]])
}

/// Build an mDNS query message for `name` / `qtype` into `buf`.
///
/// DNS header: ID=0, QR=0, OPCODE=0, RD=0 (RFC 6762 §6)
/// Question class is CLASS_IN with unicast-response bit (0x8000) set.
/// Returns the number of bytes written, or 0 on error.
pub fn build_query(name: &str, qtype: u16, buf: &mut [u8]) -> usize {
    if buf.len() < 12 {
        return 0;
    }
    // Header
    let mut pos = 0usize;
    pos = write_u16(buf, pos, 0); // ID = 0
    pos = write_u16(buf, pos, 0); // flags = 0 (standard query)
    pos = write_u16(buf, pos, 1); // QDCOUNT = 1
    pos = write_u16(buf, pos, 0); // ANCOUNT = 0
    pos = write_u16(buf, pos, 0); // NSCOUNT = 0
    pos = write_u16(buf, pos, 0); // ARCOUNT = 0
    if pos == 0 {
        return 0;
    }

    // Question: QNAME
    let name_len = encode_name(name, &mut buf[pos..]);
    if name_len == 0 {
        return 0;
    }
    pos = pos.saturating_add(name_len);

    // QTYPE
    pos = write_u16(buf, pos, qtype);
    // QCLASS = IN | unicast-response bit (RFC 6762 §5.4)
    pos = write_u16(buf, pos, CLASS_IN | 0x8000);
    if pos == 0 {
        return 0;
    }

    pos
}

/// Build an mDNS response containing `records[..count]` into `buf`.
///
/// DNS header: ID=0, QR=1, AA=1 (RFC 6762 §6)
/// Returns the number of bytes written, or 0 on error.
pub fn build_response(records: &[MdnsRecord], count: usize, buf: &mut [u8]) -> usize {
    if buf.len() < 12 || count == 0 {
        return 0;
    }
    let answer_count = count.min(records.len());

    let mut pos = 0usize;
    pos = write_u16(buf, pos, 0); // ID = 0
    pos = write_u16(buf, pos, FLAGS_QR_RESPONSE); // QR=1, AA=1
    pos = write_u16(buf, pos, 0); // QDCOUNT = 0
    pos = write_u16(buf, pos, answer_count as u16); // ANCOUNT
    pos = write_u16(buf, pos, 0); // NSCOUNT = 0
    pos = write_u16(buf, pos, 0); // ARCOUNT = 0
    if pos == 0 {
        return 0;
    }

    for i in 0..answer_count {
        let rec = &records[i];
        if !rec.active {
            continue;
        }

        // NAME
        let name_len = encode_name(
            core::str::from_utf8(&rec.name[..rec.name_len]).unwrap_or(""),
            &mut buf[pos..],
        );
        if name_len == 0 {
            return pos;
        }
        pos = pos.saturating_add(name_len);

        // TYPE, CLASS (with flush bit for unique records), TTL, RDLENGTH
        pos = write_u16(buf, pos, rec.rtype);
        pos = write_u16(buf, pos, CLASS_IN_FLUSH);
        pos = write_u32(buf, pos, rec.ttl);
        pos = write_u16(buf, pos, rec.data_len as u16);
        if pos == 0 {
            return 0;
        }

        // RDATA
        if pos.saturating_add(rec.data_len) > buf.len() {
            return pos;
        }
        buf[pos..pos + rec.data_len].copy_from_slice(&rec.data[..rec.data_len]);
        pos = pos.saturating_add(rec.data_len);
    }

    pos
}

// ---------------------------------------------------------------------------
// Record store
// ---------------------------------------------------------------------------

/// One mDNS resource record that we own and advertise.
#[derive(Clone, Copy)]
pub struct MdnsRecord {
    /// DNS wire-encoded name (NOT dot-separated; stored as raw bytes)
    pub name: [u8; MAX_HOSTNAME],
    /// Valid bytes in `name` (including root 0 byte)
    pub name_len: usize,
    /// Record type (RR_A, RR_PTR, RR_SRV, RR_TXT, …)
    pub rtype: u16,
    /// Record TTL in seconds
    pub ttl: u32,
    /// Record data (wire format)
    pub data: [u8; MAX_RDATA],
    /// Valid bytes in `data`
    pub data_len: usize,
    /// Whether this slot is occupied
    pub active: bool,
}

impl MdnsRecord {
    const fn empty() -> Self {
        MdnsRecord {
            name: [0u8; MAX_HOSTNAME],
            name_len: 0,
            rtype: 0,
            ttl: 0,
            data: [0u8; MAX_RDATA],
            data_len: 0,
            active: false,
        }
    }
}

const EMPTY_RECORD: Option<MdnsRecord> = None;
// We store records as plain array (not Option) to keep Copy easy; use active flag.
static RECORDS: Mutex<[MdnsRecord; MAX_RECORDS]> = Mutex::new(
    [MdnsRecord {
        name: [0u8; MAX_HOSTNAME],
        name_len: 0,
        rtype: 0,
        ttl: 0,
        data: [0u8; MAX_RDATA],
        data_len: 0,
        active: false,
    }; MAX_RECORDS],
);

/// Add a record to the store.  Returns `true` on success, `false` if full.
fn records_add(rec: MdnsRecord) -> bool {
    let mut table = RECORDS.lock();
    for slot in table.iter_mut() {
        if !slot.active {
            *slot = rec;
            return true;
        }
    }
    false
}

/// Remove all records whose name matches `name` (str) and rtype matches `rtype`.
/// Pass `rtype = 0` to remove all types for that name.
fn records_remove(name: &str, rtype: u16) {
    let mut tmp_name = [0u8; MAX_HOSTNAME];
    let name_len = encode_name(name, &mut tmp_name);
    if name_len == 0 {
        return;
    }

    let mut table = RECORDS.lock();
    for slot in table.iter_mut() {
        if !slot.active {
            continue;
        }
        if slot.name_len == name_len
            && slot.name[..name_len] == tmp_name[..name_len]
            && (rtype == 0 || slot.rtype == rtype)
        {
            slot.active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// One cached mDNS record received from the network.
#[derive(Clone, Copy)]
pub struct CacheEntry {
    pub name: [u8; MAX_HOSTNAME],
    pub name_len: usize,
    pub rtype: u16,
    pub data: [u8; MAX_RDATA],
    pub data_len: usize,
    /// Remaining TTL in seconds; decremented by `tick()`
    pub ttl_remaining: u32,
    pub active: bool,
}

impl CacheEntry {
    const fn empty() -> Self {
        CacheEntry {
            name: [0u8; MAX_HOSTNAME],
            name_len: 0,
            rtype: 0,
            data: [0u8; MAX_RDATA],
            data_len: 0,
            ttl_remaining: 0,
            active: false,
        }
    }
}

static CACHE: Mutex<[CacheEntry; CACHE_SIZE]> = Mutex::new(
    [CacheEntry {
        name: [0u8; MAX_HOSTNAME],
        name_len: 0,
        rtype: 0,
        data: [0u8; MAX_RDATA],
        data_len: 0,
        ttl_remaining: 0,
        active: false,
    }; CACHE_SIZE],
);

/// Look up a cache entry by decoded-ASCII name and rtype.
///
/// `name` is a dot-separated string like "myhost.local".
/// Returns a copy of the matching entry if found and TTL > 0.
pub fn cache_lookup(name: &str, rtype: u16) -> Option<CacheEntry> {
    let mut wire = [0u8; MAX_HOSTNAME];
    let wire_len = encode_name(name, &mut wire);
    if wire_len == 0 {
        return None;
    }

    let table = CACHE.lock();
    for entry in table.iter() {
        if !entry.active || entry.ttl_remaining == 0 {
            continue;
        }
        if entry.rtype != rtype {
            continue;
        }
        if entry.name_len == wire_len && entry.name[..wire_len] == wire[..wire_len] {
            return Some(*entry);
        }
    }
    None
}

/// Insert or update a cache entry.
///
/// `name_wire` is already in DNS wire format (length-prefixed labels).
pub fn cache_insert(name_wire: &[u8], rtype: u16, data: &[u8], ttl: u32) {
    if name_wire.is_empty() || name_wire.len() > MAX_HOSTNAME {
        return;
    }
    if data.len() > MAX_RDATA {
        return;
    }

    let mut table = CACHE.lock();

    // Try to update existing entry
    for entry in table.iter_mut() {
        if !entry.active {
            continue;
        }
        if entry.rtype == rtype
            && entry.name_len == name_wire.len()
            && entry.name[..name_wire.len()] == *name_wire
        {
            entry.ttl_remaining = ttl;
            entry.data_len = data.len();
            entry.data[..data.len()].copy_from_slice(data);
            return;
        }
    }

    // Find an empty slot
    for entry in table.iter_mut() {
        if !entry.active {
            entry.active = true;
            entry.rtype = rtype;
            entry.name_len = name_wire.len();
            entry.name[..name_wire.len()].copy_from_slice(name_wire);
            entry.data_len = data.len();
            entry.data[..data.len()].copy_from_slice(data);
            entry.ttl_remaining = ttl;
            return;
        }
    }

    // Cache full — evict the entry with the lowest remaining TTL
    let mut min_ttl = u32::MAX;
    let mut min_idx = 0usize;
    for (i, entry) in table.iter().enumerate() {
        if entry.ttl_remaining < min_ttl {
            min_ttl = entry.ttl_remaining;
            min_idx = i;
        }
    }
    let entry = &mut table[min_idx];
    entry.active = true;
    entry.rtype = rtype;
    entry.name_len = name_wire.len();
    entry.name[..name_wire.len()].copy_from_slice(name_wire);
    entry.data_len = data.len();
    entry.data[..data.len()].copy_from_slice(data);
    entry.ttl_remaining = ttl;
}

// ---------------------------------------------------------------------------
// Service instance (DNS-SD browse result)
// ---------------------------------------------------------------------------

/// A discovered DNS-SD service instance.
#[derive(Clone, Copy)]
pub struct ServiceInstance {
    /// Instance name (e.g. "My Printer")  — null-terminated ASCII
    pub name: [u8; 64],
    /// Hostname (e.g. "myprinter.local") — null-terminated ASCII
    pub host: [u8; MAX_HOSTNAME],
    /// TCP/UDP port
    pub port: u16,
    /// Raw TXT record data (wire format)
    pub txt: [u8; MAX_RDATA],
    /// Valid bytes in `txt`
    pub txt_len: usize,
    /// Resolved IPv4 address, if available
    pub addr: Option<[u8; 4]>,
}

impl ServiceInstance {
    pub const fn empty() -> Self {
        ServiceInstance {
            name: [0u8; 64],
            host: [0u8; MAX_HOSTNAME],
            port: 0,
            txt: [0u8; MAX_RDATA],
            txt_len: 0,
            addr: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Hostname state
// ---------------------------------------------------------------------------

/// Our hostname in dot-separated form (e.g. "genesis.local"), null-terminated.
static HOSTNAME: Mutex<[u8; MAX_HOSTNAME]> = Mutex::new([0u8; MAX_HOSTNAME]);

/// Set our hostname.  `name` should be of the form "myhost.local".
pub fn set_hostname(name: &str) {
    let mut h = HOSTNAME.lock();
    h.iter_mut().for_each(|b| *b = 0);
    let src = name.as_bytes();
    let copy_len = src.len().min(MAX_HOSTNAME.saturating_sub(1));
    h[..copy_len].copy_from_slice(&src[..copy_len]);
    // null terminator already 0
}

/// Get our hostname into `out` (null-terminated). Returns number of bytes written.
pub fn get_hostname(out: &mut [u8]) -> usize {
    let h = HOSTNAME.lock();
    let mut len = 0usize;
    while len < h.len() && len < out.len() && h[len] != 0 {
        out[len] = h[len];
        len = len.saturating_add(1);
    }
    if len < out.len() {
        out[len] = 0;
    }
    len
}

// ---------------------------------------------------------------------------
// Our IP address
// ---------------------------------------------------------------------------

static OUR_IP: Mutex<[u8; 4]> = Mutex::new([0u8; 4]);

/// Set the IP address used for A records.
pub fn set_ip(ip: [u8; 4]) {
    *OUR_IP.lock() = ip;
}

// ---------------------------------------------------------------------------
// Send helper
// ---------------------------------------------------------------------------

/// Transmit `data` as a UDP datagram to the mDNS multicast address.
fn send_mdns(data: &[u8]) {
    use crate::net::Ipv4Addr;
    let dst = Ipv4Addr(MDNS_MULTICAST_ADDR);
    let _ = crate::net::send_udp(MDNS_PORT, dst, MDNS_PORT, data);
}

// ---------------------------------------------------------------------------
// Probe phase (RFC 6762 §8.1)
// ---------------------------------------------------------------------------

/// Send 3 QU (unicast-response) queries for our hostname A record,
/// polling for responses between each.
///
/// Returns `true` if no conflict detected (name is unique on the link).
/// Returns `false` if another host claims the same name.
pub fn probe_hostname(hostname: &str) -> bool {
    let mut query_buf = [0u8; 512];
    // Build <hostname>.local query
    let mut qname = [0u8; MAX_HOSTNAME];
    let qname_src = hostname.as_bytes();
    let copy_len = qname_src.len().min(MAX_HOSTNAME.saturating_sub(1));
    qname[..copy_len].copy_from_slice(&qname_src[..copy_len]);

    let qlen = build_query(hostname, RR_A, &mut query_buf);
    if qlen == 0 {
        return true;
    } // can't probe — assume unique

    // Bind receive port
    crate::net::udp_bind(MDNS_PORT);

    let mut conflict = false;

    for _attempt in 0..3u8 {
        send_mdns(&query_buf[..qlen]);

        // Spin-poll ~250 ms worth of iterations
        for _ in 0..250_000u32 {
            crate::net::poll();
            if let Some((_src_ip, _src_port, resp)) = crate::net::udp_recv(MDNS_PORT) {
                // If QR bit is set (it's a response) and it contains an A record
                // for our hostname, we have a conflict.
                if resp.len() >= 12 {
                    let flags = read_u16(&resp, 2);
                    if flags & 0x8000 != 0 {
                        // It's a response — check answers for our hostname
                        if response_mentions_name(&resp, hostname) {
                            conflict = true;
                            break;
                        }
                    }
                }
            }
            core::hint::spin_loop();
        }
        if conflict {
            break;
        }
    }

    !conflict
}

/// Returns true if any answer record in a parsed response packet has a name
/// matching `name` (dot-separated, case-insensitive).
fn response_mentions_name(pkt: &[u8], name: &str) -> bool {
    if pkt.len() < 12 {
        return false;
    }
    let ancount = read_u16(pkt, 6) as usize;
    if ancount == 0 {
        return false;
    }

    let mut pos = 12usize;

    // Skip questions
    let qdcount = read_u16(pkt, 4) as usize;
    for _ in 0..qdcount {
        let mut tmp = [0u8; MAX_HOSTNAME];
        let consumed = decode_name(pkt, pos, &mut tmp);
        if consumed == 0 {
            return false;
        }
        pos = pos.saturating_add(consumed);
        pos = pos.saturating_add(4); // QTYPE + QCLASS
        if pos > pkt.len() {
            return false;
        }
    }

    // Check answers
    for _ in 0..ancount {
        if pos >= pkt.len() {
            break;
        }
        let mut tmp = [0u8; MAX_HOSTNAME];
        let consumed = decode_name(pkt, pos, &mut tmp);
        if consumed == 0 {
            break;
        }
        if name_eq(&tmp, name) {
            return true;
        }
        pos = pos.saturating_add(consumed);
        if pos.saturating_add(10) > pkt.len() {
            break;
        }
        let rdlength = read_u16(pkt, pos.saturating_add(8)) as usize;
        pos = pos.saturating_add(10).saturating_add(rdlength);
    }
    false
}

// ---------------------------------------------------------------------------
// Announcement (RFC 6762 §8.3)
// ---------------------------------------------------------------------------

/// Announce all active records via unsolicited mDNS responses (sent twice
/// per RFC 6762 §8.3, 1 second apart would require a timer; here we send once
/// and rely on periodic `tick()` for re-announcements).
pub fn announce_all() {
    let table = RECORDS.lock();
    let mut count = 0usize;
    for slot in table.iter() {
        if slot.active {
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        return;
    }

    let mut buf = [0u8; 1500];
    let len = build_response(&*table, count, &mut buf);
    drop(table);

    if len > 0 {
        send_mdns(&buf[..len]);
    }
}

// ---------------------------------------------------------------------------
// Query for a hostname A record (RFC 6762 §6)
// ---------------------------------------------------------------------------

/// Send a QU query for `hostname` and spin-poll up to ~1000 ms for an answer.
///
/// Returns the resolved IPv4 address, or `None` on timeout / not found.
pub fn query_a(hostname: &str) -> Option<[u8; 4]> {
    // Check cache first
    if let Some(entry) = cache_lookup(hostname, RR_A) {
        if entry.data_len == 4 {
            return Some([entry.data[0], entry.data[1], entry.data[2], entry.data[3]]);
        }
    }

    let mut buf = [0u8; 512];
    let qlen = build_query(hostname, RR_A, &mut buf);
    if qlen == 0 {
        return None;
    }

    crate::net::udp_bind(MDNS_PORT);
    send_mdns(&buf[..qlen]);

    for _ in 0..1_000_000u32 {
        crate::net::poll();
        if let Some((_src_ip, _src_port, resp)) = crate::net::udp_recv(MDNS_PORT) {
            if let Some(ip) = parse_a_from_response(&resp, hostname) {
                // Store in cache
                let mut wire_name = [0u8; MAX_HOSTNAME];
                let wlen = encode_name(hostname, &mut wire_name);
                if wlen > 0 {
                    cache_insert(&wire_name[..wlen], RR_A, &ip, DEFAULT_RECORD_TTL);
                }
                return Some(ip);
            }
        }
        core::hint::spin_loop();
    }
    None
}

/// Extract the first A record answer for `hostname` from a raw mDNS response.
fn parse_a_from_response(pkt: &[u8], hostname: &str) -> Option<[u8; 4]> {
    if pkt.len() < 12 {
        return None;
    }
    let flags = read_u16(pkt, 2);
    if flags & 0x8000 == 0 {
        return None;
    } // not a response

    let ancount = read_u16(pkt, 6) as usize;
    if ancount == 0 {
        return None;
    }

    let mut pos = 12usize;

    // Skip questions
    let qdcount = read_u16(pkt, 4) as usize;
    for _ in 0..qdcount {
        let mut tmp = [0u8; MAX_HOSTNAME];
        let consumed = decode_name(pkt, pos, &mut tmp);
        if consumed == 0 {
            return None;
        }
        pos = pos.saturating_add(consumed).saturating_add(4);
        if pos > pkt.len() {
            return None;
        }
    }

    for _ in 0..ancount {
        if pos >= pkt.len() {
            break;
        }
        let mut rr_name = [0u8; MAX_HOSTNAME];
        let consumed = decode_name(pkt, pos, &mut rr_name);
        if consumed == 0 {
            break;
        }
        pos = pos.saturating_add(consumed);
        if pos.saturating_add(10) > pkt.len() {
            break;
        }

        let rtype = read_u16(pkt, pos);
        let ttl = read_u32(pkt, pos.saturating_add(4));
        let rdlength = read_u16(pkt, pos.saturating_add(8)) as usize;
        pos = pos.saturating_add(10);

        if pos.saturating_add(rdlength) > pkt.len() {
            break;
        }

        if rtype == RR_A && rdlength == 4 && name_eq(&rr_name, hostname) {
            let ip = [pkt[pos], pkt[pos + 1], pkt[pos + 2], pkt[pos + 3]];
            // Cache
            let mut wire_name = [0u8; MAX_HOSTNAME];
            let wlen = encode_name(hostname, &mut wire_name);
            if wlen > 0 {
                cache_insert(&wire_name[..wlen], RR_A, &ip, ttl);
            }
            return Some(ip);
        }
        pos = pos.saturating_add(rdlength);
    }
    None
}

// ---------------------------------------------------------------------------
// Query for service instances (DNS-SD PTR browse)
// ---------------------------------------------------------------------------

/// Send a PTR query for `service_type` (e.g. "_http._tcp.local") and collect
/// responses for ~1 000 000 spin iterations.
///
/// Returns an array of up to 8 service instances discovered.
pub fn query_service(service_type: &str) -> [Option<ServiceInstance>; 8] {
    let mut results = [None; 8];
    let mut found = 0usize;

    let mut buf = [0u8; 512];
    let qlen = build_query(service_type, RR_PTR, &mut buf);
    if qlen == 0 {
        return results;
    }

    crate::net::udp_bind(MDNS_PORT);
    send_mdns(&buf[..qlen]);

    for _ in 0..1_000_000u32 {
        crate::net::poll();
        if let Some((_src_ip, _src_port, resp)) = crate::net::udp_recv(MDNS_PORT) {
            if resp.len() < 12 {
                continue;
            }
            let flags = read_u16(&resp, 2);
            if flags & 0x8000 == 0 {
                continue;
            } // not a response

            // Parse any PTR records whose name matches our service_type
            let ancount = read_u16(&resp, 6) as usize;
            let mut pos = 12usize;

            let qdcount = read_u16(&resp, 4) as usize;
            for _ in 0..qdcount {
                let mut tmp = [0u8; MAX_HOSTNAME];
                let c = decode_name(&resp, pos, &mut tmp);
                if c == 0 {
                    break;
                }
                pos = pos.saturating_add(c).saturating_add(4);
            }

            for _ in 0..ancount {
                if found >= 8 || pos >= resp.len() {
                    break;
                }
                let mut rr_name = [0u8; MAX_HOSTNAME];
                let consumed = decode_name(&resp, pos, &mut rr_name);
                if consumed == 0 {
                    break;
                }
                pos = pos.saturating_add(consumed);
                if pos.saturating_add(10) > resp.len() {
                    break;
                }

                let rtype = read_u16(&resp, pos);
                let rdlength = read_u16(&resp, pos.saturating_add(8)) as usize;
                pos = pos.saturating_add(10);

                if pos.saturating_add(rdlength) > resp.len() {
                    break;
                }

                if rtype == RR_PTR && name_eq(&rr_name, service_type) {
                    // rdata is the instance name in wire format
                    let mut inst_wire = [0u8; MAX_HOSTNAME];
                    let ic = decode_name(&resp, pos, &mut inst_wire);
                    if ic > 0 {
                        let mut si = ServiceInstance::empty();
                        // Copy short instance name (up to first '.')
                        let mut j = 0usize;
                        while j < 63 && inst_wire[j] != 0 && inst_wire[j] != b'.' {
                            si.name[j] = inst_wire[j];
                            j = j.saturating_add(1);
                        }
                        si.name[j] = 0;
                        results[found] = Some(si);
                        found = found.saturating_add(1);
                    }
                }
                pos = pos.saturating_add(rdlength);
            }
        }
        if found >= 8 {
            break;
        }
        core::hint::spin_loop();
    }

    results
}

// ---------------------------------------------------------------------------
// Service registration (DNS-SD)
// ---------------------------------------------------------------------------

/// Register a DNS-SD service by creating PTR, SRV, and TXT records.
///
/// `instance_name` — e.g. "My Genesis Node"
/// `service_type`  — e.g. "_http._tcp.local"
/// `port`          — TCP/UDP port number
/// `txt_records`   — slice of (key, value) pairs for the TXT record
///
/// Returns `true` if all records were stored successfully.
pub fn register_service(
    instance_name: &str,
    service_type: &str,
    port: u16,
    txt_records: &[(&str, &str)],
) -> bool {
    let our_ip = *OUR_IP.lock();

    // Build full instance name: "Instance._service._tcp.local"
    // We concatenate into a fixed-size buffer
    let mut full_name_buf = [0u8; MAX_HOSTNAME];
    let inst_b = instance_name.as_bytes();
    let svc_b = service_type.as_bytes();
    let mut fn_len = 0usize;
    // Copy instance
    let copy_a = inst_b.len().min(MAX_HOSTNAME.saturating_sub(2));
    full_name_buf[fn_len..fn_len + copy_a].copy_from_slice(&inst_b[..copy_a]);
    fn_len = fn_len.saturating_add(copy_a);
    // Dot separator
    if fn_len < MAX_HOSTNAME.saturating_sub(1) {
        full_name_buf[fn_len] = b'.';
        fn_len = fn_len.saturating_add(1);
    }
    // Copy service type
    let copy_b = svc_b
        .len()
        .min(MAX_HOSTNAME.saturating_sub(fn_len).saturating_sub(1));
    full_name_buf[fn_len..fn_len + copy_b].copy_from_slice(&svc_b[..copy_b]);
    fn_len = fn_len.saturating_add(copy_b);
    full_name_buf[fn_len] = 0;
    let full_name_str = core::str::from_utf8(&full_name_buf[..fn_len]).unwrap_or("");

    // --- PTR record: _service._tcp.local → full_name ---
    {
        let mut rec = MdnsRecord::empty();
        rec.active = true;
        rec.rtype = RR_PTR;
        rec.ttl = DEFAULT_RECORD_TTL;

        let name_len = encode_name(service_type, &mut rec.name);
        if name_len == 0 {
            return false;
        }
        rec.name_len = name_len;

        // rdata = wire-encoded full instance name
        let data_len = encode_name(full_name_str, &mut rec.data);
        if data_len == 0 {
            return false;
        }
        rec.data_len = data_len;

        if !records_add(rec) {
            return false;
        }
    }

    // --- SRV record: full_name → hostname:port ---
    {
        let mut rec = MdnsRecord::empty();
        rec.active = true;
        rec.rtype = RR_SRV;
        rec.ttl = DEFAULT_RECORD_TTL;

        let name_len = encode_name(full_name_str, &mut rec.name);
        if name_len == 0 {
            return false;
        }
        rec.name_len = name_len;

        // rdata: priority(2), weight(2), port(2), target name
        rec.data[0] = 0;
        rec.data[1] = 0; // priority = 0
        rec.data[2] = 0;
        rec.data[3] = 0; // weight = 0
        rec.data[4] = (port >> 8) as u8;
        rec.data[5] = port as u8;

        let hostname_buf = *HOSTNAME.lock();
        let hostname_str = core::str::from_utf8(
            // Find null terminator
            {
                let mut end = 0usize;
                while end < hostname_buf.len() && hostname_buf[end] != 0 {
                    end += 1;
                }
                &hostname_buf[..end]
            },
        )
        .unwrap_or("aios.local");

        let target_len = encode_name(hostname_str, &mut rec.data[6..]);
        if target_len == 0 {
            return false;
        }
        rec.data_len = 6usize.saturating_add(target_len);

        if !records_add(rec) {
            return false;
        }
    }

    // --- TXT record ---
    {
        let mut rec = MdnsRecord::empty();
        rec.active = true;
        rec.rtype = RR_TXT;
        rec.ttl = DEFAULT_RECORD_TTL;

        let name_len = encode_name(full_name_str, &mut rec.name);
        if name_len == 0 {
            return false;
        }
        rec.name_len = name_len;

        // rdata: length-prefixed "key=value" strings
        let mut dpos = 0usize;
        for (k, v) in txt_records.iter() {
            let klen = k.len();
            let vlen = v.len();
            // "key=value" or "key" (if value is empty)
            let entry_len = if vlen == 0 { klen } else { klen + 1 + vlen };
            if entry_len > 255 {
                continue;
            }
            if dpos.saturating_add(1 + entry_len) > MAX_RDATA {
                break;
            }
            rec.data[dpos] = entry_len as u8;
            dpos = dpos.saturating_add(1);
            rec.data[dpos..dpos + klen].copy_from_slice(k.as_bytes());
            dpos = dpos.saturating_add(klen);
            if vlen > 0 {
                rec.data[dpos] = b'=';
                dpos = dpos.saturating_add(1);
                rec.data[dpos..dpos + vlen].copy_from_slice(v.as_bytes());
                dpos = dpos.saturating_add(vlen);
            }
        }
        rec.data_len = dpos;

        if !records_add(rec) {
            return false;
        }
    }

    // --- A record: hostname → our IP ---
    {
        let hostname_buf = *HOSTNAME.lock();
        let mut end = 0usize;
        while end < hostname_buf.len() && hostname_buf[end] != 0 {
            end += 1;
        }
        let hostname_str = core::str::from_utf8(&hostname_buf[..end]).unwrap_or("aios.local");

        let mut rec = MdnsRecord::empty();
        rec.active = true;
        rec.rtype = RR_A;
        rec.ttl = DEFAULT_RECORD_TTL;

        let name_len = encode_name(hostname_str, &mut rec.name);
        if name_len == 0 {
            return false;
        }
        rec.name_len = name_len;

        rec.data[..4].copy_from_slice(&our_ip);
        rec.data_len = 4;

        // Don't fail if A record already registered (just try to add)
        records_add(rec);
    }

    // Announce
    announce_all();
    true
}

/// Unregister a service by removing its PTR, SRV, TXT records and sending
/// goodbye packets (TTL = 0) per RFC 6762 §11.3.
pub fn unregister_service(instance_name: &str, service_type: &str) {
    // Build full name
    let mut full_name_buf = [0u8; MAX_HOSTNAME];
    let inst_b = instance_name.as_bytes();
    let svc_b = service_type.as_bytes();
    let mut fn_len = 0usize;
    let copy_a = inst_b.len().min(MAX_HOSTNAME.saturating_sub(2));
    full_name_buf[..copy_a].copy_from_slice(&inst_b[..copy_a]);
    fn_len = copy_a;
    if fn_len < MAX_HOSTNAME.saturating_sub(1) {
        full_name_buf[fn_len] = b'.';
        fn_len = fn_len.saturating_add(1);
    }
    let copy_b = svc_b
        .len()
        .min(MAX_HOSTNAME.saturating_sub(fn_len).saturating_sub(1));
    full_name_buf[fn_len..fn_len + copy_b].copy_from_slice(&svc_b[..copy_b]);
    fn_len = fn_len.saturating_add(copy_b);
    full_name_buf[fn_len] = 0;
    let full_name_str = core::str::from_utf8(&full_name_buf[..fn_len]).unwrap_or("");

    // Build and send goodbye PTR (TTL=0)
    let mut goodbye_rec = MdnsRecord::empty();
    goodbye_rec.rtype = RR_PTR;
    goodbye_rec.ttl = 0;

    let name_len = encode_name(service_type, &mut goodbye_rec.name);
    goodbye_rec.name_len = name_len;
    let data_len = encode_name(full_name_str, &mut goodbye_rec.data);
    goodbye_rec.data_len = data_len;
    goodbye_rec.active = true;

    let mut buf = [0u8; 512];
    let len = build_response(core::slice::from_ref(&goodbye_rec), 1, &mut buf);
    if len > 0 {
        send_mdns(&buf[..len]);
    }

    // Remove from record store
    records_remove(full_name_str, 0); // PTR, SRV, TXT for full name
    records_remove(service_type, RR_PTR);
}

// ---------------------------------------------------------------------------
// Incoming packet processing (RFC 6762 §6)
// ---------------------------------------------------------------------------

/// Process an incoming mDNS packet.
///
/// Called when a UDP datagram arrives on port 5353 from the multicast group.
/// `data` — raw UDP payload bytes.
/// `src_ip`, `src_port` — sender information (not currently used for replies;
/// mDNS responses always go to the multicast group).
pub fn process_packet(data: &[u8], _src_ip: [u8; 4], _src_port: u16) {
    if data.len() < 12 {
        return;
    }

    let flags = read_u16(data, 2);
    let qdcount = read_u16(data, 4) as usize;
    let ancount = read_u16(data, 6) as usize;

    let is_response = flags & 0x8000 != 0;

    let mut pos = 12usize;

    if is_response {
        // Cache all answer records
        // (also used by query_a / query_service via udp_recv)
        for _ in 0..ancount {
            if pos >= data.len() {
                break;
            }
            let mut rr_name = [0u8; MAX_HOSTNAME];
            let consumed = decode_name(data, pos, &mut rr_name);
            if consumed == 0 {
                break;
            }
            // Get wire-format name for cache_insert
            let mut wire_name = [0u8; MAX_HOSTNAME];
            let wlen = {
                // re-encode to canonical wire format
                let name_str = core::str::from_utf8({
                    let mut end = 0usize;
                    while end < rr_name.len() && rr_name[end] != 0 {
                        end += 1;
                    }
                    &rr_name[..end]
                })
                .unwrap_or("");
                encode_name(name_str, &mut wire_name)
            };

            pos = pos.saturating_add(consumed);
            if pos.saturating_add(10) > data.len() {
                break;
            }

            let rtype = read_u16(data, pos);
            let ttl = read_u32(data, pos.saturating_add(4));
            let rdlength = read_u16(data, pos.saturating_add(8)) as usize;
            pos = pos.saturating_add(10);

            if pos.saturating_add(rdlength) > data.len() {
                break;
            }

            if wlen > 0 {
                cache_insert(&wire_name[..wlen], rtype, &data[pos..pos + rdlength], ttl);
            }

            pos = pos.saturating_add(rdlength);
        }
    } else {
        // It's a query — answer if we have matching records
        let mut answer_buf = [0u8; 1400];
        let mut answer_records = [MdnsRecord::empty(); 16];
        let mut answer_count = 0usize;

        let table = RECORDS.lock();

        for _ in 0..qdcount {
            if pos >= data.len() {
                break;
            }
            let mut qname = [0u8; MAX_HOSTNAME];
            let consumed = decode_name(data, pos, &mut qname);
            if consumed == 0 {
                break;
            }
            pos = pos.saturating_add(consumed);
            if pos.saturating_add(4) > data.len() {
                break;
            }

            let qtype = read_u16(data, pos);
            // qclass (lower 15 bits, bit 15 is QU flag) — we accept any class
            let _qclass = read_u16(data, pos.saturating_add(2));
            pos = pos.saturating_add(4);

            // Known-answer suppression: skip answers we already know about
            // (simplified — we just match on name+type)

            // Find matching records in our store
            for slot in table.iter() {
                if !slot.active {
                    continue;
                }
                if answer_count >= 16 {
                    break;
                }

                // Compare wire-encoded name
                let matches_name = slot.name_len > 0
                    && slot.name[..slot.name_len] == qname[..slot.name_len.min(qname.len())];

                // Accept if type matches or query is ANY
                let matches_type = qtype == RR_ANY || qtype == slot.rtype;

                if matches_name && matches_type {
                    answer_records[answer_count] = *slot;
                    answer_count = answer_count.saturating_add(1);
                }
            }
        }
        drop(table);

        if answer_count > 0 {
            let len = build_response(&answer_records, answer_count, &mut answer_buf);
            if len > 0 {
                send_mdns(&answer_buf[..len]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Periodic maintenance
// ---------------------------------------------------------------------------

/// Tick counter for announcement scheduling
static TICK_COUNT: Mutex<u32> = Mutex::new(0);

/// Periodic maintenance — call approximately every 1 second.
///
/// - Decrements TTLs in the cache and evicts expired entries.
/// - Re-announces our records every ~60 ticks (≈ 1 minute).
pub fn tick() {
    // Age cache entries
    {
        let mut cache = CACHE.lock();
        for entry in cache.iter_mut() {
            if entry.active {
                if entry.ttl_remaining == 0 {
                    entry.active = false;
                } else {
                    entry.ttl_remaining = entry.ttl_remaining.saturating_sub(1);
                }
            }
        }
    }

    // Periodic re-announcement
    let count = {
        let mut c = TICK_COUNT.lock();
        *c = c.wrapping_add(1);
        *c
    };
    if count % 60 == 0 {
        announce_all();
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the mDNS subsystem.
///
/// - Joins multicast group 224.0.0.251 via IGMP.
/// - Sets the default hostname to "genesis.local".
/// - Registers the default hostname A record once the IP is configured.
pub fn init() {
    // Join mDNS multicast group
    crate::net::igmp::igmp_join(MDNS_MULTICAST_ADDR);

    // Default hostname
    set_hostname("genesis.local");

    // Bind UDP port for receiving mDNS packets
    crate::net::udp_bind(MDNS_PORT);

    serial_println!("  Net: mDNS subsystem initialized (224.0.0.251:5353)");
}
