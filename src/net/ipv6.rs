/// IPv6 basic support for Genesis — Internet Protocol version 6
///
/// Fixed-size, no-heap, no-float, no-panic implementation of the IPv6 layer.
///
/// Standards basis: RFC 8200 (IPv6), RFC 4861 (NDP), RFC 4862 (SLAAC).
/// All code is original.
///
/// Rules enforced:
///   - NO heap: no Vec, Box, String, format!, alloc::*
///   - NO floats: no f32/f64 literals or casts
///   - NO panics: no unwrap(), expect(), panic!()
///   - Counters: saturating_add / saturating_sub only
///   - All fixed-size static arrays
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Fixed size of the IPv6 header in bytes (no extension headers).
pub const IPV6_HEADER_SIZE: usize = 40;

/// EtherType value for IPv6 frames.
pub const IPV6_ETH_TYPE: u16 = 0x86DD;

/// IPv6 version nibble.
pub const IPV6_VERSION: u8 = 6;

/// Maximum number of IPv6 addresses per network interface.
pub const MAX_IPV6_ADDRS: usize = 8;

/// Maximum number of network interfaces tracked.
pub const MAX_INTERFACES: usize = 8;

/// Default hop limit for outgoing packets.
pub const DEFAULT_HOP_LIMIT: u8 = 64;

// ---------------------------------------------------------------------------
// IPv6 next-header / protocol constants
// ---------------------------------------------------------------------------

pub mod next_header {
    pub const HOP_BY_HOP: u8 = 0;
    pub const TCP: u8 = 6;
    pub const UDP: u8 = 17;
    pub const ROUTING: u8 = 43;
    pub const FRAGMENT: u8 = 44;
    pub const ICMPV6: u8 = 58;
    pub const NO_NEXT: u8 = 59;
    pub const DEST_OPTS: u8 = 60;
}

// ---------------------------------------------------------------------------
// ICMPv6 type codes
// ---------------------------------------------------------------------------

pub mod icmpv6_type {
    pub const ECHO_REQUEST: u8 = 128;
    pub const ECHO_REPLY: u8 = 129;
    pub const ROUTER_SOLICITATION: u8 = 133;
    pub const ROUTER_ADVERTISEMENT: u8 = 134;
    pub const NEIGHBOR_SOLICITATION: u8 = 135;
    pub const NEIGHBOR_ADVERTISEMENT: u8 = 136;
}

// ---------------------------------------------------------------------------
// Ipv6Addr — 128-bit address (16 bytes)
// ---------------------------------------------------------------------------

/// A 128-bit IPv6 address stored as 16 raw bytes in network (big-endian) order.
#[derive(Copy, Clone)]
pub struct Ipv6Addr(pub [u8; 16]);

impl Ipv6Addr {
    /// The unspecified address `::` (all zeros).
    pub const UNSPECIFIED: Self = Self([0u8; 16]);

    /// The loopback address `::1`.
    pub const LOOPBACK: Self = Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

    /// All-nodes multicast address `FF02::1`.
    pub const ALL_NODES: Self = Self([0xFF, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01]);

    /// All-routers multicast address `FF02::2`.
    pub const ALL_ROUTERS: Self = Self([0xFF, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02]);

    /// True if this is the loopback address (`::1`).
    #[inline]
    pub fn is_loopback(&self) -> bool {
        self.0 == Self::LOOPBACK.0
    }

    /// True if this is the unspecified address (`::`).
    #[inline]
    pub fn is_unspecified(&self) -> bool {
        self.0 == Self::UNSPECIFIED.0
    }

    /// True if this is a multicast address (`FF00::/8`).
    #[inline]
    pub fn is_multicast(&self) -> bool {
        self.0[0] == 0xFF
    }

    /// True if this is a link-local unicast address (`FE80::/10`).
    #[inline]
    pub fn is_link_local(&self) -> bool {
        self.0[0] == 0xFE && (self.0[1] & 0xC0) == 0x80
    }

    /// True if this is a site-local unicast address (`FEC0::/10`, deprecated).
    #[inline]
    pub fn is_site_local(&self) -> bool {
        self.0[0] == 0xFE && (self.0[1] & 0xC0) == 0xC0
    }

