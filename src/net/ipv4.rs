use super::Ipv4Addr;
use crate::sync::Mutex;
/// IPv4 packet handling for Genesis
///
/// Parses and constructs IPv4 packets (RFC 791).
/// Header format: version/IHL, DSCP/ECN, total length, ID, flags/fragment,
///   TTL, protocol, checksum, source IP, destination IP, [options]
///
/// Features:
///   - IP header building with proper checksum
///   - IP fragmentation (split packets > MTU)
///   - IP fragment reassembly (collect fragments by ID, timeout stale)
///   - TTL decrement and expiry handling
///   - IP options parsing (record route, timestamp)
///   - Header checksum verification on receive
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, Ordering as AOrdering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IP protocol numbers
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;
pub const PROTO_GRE: u8 = 47;
pub const PROTO_ESP: u8 = 50;
pub const PROTO_AH: u8 = 51;

/// IP flags (in flags_fragment field)
const FLAG_RESERVED: u16 = 0x8000;
const FLAG_DONT_FRAGMENT: u16 = 0x4000;
const FLAG_MORE_FRAGMENTS: u16 = 0x2000;

/// Fragment offset mask (13 bits)
const FRAG_OFFSET_MASK: u16 = 0x1FFF;

/// Default TTL for outgoing packets
pub const DEFAULT_TTL: u8 = 64;

/// Maximum TTL
pub const MAX_TTL: u8 = 255;

/// Fragment reassembly timeout in ticks (ms) — 30 seconds
const REASSEMBLY_TIMEOUT_MS: u64 = 30_000;

/// Maximum total size of reassembled packet
const MAX_REASSEMBLED_SIZE: usize = 65535;

/// Maximum number of active reassembly buffers
const MAX_REASSEMBLY_BUFFERS: usize = 64;

/// IP option types
pub const OPT_END: u8 = 0;
pub const OPT_NOP: u8 = 1;
pub const OPT_RECORD_ROUTE: u8 = 7;
pub const OPT_TIMESTAMP: u8 = 68;
pub const OPT_SECURITY: u8 = 130;
pub const OPT_LOOSE_SOURCE: u8 = 131;
pub const OPT_STRICT_SOURCE: u8 = 137;

// ---------------------------------------------------------------------------
// IPv4 header
// ---------------------------------------------------------------------------

/// IPv4 header (20 bytes minimum, no options)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ipv4Header {
    pub version_ihl: u8,       // Version (4) + IHL (4)
    pub dscp_ecn: u8,          // DSCP (6) + ECN (2)
    pub total_length: [u8; 2], // Total packet length (big-endian)
    pub identification: [u8; 2],
    pub flags_fragment: [u8; 2], // Flags (3) + Fragment offset (13)
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: [u8; 2],
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
}

impl Ipv4Header {
    /// Parse an IPv4 header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(&Ipv4Header, &[u8])> {
        if data.len() < 20 {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const Ipv4Header) };

        // Verify version is 4
        if header.version() != 4 {
            return None;
        }

        let header_len = header.ihl() as usize * 4;
        if data.len() < header_len {
            return None;
        }

        // Determine payload length from total_length
        let total = header.total_length() as usize;
        if total < header_len || data.len() < total {
            // If total_length is less than header, or we don't have enough data
            // Fall back to remaining data
            let payload = &data[header_len..];
            return Some((header, payload));
        }

        let payload = &data[header_len..total];
        Some((header, payload))
    }

    /// IP version (should be 4)
    // hot path: called on every received packet
    #[inline(always)]
    pub fn version(&self) -> u8 {
        self.version_ihl >> 4
    }

    /// Internet Header Length in 32-bit words
    #[inline(always)]
    pub fn ihl(&self) -> u8 {
        self.version_ihl & 0x0F
    }

    /// Header length in bytes
    #[inline(always)]
    pub fn header_len(&self) -> usize {
        self.ihl() as usize * 4
    }

    /// Total packet length
    #[inline(always)]
    pub fn total_length(&self) -> u16 {
        u16::from_be_bytes(self.total_length)
    }

    /// Identification field
    pub fn id(&self) -> u16 {
        u16::from_be_bytes(self.identification)
    }

    /// Flags + fragment offset as u16
    fn flags_frag_u16(&self) -> u16 {
        u16::from_be_bytes(self.flags_fragment)
    }

    /// Don't Fragment flag
    pub fn dont_fragment(&self) -> bool {
        self.flags_frag_u16() & FLAG_DONT_FRAGMENT != 0
    }

    /// More Fragments flag
    pub fn more_fragments(&self) -> bool {
        self.flags_frag_u16() & FLAG_MORE_FRAGMENTS != 0
    }

    /// Fragment offset in 8-byte units
    pub fn fragment_offset(&self) -> u16 {
        self.flags_frag_u16() & FRAG_OFFSET_MASK
    }

    /// Fragment offset in bytes
    pub fn fragment_offset_bytes(&self) -> usize {
        (self.fragment_offset() as usize) * 8
    }

    /// Is this a fragment (either MF set or offset > 0)?
    pub fn is_fragment(&self) -> bool {
        self.more_fragments() || self.fragment_offset() > 0
    }

    /// Source IP address
    pub fn src_addr(&self) -> Ipv4Addr {
        Ipv4Addr(self.src_ip)
    }

    /// Destination IP address
    pub fn dst_addr(&self) -> Ipv4Addr {
        Ipv4Addr(self.dst_ip)
    }

    /// DSCP (Differentiated Services Code Point)
    pub fn dscp(&self) -> u8 {
        self.dscp_ecn >> 2
    }

    /// ECN (Explicit Congestion Notification)
    pub fn ecn(&self) -> u8 {
        self.dscp_ecn & 0x03
    }

    /// Verify the header checksum
    pub fn verify_checksum(&self) -> bool {
        let header_bytes = unsafe {
            core::slice::from_raw_parts(self as *const Self as *const u8, self.ihl() as usize * 4)
        };
        internet_checksum(header_bytes) == 0
    }
}

// ---------------------------------------------------------------------------
// IP identification counter
// ---------------------------------------------------------------------------

/// Global packet identification counter.
///
/// Replaced Mutex<u16> with AtomicU16: the IP ID just needs to be unique
/// enough to avoid collisions within the reassembly window.  Atomic
/// fetch_add gives that without a lock.  Wrapping is fine — 0 is skipped
/// via the modulo trick below.
// hot path: called for every outgoing IP packet build
static IP_ID_COUNTER: AtomicU16 = AtomicU16::new(1);

/// Get the next IP identification number (lock-free, O(1)).
#[inline(always)]
fn next_ip_id() -> u16 {
    // Relaxed ordering: IP ID does not synchronise any other memory —
    // it just needs to be different from recent values.
    let val = IP_ID_COUNTER.fetch_add(1, AOrdering::Relaxed);
    // Skip 0 (some stacks treat 0 as "don't fragment" indication).
    if val == 0 {
        1
    } else {
        val
    }
}

// ---------------------------------------------------------------------------
// Header building
// ---------------------------------------------------------------------------

/// Build an IPv4 header
pub fn build_header(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    payload_len: u16,
    ttl: u8,
) -> Ipv4Header {
    let total_len = 20 + payload_len;
    let id = next_ip_id();

    let mut header = Ipv4Header {
        version_ihl: (4 << 4) | 5, // version 4, IHL 5 (20 bytes, no options)
        dscp_ecn: 0,
        total_length: total_len.to_be_bytes(),
        identification: id.to_be_bytes(),
        flags_fragment: [0x40, 0x00], // Don't Fragment flag set
        ttl,
        protocol,
        checksum: [0, 0],
        src_ip: src.0,
        dst_ip: dst.0,
    };

    // Compute checksum
    let header_bytes =
        unsafe { core::slice::from_raw_parts(&header as *const Ipv4Header as *const u8, 20) };
    let cksum = internet_checksum(header_bytes);
    header.checksum = cksum.to_be_bytes();

    header
}

/// Build an IPv4 header with specific DSCP value.
pub fn build_header_dscp(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    payload_len: u16,
    ttl: u8,
    dscp: u8,
) -> Ipv4Header {
    let mut hdr = build_header(src, dst, protocol, payload_len, ttl);
    hdr.dscp_ecn = (dscp & 0x3F) << 2;
    // Recompute checksum
    hdr.checksum = [0, 0];
    let header_bytes =
        unsafe { core::slice::from_raw_parts(&hdr as *const Ipv4Header as *const u8, 20) };
    hdr.checksum = internet_checksum(header_bytes).to_be_bytes();
    hdr
}