    /// Byte-level equality.
    #[inline]
    pub fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl PartialEq for Ipv6Addr {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

// ---------------------------------------------------------------------------
// Ipv6Header — fixed 40-byte header (RFC 8200 §3)
// ---------------------------------------------------------------------------

/// The fixed 40-byte IPv6 header.
///
/// Layout (all big-endian in memory):
///   Bits  0.. 3 : Version (always 6)
///   Bits  4..11 : Traffic Class
///   Bits 12..31 : Flow Label
///   Bits 32..47 : Payload Length (bytes after this header)
///   Bits 48..55 : Next Header
///   Bits 56..63 : Hop Limit
///   Bits 64..191 : Source Address (128 bits)
///   Bits 192..319: Destination Address (128 bits)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Ipv6Header {
    /// version(4) | traffic-class(8) | flow-label(20) — stored as 4 bytes BE.
    pub version_tc_flow: [u8; 4],
    /// Payload length in bytes (big-endian), excluding this 40-byte header.
    pub payload_len: [u8; 2],
    /// Next-header identifier (TCP=6, UDP=17, ICMPv6=58, …).
    pub next_header: u8,
    /// Hop limit (decremented by each router, packet dropped at 0).
    pub hop_limit: u8,
    /// Source IPv6 address (16 bytes, network order).
    pub src: [u8; 16],
    /// Destination IPv6 address (16 bytes, network order).
    pub dst: [u8; 16],
}

impl Ipv6Header {
    /// Construct a new IPv6 header with version=6, TC=0, Flow=0.
    pub fn new(
        src: Ipv6Addr,
        dst: Ipv6Addr,
        next_hdr: u8,
        payload_len: u16,
        hop_limit: u8,
    ) -> Self {
        Self {
            version_tc_flow: [0x60, 0x00, 0x00, 0x00], // ver=6, TC=0, flow=0
            payload_len: payload_len.to_be_bytes(),
            next_header: next_hdr,
            hop_limit,
            src: src.0,
            dst: dst.0,
        }
    }

    /// Return the next-header field.
    #[inline]
    pub fn next_header(&self) -> u8 {
        self.next_header
    }

    /// Return the payload length.
    #[inline]
    pub fn payload_len(&self) -> u16 {
        u16::from_be_bytes(self.payload_len)
    }

    /// Return the source address.
    #[inline]
    pub fn src_addr(&self) -> Ipv6Addr {
        Ipv6Addr(self.src)
    }

    /// Return the destination address.
    #[inline]
    pub fn dst_addr(&self) -> Ipv6Addr {
        Ipv6Addr(self.dst)
    }

    /// Return the version nibble (should always be 6 for valid packets).
    #[inline]
    pub fn version(&self) -> u8 {
        (self.version_tc_flow[0] >> 4) & 0x0F
    }

    /// Return the traffic class byte.
    #[inline]
    pub fn traffic_class(&self) -> u8 {
        ((self.version_tc_flow[0] & 0x0F) << 4) | ((self.version_tc_flow[1] >> 4) & 0x0F)
    }

    /// Return the 20-bit flow label.
    #[inline]
    pub fn flow_label(&self) -> u32 {
        ((self.version_tc_flow[1] as u32 & 0x0F) << 16)
            | ((self.version_tc_flow[2] as u32) << 8)
            | (self.version_tc_flow[3] as u32)
    }

    /// Serialize this header into the first 40 bytes of `buf`.
    /// Returns false if `buf` is shorter than 40 bytes.
    pub fn write_to(&self, buf: &mut [u8]) -> bool {
        if buf.len() < IPV6_HEADER_SIZE {
            return false;
        }
        buf[0..4].copy_from_slice(&self.version_tc_flow);
        buf[4..6].copy_from_slice(&self.payload_len);
        buf[6] = self.next_header;
        buf[7] = self.hop_limit;
        buf[8..24].copy_from_slice(&self.src);
        buf[24..40].copy_from_slice(&self.dst);
        true
    }

    /// Parse a header from the first 40 bytes of `buf`.
    /// Returns `None` if `buf` is too short or version != 6.
    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < IPV6_HEADER_SIZE {
            return None;
        }
        if (buf[0] >> 4) != IPV6_VERSION {
            return None;
        }
        let mut h = Self {
            version_tc_flow: [0; 4],
            payload_len: [0; 2],
            next_header: 0,
            hop_limit: 0,
            src: [0; 16],
            dst: [0; 16],
        };
        h.version_tc_flow.copy_from_slice(&buf[0..4]);
        h.payload_len.copy_from_slice(&buf[4..6]);
        h.next_header = buf[6];
        h.hop_limit = buf[7];
        h.src.copy_from_slice(&buf[8..24]);
        h.dst.copy_from_slice(&buf[24..40]);
        Some(h)
    }
}