/// Build an IPv4 header allowing fragmentation (DF bit cleared).
pub fn build_header_fragmentable(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    payload_len: u16,
    ttl: u8,
) -> Ipv4Header {
    let mut hdr = build_header(src, dst, protocol, payload_len, ttl);
    // Clear Don't Fragment flag
    hdr.flags_fragment = [0x00, 0x00];
    // Recompute checksum
    hdr.checksum = [0, 0];
    let header_bytes =
        unsafe { core::slice::from_raw_parts(&hdr as *const Ipv4Header as *const u8, 20) };
    hdr.checksum = internet_checksum(header_bytes).to_be_bytes();
    hdr
}

/// Serialize an IPv4 header to bytes
pub fn header_to_bytes(hdr: &Ipv4Header) -> [u8; 20] {
    let bytes = unsafe { core::slice::from_raw_parts(hdr as *const Ipv4Header as *const u8, 20) };
    let mut buf = [0u8; 20];
    buf.copy_from_slice(bytes);
    buf
}

// ---------------------------------------------------------------------------
// TTL handling
// ---------------------------------------------------------------------------

/// Decrement TTL and return the new value. Returns None if TTL was 0 or 1
/// (packet should be discarded and ICMP Time Exceeded sent).
pub fn decrement_ttl(packet: &mut [u8]) -> Option<u8> {
    if packet.len() < 20 {
        return None;
    }
    let ttl = packet[8];
    if ttl <= 1 {
        return None; // TTL expired
    }
    let new_ttl = ttl - 1;
    packet[8] = new_ttl;

    // Update header checksum incrementally
    // Old checksum
    let old_cksum = u16::from_be_bytes([packet[10], packet[11]]);
    // Incremental update: adding 1 to the TTL byte means subtracting 0x0100 from the checksum
    // (since TTL is at byte offset 8, which is in the high byte of the 4th 16-bit word)
    let mut sum = old_cksum as u32 + 0x0100; // TTL decreased by 1 → checksum increases by 0x0100
                                             // Fold carry
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    packet[10] = (sum >> 8) as u8;
    packet[11] = sum as u8;

    Some(new_ttl)
}

// ---------------------------------------------------------------------------
// Internet checksum (RFC 1071)
// ---------------------------------------------------------------------------

/// Compute the Internet checksum (RFC 1071)
/// Used for IPv4 headers, ICMP, TCP, UDP
// hot path: called on every outgoing IP packet and every received header verify
#[inline(always)]
pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;

    // Sum 16-bit words
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i = i.saturating_add(2);
    }

    // Handle odd byte
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }

    // Fold 32-bit sum to 16 bits
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }

    !sum as u16
}

// ---------------------------------------------------------------------------
// IP fragmentation
// ---------------------------------------------------------------------------

/// Fragment a payload into multiple IP packets suitable for the given MTU.
/// Returns a Vec of complete IP packets (header + payload).
/// `mtu` is the maximum total size of each IP packet (header + data).
pub fn fragment(
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    ttl: u8,
    payload: &[u8],
    mtu: usize,
) -> Vec<Vec<u8>> {
    let header_len = 20usize;
    let max_payload = mtu - header_len;
    // Fragment payload must be multiple of 8 (except last fragment)
    let frag_payload = (max_payload / 8) * 8;

    if frag_payload == 0 {
        return Vec::new(); // MTU too small
    }

    // If no fragmentation needed
    if payload.len() + header_len <= mtu {
        let hdr = build_header(src, dst, protocol, payload.len() as u16, ttl);
        let hdr_bytes = header_to_bytes(&hdr);
        let mut pkt = Vec::with_capacity(header_len + payload.len());
        pkt.extend_from_slice(&hdr_bytes);
        pkt.extend_from_slice(payload);
        return alloc::vec![pkt];
    }

    let id = next_ip_id();
    let mut fragments = Vec::new();
    let mut offset = 0usize;

    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let is_last = remaining <= frag_payload;
        let this_len = if is_last { remaining } else { frag_payload };

        let total_len = (header_len + this_len) as u16;
        let frag_offset_units = (offset / 8) as u16;
        let flags_frag: u16 = if is_last {
            frag_offset_units // No MF flag, just offset
        } else {
            FLAG_MORE_FRAGMENTS | frag_offset_units
        };

        let mut hdr = Ipv4Header {
            version_ihl: (4 << 4) | 5,
            dscp_ecn: 0,
            total_length: total_len.to_be_bytes(),
            identification: id.to_be_bytes(),
            flags_fragment: flags_frag.to_be_bytes(),
            ttl,
            protocol,
            checksum: [0, 0],
            src_ip: src.0,
            dst_ip: dst.0,
        };

        // Compute checksum
        let hdr_bytes_raw =
            unsafe { core::slice::from_raw_parts(&hdr as *const Ipv4Header as *const u8, 20) };
        let cksum = internet_checksum(hdr_bytes_raw);
        hdr.checksum = cksum.to_be_bytes();

        let hdr_bytes = header_to_bytes(&hdr);
        let mut pkt = Vec::with_capacity(header_len + this_len);
        pkt.extend_from_slice(&hdr_bytes);
        pkt.extend_from_slice(&payload[offset..offset + this_len]);
        fragments.push(pkt);

        offset = offset.saturating_add(this_len);
    }

    fragments
}

// ---------------------------------------------------------------------------
// IP fragment reassembly
// ---------------------------------------------------------------------------

/// A single received fragment
struct Fragment {
    offset: usize, // byte offset in the original datagram
    data: Vec<u8>, // payload bytes
    more: bool,    // More Fragments flag was set
}

/// Reassembly buffer for a single IP datagram
struct ReassemblyBuffer {
    /// Source IP
    src_ip: Ipv4Addr,
    /// Destination IP
    dst_ip: Ipv4Addr,
    /// Protocol number
    protocol: u8,
    /// Identification
    id: u16,
    /// Collected fragments
    fragments: Vec<Fragment>,
    /// Total expected length (known when we receive the last fragment)
    total_len: Option<usize>,
    /// Tick when first fragment arrived
    created_tick: u64,
    /// Total bytes received so far
    bytes_received: usize,
}

impl ReassemblyBuffer {
    fn new(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, protocol: u8, id: u16) -> Self {
        ReassemblyBuffer {
            src_ip,
            dst_ip,
            protocol,
            id,
            fragments: Vec::new(),
            total_len: None,
            created_tick: crate::time::clock::uptime_ms(),
            bytes_received: 0,
        }
    }

    /// Add a fragment. Returns true if all fragments have been received.
    fn add_fragment(&mut self, offset: usize, data: &[u8], more_fragments: bool) -> bool {
        // Check for overlapping fragments (replace existing)
        self.fragments
            .retain(|f| !(f.offset == offset && f.data.len() == data.len()));

        self.bytes_received += data.len();

        self.fragments.push(Fragment {
            offset,
            data: Vec::from(data),
            more: more_fragments,
        });

        // If this is the last fragment (MF=0), we know the total length
        if !more_fragments {
            self.total_len = Some(offset + data.len());
        }

        // Check if we have all fragments
        self.is_complete()
    }

    /// Check if all fragments have been received.
    fn is_complete(&self) -> bool {
        let total = match self.total_len {
            Some(t) => t,
            None => return false,
        };

        if total > MAX_REASSEMBLED_SIZE {
            return false;
        }

        // Sort fragments by offset and check for gaps
        let mut sorted: Vec<(usize, usize)> = self
            .fragments
            .iter()
            .map(|f| (f.offset, f.data.len()))
            .collect();
        sorted.sort_by_key(|&(off, _)| off);

        let mut expected_offset = 0;
        for (off, len) in &sorted {
            if *off > expected_offset {
                return false; // Gap
            }
            let end = *off + *len;
            if end > expected_offset {
                expected_offset = end;
            }
        }

        expected_offset >= total
    }

    /// Reassemble all fragments into a complete payload.
    fn reassemble(&self) -> Option<Vec<u8>> {
        let total = self.total_len?;
        if total > MAX_REASSEMBLED_SIZE {
            return None;
        }

        let mut payload = alloc::vec![0u8; total];
        for frag in &self.fragments {
            let end = frag.offset + frag.data.len();
            if end <= total {
                payload[frag.offset..end].copy_from_slice(&frag.data);
            }
        }

        Some(payload)
    }

    /// Check if this buffer has timed out.
    fn is_expired(&self) -> bool {
        let now = crate::time::clock::uptime_ms();
        now.saturating_sub(self.created_tick) > REASSEMBLY_TIMEOUT_MS
    }
}

/// Reassembly key: (src_ip_u32, dst_ip_u32, protocol, identification)
type ReassemblyKey = (u32, u32, u8, u16);

/// Global reassembly table
static REASSEMBLY_TABLE: Mutex<BTreeMap<ReassemblyKey, ReassemblyBuffer>> =
    Mutex::new(BTreeMap::new());