// ---------------------------------------------------------------------------
// Ipv6IfAddr — per-interface address entry
// ---------------------------------------------------------------------------

/// One IPv6 address assigned to a network interface.
#[derive(Copy, Clone)]
pub struct Ipv6IfAddr {
    /// The IPv6 address.
    pub addr: Ipv6Addr,
    /// Prefix length in bits (e.g. 64 for a /64 network, 128 for loopback).
    pub prefix_len: u8,
    /// Address scope: 0 = node-local, 1 = link-local, 2 = global.
    pub scope: u8,
    /// Whether this slot is occupied.
    pub active: bool,
}

impl Ipv6IfAddr {
    /// Return an empty (inactive) slot.
    pub const fn empty() -> Self {
        Self {
            addr: Ipv6Addr::UNSPECIFIED,
            prefix_len: 0,
            scope: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static address table — [interface][slot]
// ---------------------------------------------------------------------------

static IPV6_ADDRS: Mutex<[[Ipv6IfAddr; MAX_IPV6_ADDRS]; MAX_INTERFACES]> = Mutex::new(
    [[Ipv6IfAddr {
        addr: Ipv6Addr::UNSPECIFIED,
        prefix_len: 0,
        scope: 0,
        active: false,
    }; MAX_IPV6_ADDRS]; MAX_INTERFACES],
);

// ---------------------------------------------------------------------------
// Address management
// ---------------------------------------------------------------------------

/// Add an IPv6 address to `ifindex`.
///
/// Returns `true` on success; `false` if `ifindex >= MAX_INTERFACES`, if the
/// address is already assigned to this interface, or if all slots are full.
pub fn ipv6_add_addr(ifindex: u8, addr: Ipv6Addr, prefix_len: u8) -> bool {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return false;
    }
    let mut table = IPV6_ADDRS.lock();

    // Duplicate check
    for slot in table[idx].iter() {
        if slot.active && slot.addr.eq(&addr) {
            return true; // already present — idempotent
        }
    }

    // Classify scope
    let scope: u8 = if addr.is_loopback() {
        0 // node-local
    } else if addr.is_link_local() {
        1 // link-local
    } else {
        2 // global
    };

    // Find a free slot
    for slot in table[idx].iter_mut() {
        if !slot.active {
            slot.addr = addr;
            slot.prefix_len = prefix_len;
            slot.scope = scope;
            slot.active = true;
            return true;
        }
    }
    false // no free slot
}

/// Remove an IPv6 address from `ifindex`.
///
/// Returns `true` if the address was found and removed; `false` otherwise.
pub fn ipv6_del_addr(ifindex: u8, addr: Ipv6Addr) -> bool {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return false;
    }
    let mut table = IPV6_ADDRS.lock();
    for slot in table[idx].iter_mut() {
        if slot.active && slot.addr.eq(&addr) {
            *slot = Ipv6IfAddr::empty();
            return true;
        }
    }
    false
}

/// Return the first non-loopback active address on `ifindex`, if any.
pub fn ipv6_get_addr(ifindex: u8) -> Option<Ipv6Addr> {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return None;
    }
    let table = IPV6_ADDRS.lock();
    for slot in table[idx].iter() {
        if slot.active && !slot.addr.is_loopback() {
            return Some(slot.addr);
        }
    }
    None
}

/// Return the first active address on `ifindex` that matches `scope`.
pub fn ipv6_get_addr_by_scope(ifindex: u8, scope: u8) -> Option<Ipv6Addr> {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return None;
    }
    let table = IPV6_ADDRS.lock();
    for slot in table[idx].iter() {
        if slot.active && slot.scope == scope {
            return Some(slot.addr);
        }
    }
    None
}