/// Process an incoming IP fragment.
/// Returns Some(complete_payload) when all fragments have been received.
/// Returns None if still waiting for more fragments.
pub fn process_fragment(header: &Ipv4Header, payload: &[u8]) -> Option<Vec<u8>> {
    let key: ReassemblyKey = (
        header.src_addr().to_u32(),
        header.dst_addr().to_u32(),
        header.protocol,
        header.id(),
    );

    let offset = header.fragment_offset_bytes();
    let more = header.more_fragments();

    let mut table = REASSEMBLY_TABLE.lock();

    // Evict expired buffers first
    table.retain(|_, buf| !buf.is_expired());

    // Evict if table is full
    if !table.contains_key(&key) && table.len() >= MAX_REASSEMBLY_BUFFERS {
        // Remove the oldest buffer
        let oldest_key = table
            .iter()
            .min_by_key(|(_, buf)| buf.created_tick)
            .map(|(k, _)| *k);
        if let Some(k) = oldest_key {
            table.remove(&k);
        }
    }

    let buffer = table.entry(key).or_insert_with(|| {
        ReassemblyBuffer::new(
            header.src_addr(),
            header.dst_addr(),
            header.protocol,
            header.id(),
        )
    });

    if buffer.add_fragment(offset, payload, more) {
        let result = buffer.reassemble();
        table.remove(&key);
        result
    } else {
        None
    }
}

/// Clean up expired reassembly buffers (called periodically).
pub fn reassembly_gc() {
    REASSEMBLY_TABLE.lock().retain(|_, buf| !buf.is_expired());
}

/// Get the number of active reassembly buffers.
pub fn reassembly_buffer_count() -> usize {
    REASSEMBLY_TABLE.lock().len()
}

// ---------------------------------------------------------------------------
// IP options parsing
// ---------------------------------------------------------------------------

/// Parsed IP option
#[derive(Debug, Clone)]
pub enum IpOption {
    /// End of options
    End,
    /// No operation (padding)
    Nop,
    /// Record Route: list of IPs visited
    RecordRoute(Vec<Ipv4Addr>),
    /// Timestamp: list of (IP, timestamp) pairs
    Timestamp(Vec<(Ipv4Addr, u32)>),
    /// Loose Source Route
    LooseSourceRoute(Vec<Ipv4Addr>),
    /// Strict Source Route
    StrictSourceRoute(Vec<Ipv4Addr>),
    /// Unknown option (type, data)
    Unknown(u8, Vec<u8>),
}