/// True if `addr` is assigned to any interface.
pub fn ipv6_is_local_addr(addr: &Ipv6Addr) -> bool {
    let table = IPV6_ADDRS.lock();
    for iface in table.iter() {
        for slot in iface.iter() {
            if slot.active && slot.addr.eq(addr) {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Header construction helper
// ---------------------------------------------------------------------------

/// Build an `Ipv6Header` with standard defaults (version=6, TC=0, flow=0).
pub fn ipv6_build_header(
    src: Ipv6Addr,
    dst: Ipv6Addr,
    next_hdr: u8,
    payload_len: u16,
) -> Ipv6Header {
    Ipv6Header::new(src, dst, next_hdr, payload_len, DEFAULT_HOP_LIMIT)
}

// ---------------------------------------------------------------------------
// Extension header walker (no alloc required — iterates in place)
// ---------------------------------------------------------------------------

/// Walk IPv6 extension headers to locate the upper-layer protocol.
///
/// `buf`            : slice starting immediately after the 40-byte fixed header.
/// `first_next_hdr` : `next_header` field from the fixed header.
///
/// Returns `(final_next_header, total_extension_bytes_consumed)`.
pub fn walk_extension_headers(buf: &[u8], first_next_hdr: u8) -> (u8, usize) {
    let mut current_nh = first_next_hdr;
    let mut offset = 0usize;

    loop {
        match current_nh {
            // Fragment header: fixed 8 bytes, no length field
            next_header::FRAGMENT => {
                if offset.saturating_add(8) > buf.len() {
                    return (current_nh, offset);
                }
                let next = buf[offset];
                offset = offset.saturating_add(8);
                current_nh = next;
            }

            // Variable-length extension headers: (len+1)*8 bytes
            next_header::HOP_BY_HOP | next_header::ROUTING | next_header::DEST_OPTS => {
                if offset.saturating_add(2) > buf.len() {
                    return (current_nh, offset);
                }
                let next = buf[offset];
                let hdr_ext_len = buf[offset.saturating_add(1)] as usize;
                let hdr_bytes = hdr_ext_len.saturating_add(1).saturating_mul(8);
                if offset.saturating_add(hdr_bytes) > buf.len() {
                    return (current_nh, offset);
                }
                offset = offset.saturating_add(hdr_bytes);
                current_nh = next;
            }

            // Upper-layer protocol or NO_NEXT — stop walking
            _ => return (current_nh, offset),
        }
    }
}

// ---------------------------------------------------------------------------
// Input processing
// ---------------------------------------------------------------------------

/// Process an incoming raw IPv6 frame (starting at the IPv6 fixed header).
///
/// Parses the header, checks that the destination is a local address or
/// multicast, then dispatches to the appropriate upper-layer handler.
///
/// `payload` : raw bytes starting at the IPv6 fixed header.
/// `len`     : number of valid bytes in `payload` (may be < payload.len()).
pub fn ipv6_input(payload: &[u8], len: usize) {
    let valid_len = len.min(payload.len());
    if valid_len < IPV6_HEADER_SIZE {
        return;
    }

    let hdr = match Ipv6Header::parse(&payload[..valid_len]) {
        Some(h) => h,
        None => return,
    };

    // Accept only packets addressed to a local address or multicast
    let dst = hdr.dst_addr();
    if !dst.is_multicast() && !ipv6_is_local_addr(&dst) {
        return;
    }

    let payload_start = IPV6_HEADER_SIZE;
    let payload_end = payload_start
        .saturating_add(hdr.payload_len() as usize)
        .min(valid_len);

    if payload_end < payload_start {
        return;
    }

    let upper_data = &payload[payload_start..payload_end];

    // Walk past any extension headers
    let (final_nh, ext_len) = walk_extension_headers(upper_data, hdr.next_header());
    let upper_payload = if ext_len < upper_data.len() {
        &upper_data[ext_len..]
    } else {
        &[]
    };

    // Dispatch to upper-layer handlers
    match final_nh {
        next_header::ICMPV6 => {
            handle_icmpv6(hdr.src_addr(), hdr.dst_addr(), upper_payload);
        }
        next_header::TCP | next_header::UDP => {
            // Delivered to transport layer — not implemented here
            let _ = upper_payload;
        }
        next_header::NO_NEXT => {
            // No upper-layer data — valid (e.g. some hop-by-hop use cases)
        }
        _ => {
            // Unknown next header — silently discard
        }
    }
}

// ---------------------------------------------------------------------------
// ICMPv6 dispatch (minimal, no alloc)
// ---------------------------------------------------------------------------

/// Minimal ICMPv6 input handler.
///
/// Reads the type byte and dispatches.  Does not build responses — that
/// would require an output path not yet wired here.
fn handle_icmpv6(src: Ipv6Addr, dst: Ipv6Addr, data: &[u8]) {
    let _ = src;
    let _ = dst;
    if data.is_empty() {
        return;
    }
    let _icmp_type = data[0];
    // Type-specific handling hooks go here when the output path is available.
}

// ---------------------------------------------------------------------------
// Address classification helpers (free-standing, work on raw byte arrays)
// ---------------------------------------------------------------------------

/// True if `addr` is a link-local unicast address (`FE80::/10`).
pub fn is_link_local(addr: [u8; 16]) -> bool {
    addr[0] == 0xFE && (addr[1] & 0xC0) == 0x80
}

/// True if `addr` is a multicast address (`FF00::/8`).
pub fn is_multicast(addr: [u8; 16]) -> bool {
    addr[0] == 0xFF
}

/// Derive the solicited-node multicast address `FF02::1:FF<last 3 bytes>`.
pub fn solicited_node_multicast(addr: [u8; 16]) -> [u8; 16] {
    let mut mc = [0u8; 16];
    mc[0] = 0xFF;
    mc[1] = 0x02;
    mc[11] = 0x01;
    mc[12] = 0xFF;
    mc[13] = addr[13];
    mc[14] = addr[14];
    mc[15] = addr[15];
    mc
}

/// Build an EUI-64 derived link-local address (`FE80::/10`) from a 48-bit MAC.
///
/// Inserts `FF:FE` between bytes 3 and 4 of the MAC and flips the U/L bit.
pub fn build_link_local_addr(mac: [u8; 6]) -> [u8; 16] {
    let mut addr = [0u8; 16];
    addr[0] = 0xFE;
    addr[1] = 0x80;
    // bytes [2..8] = 0  (link-local prefix padding)
    addr[8] = mac[0] ^ 0x02; // flip U/L bit
    addr[9] = mac[1];
    addr[10] = mac[2];
    addr[11] = 0xFF;
    addr[12] = 0xFE;
    addr[13] = mac[3];
    addr[14] = mac[4];
    addr[15] = mac[5];
    addr
}

// ---------------------------------------------------------------------------
// ICMPv6 checksum (pseudo-header per RFC 2460 §8.1)
// ---------------------------------------------------------------------------

/// Compute the ICMPv6 checksum over the IPv6 pseudo-header and `icmpv6_payload`.
///
/// Pseudo-header: src(16) | dst(16) | upper-length(4 BE) | zeros(3) | NH=58(1)
pub fn icmpv6_checksum(src: [u8; 16], dst: [u8; 16], icmpv6_payload: &[u8]) -> u16 {
    let length = icmpv6_payload.len() as u32;
    let mut sum: u32 = 0;

    // Accumulate 16-bit big-endian words from a byte slice
    let mut add_bytes = |bytes: &[u8]| {
        let mut i = 0usize;
        while i.saturating_add(1) < bytes.len() {
            let word = ((bytes[i] as u32) << 8) | (bytes[i.saturating_add(1)] as u32);
            sum = sum.wrapping_add(word);
            i = i.saturating_add(2);
        }
        if i < bytes.len() {
            sum = sum.wrapping_add((bytes[i] as u32) << 8);
        }
    };

    add_bytes(&src);
    add_bytes(&dst);
    add_bytes(&length.to_be_bytes());
    add_bytes(&[0u8, 0u8, 0u8, next_header::ICMPV6]);
    add_bytes(icmpv6_payload);

    // Fold 32-bit sum into 16 bits
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF).wrapping_add(sum >> 16);
    }
    !(sum as u16)
}

// ---------------------------------------------------------------------------
// NDP packet builders (fixed-size output buffers, no alloc)
// ---------------------------------------------------------------------------

/// Build an ICMPv6 Neighbor Solicitation message (type 135) into `buf`.
///
/// `buf` must be at least 32 bytes (24-byte NS body + 8-byte SLLA option).
/// Returns the number of bytes written, or 0 on error.
///
/// The checksum field is zeroed — caller must fill it.
pub fn build_neighbor_solicitation(
    buf: &mut [u8; 32],
    target: [u8; 16],
    our_mac: [u8; 6],
) -> usize {
    buf[0] = icmpv6_type::NEIGHBOR_SOLICITATION;
    buf[1] = 0; // code
    buf[2] = 0; // checksum high (caller fills)
    buf[3] = 0; // checksum low
    buf[4] = 0; // reserved
    buf[5] = 0;
    buf[6] = 0;
    buf[7] = 0;
    buf[8..24].copy_from_slice(&target);
    buf[24] = 1; // option type: Source Link-Layer Address
    buf[25] = 1; // length in units of 8 octets
    buf[26..32].copy_from_slice(&our_mac);
    32
}

/// Build an ICMPv6 Neighbor Advertisement message (type 136) into `buf`.
///
/// `buf` must be at least 32 bytes.  Checksum is zeroed — caller must fill.
pub fn build_neighbor_advertisement(
    buf: &mut [u8; 32],
    target: [u8; 16],
    our_mac: [u8; 6],
    solicited: bool,
    override_flag: bool,
) -> usize {
    buf[0] = icmpv6_type::NEIGHBOR_ADVERTISEMENT;
    buf[1] = 0; // code
    buf[2] = 0; // checksum high
    buf[3] = 0; // checksum low
                // Flags byte (bit 6 = Solicited, bit 5 = Override)
    buf[4] = (if solicited { 0x40u8 } else { 0u8 }) | (if override_flag { 0x20u8 } else { 0u8 });
    buf[5] = 0; // reserved
    buf[6] = 0;
    buf[7] = 0;
    buf[8..24].copy_from_slice(&target);
    buf[24] = 2; // option type: Target Link-Layer Address
    buf[25] = 1; // length = 1 (8 octets)
    buf[26..32].copy_from_slice(&our_mac);
    32
}

/// Build a complete Neighbor Solicitation with a valid ICMPv6 checksum.
///
/// The solicitation is sent from `our_addr` to the solicited-node multicast
/// address derived from `target`.
pub fn ndp_neighbor_solicitation(
    buf: &mut [u8; 32],
    our_addr: [u8; 16],
    target: [u8; 16],
    our_mac: [u8; 6],
) -> usize {
    let written = build_neighbor_solicitation(buf, target, our_mac);
    let sol_node = solicited_node_multicast(target);
    let cksum = icmpv6_checksum(our_addr, sol_node, &buf[..written]);
    buf[2] = (cksum >> 8) as u8;
    buf[3] = cksum as u8;
    written
}

/// Build a complete Neighbor Advertisement with a valid ICMPv6 checksum.
pub fn ndp_neighbor_advertisement(
    buf: &mut [u8; 32],
    our_addr: [u8; 16],
    dst_addr: [u8; 16],
    target: [u8; 16],
    our_mac: [u8; 6],
    solicited: bool,
) -> usize {
    let written = build_neighbor_advertisement(buf, target, our_mac, solicited, true);
    let cksum = icmpv6_checksum(our_addr, dst_addr, &buf[..written]);
    buf[2] = (cksum >> 8) as u8;
    buf[3] = cksum as u8;
    written
}

// ---------------------------------------------------------------------------
// State accessors
// ---------------------------------------------------------------------------

/// Return the primary link-local address on interface `ifindex`, if configured.
pub fn get_link_local(ifindex: u8) -> Option<[u8; 16]> {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return None;
    }
    let table = IPV6_ADDRS.lock();
    for slot in table[idx].iter() {
        if slot.active && slot.addr.is_link_local() {
            return Some(slot.addr.0);
        }
    }
    None
}

/// Copy all active addresses for `ifindex` into `out`.
///
/// Returns the number of addresses written (at most `out.len()`).
pub fn get_addresses(ifindex: u8, out: &mut [([u8; 16], u8)]) -> usize {
    let idx = ifindex as usize;
    if idx >= MAX_INTERFACES {
        return 0;
    }
    let table = IPV6_ADDRS.lock();
    let mut count = 0usize;
    for slot in table[idx].iter() {
        if slot.active {
            if count < out.len() {
                out[count] = (slot.addr.0, slot.prefix_len);
            }
            count = count.saturating_add(1);
        }
    }
    count.min(out.len())
}

// ---------------------------------------------------------------------------
// Module initialisation
// ---------------------------------------------------------------------------

/// Initialise the IPv6 subsystem.
///
/// - Assigns `::1/128` (loopback) to interface 0.
/// - Assigns `FE80::1/64` (static link-local) to interface 1.
pub fn init() {
    // Interface 0: loopback ::1/128
    ipv6_add_addr(0, Ipv6Addr::LOOPBACK, 128);

    // Interface 1: link-local FE80::1/64
    // Using a static address rather than EUI-64 MAC derivation because no
    // NIC MAC is available at early init time.
    let ll = Ipv6Addr([
        0xFE, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01,
    ]);
    ipv6_add_addr(1, ll, 64);

    crate::serial_println!("[ipv6] IPv6 basic support initialized");
}