/// Parse IP options from the header bytes beyond the first 20 bytes.
pub fn parse_options(header_data: &[u8]) -> Vec<IpOption> {
    let mut options = Vec::new();
    if header_data.len() <= 20 {
        return options;
    }

    let opt_data = &header_data[20..];
    let mut i = 0;

    while i < opt_data.len() {
        let opt_type = opt_data[i];
        match opt_type {
            OPT_END => {
                options.push(IpOption::End);
                break;
            }
            OPT_NOP => {
                options.push(IpOption::Nop);
                i = i.saturating_add(1);
            }
            OPT_RECORD_ROUTE => {
                if i + 2 >= opt_data.len() {
                    break;
                }
                let opt_len = opt_data[i + 1] as usize;
                let pointer = opt_data[i + 2] as usize;
                if opt_len < 3 || i + opt_len > opt_data.len() {
                    break;
                }
                let mut addrs = Vec::new();
                let mut j = 3; // start after type, length, pointer
                while j + 3 < opt_len.min(pointer.saturating_sub(1)) {
                    let addr = Ipv4Addr([
                        opt_data[i + j],
                        opt_data[i + j + 1],
                        opt_data[i + j + 2],
                        opt_data[i + j + 3],
                    ]);
                    addrs.push(addr);
                    j = j.saturating_add(4);
                }
                options.push(IpOption::RecordRoute(addrs));
                i = i.saturating_add(opt_len);
            }
            OPT_TIMESTAMP => {
                if i + 2 >= opt_data.len() {
                    break;
                }
                let opt_len = opt_data[i + 1] as usize;
                if opt_len < 4 || i + opt_len > opt_data.len() {
                    break;
                }
                let pointer = opt_data[i + 2] as usize;
                let oflw_flag = opt_data[i + 3];
                let flag = oflw_flag & 0x0F;
                let mut entries = Vec::new();

                let mut j = 4;
                while j + 7 < opt_len.min(pointer.saturating_sub(1)) {
                    if flag == 1 || flag == 3 {
                        // IP + timestamp pairs
                        let addr = Ipv4Addr([
                            opt_data[i + j],
                            opt_data[i + j + 1],
                            opt_data[i + j + 2],
                            opt_data[i + j + 3],
                        ]);
                        let ts = u32::from_be_bytes([
                            opt_data[i + j + 4],
                            opt_data[i + j + 5],
                            opt_data[i + j + 6],
                            opt_data[i + j + 7],
                        ]);
                        entries.push((addr, ts));
                        j = j.saturating_add(8);
                    } else {
                        // Timestamp only (no IP)
                        if j + 3 < opt_len {
                            let ts = u32::from_be_bytes([
                                opt_data[i + j],
                                opt_data[i + j + 1],
                                opt_data[i + j + 2],
                                opt_data[i + j + 3],
                            ]);
                            entries.push((Ipv4Addr::ANY, ts));
                            j = j.saturating_add(4);
                        } else {
                            break;
                        }
                    }
                }
                options.push(IpOption::Timestamp(entries));
                i = i.saturating_add(opt_len);
            }
            OPT_LOOSE_SOURCE | OPT_STRICT_SOURCE => {
                if i + 2 >= opt_data.len() {
                    break;
                }
                let opt_len = opt_data[i + 1] as usize;
                if opt_len < 3 || i + opt_len > opt_data.len() {
                    break;
                }
                let mut addrs = Vec::new();
                let mut j = 3;
                while j + 3 < opt_len {
                    let addr = Ipv4Addr([
                        opt_data[i + j],
                        opt_data[i + j + 1],
                        opt_data[i + j + 2],
                        opt_data[i + j + 3],
                    ]);
                    addrs.push(addr);
                    j = j.saturating_add(4);
                }
                if opt_type == OPT_LOOSE_SOURCE {
                    options.push(IpOption::LooseSourceRoute(addrs));
                } else {
                    options.push(IpOption::StrictSourceRoute(addrs));
                }
                i = i.saturating_add(opt_len);
            }
            _ => {
                // Variable-length option
                if i + 1 >= opt_data.len() {
                    break;
                }
                let opt_len = opt_data[i + 1] as usize;
                if opt_len < 2 || i + opt_len > opt_data.len() {
                    break;
                }
                let data = Vec::from(&opt_data[i + 2..i + opt_len]);
                options.push(IpOption::Unknown(opt_type, data));
                i = i.saturating_add(opt_len);
            }
        }
    }

    options
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

struct Ipv4Stats {
    rx_packets: u64,
    tx_packets: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_errors: u64,
    checksum_errors: u64,
    ttl_expired: u64,
    fragments_received: u64,
    fragments_reassembled: u64,
    fragments_created: u64,
    reassembly_timeouts: u64,
}

static IPV4_STATS: Mutex<Ipv4Stats> = Mutex::new(Ipv4Stats {
    rx_packets: 0,
    tx_packets: 0,
    rx_bytes: 0,
    tx_bytes: 0,
    rx_errors: 0,
    checksum_errors: 0,
    ttl_expired: 0,
    fragments_received: 0,
    fragments_reassembled: 0,
    fragments_created: 0,
    reassembly_timeouts: 0,
});

/// Record a received packet.
pub fn stat_rx(bytes: usize) {
    let mut s = IPV4_STATS.lock();
    s.rx_packets = s.rx_packets.saturating_add(1);
    s.rx_bytes = s.rx_bytes.saturating_add(bytes as u64);
}

/// Record a transmitted packet.
pub fn stat_tx(bytes: usize) {
    let mut s = IPV4_STATS.lock();
    s.tx_packets = s.tx_packets.saturating_add(1);
    s.tx_bytes = s.tx_bytes.saturating_add(bytes as u64);
}

/// Record a checksum error.
pub fn stat_checksum_error() {
    let mut s = IPV4_STATS.lock();
    s.checksum_errors = s.checksum_errors.saturating_add(1);
}

/// Record a TTL expiry.
pub fn stat_ttl_expired() {
    let mut s = IPV4_STATS.lock();
    s.ttl_expired = s.ttl_expired.saturating_add(1);
}

/// Get a snapshot of IPv4 statistics.
pub fn get_stats() -> (u64, u64, u64, u64, u64, u64, u64, u64) {
    let s = IPV4_STATS.lock();
    (
        s.rx_packets,
        s.tx_packets,
        s.rx_bytes,
        s.tx_bytes,
        s.rx_errors,
        s.checksum_errors,
        s.ttl_expired,
        s.fragments_received,
    )
}
