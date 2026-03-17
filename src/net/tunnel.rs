use super::arp;
use super::ipv4;
use super::Ipv4Addr;
use crate::serial_println;
/// IP tunnel protocols — IPIP (RFC 2003), GRE (RFC 2784/2890), VXLAN (RFC 7348)
///
/// Provides IP-in-IP and Generic Routing Encapsulation tunnels for overlay
/// networks, plus a VXLAN stub for L2 overlays over UDP.
///
/// Design constraints (bare-metal kernel rules):
///   - No heap: all storage is fixed-size static arrays
///   - No panics: all error paths return Option/bool
///   - No float casts: no `as f64` / `as f32` anywhere
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile (N/A here — pure protocol logic)
///
/// Inspired by: Linux ip_tunnel.c, RFC 2003, RFC 2784, RFC 2890, RFC 7348.
/// All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// IP protocol numbers
// ---------------------------------------------------------------------------

const IPPROTO_IPIP: u8 = 4;
const IPPROTO_GRE: u8 = 47;

// ---------------------------------------------------------------------------
// GRE flag bits (in the 16-bit flags+version word, big-endian bit positions)
// Bit 15 (0x8000) = Checksum Present
// Bit 13 (0x2000) = Key Present
// Bit 12 (0x1000) = Sequence Present
// ---------------------------------------------------------------------------

const GRE_F_CHECKSUM: u16 = 0x8000;
const GRE_F_KEY: u16 = 0x2000;
const GRE_F_SEQUENCE: u16 = 0x1000;

/// GRE EtherType for IPv4 inner payload
pub const GRE_PROTO_IPV4: u16 = 0x0800;
/// GRE EtherType for IPv6 inner payload
pub const GRE_PROTO_IPV6: u16 = 0x86DD;
/// GRE EtherType for Transparent Ethernet (GRETAP)
pub const GRE_PROTO_ETHER: u16 = 0x6558;

/// VXLAN UDP destination port (IANA-assigned)
pub const VXLAN_PORT: u16 = 4789;

/// IPv4 header size (no options)
const IPV4_HDR: usize = 20;

/// Maximum number of IP tunnels
const MAX_TUNNELS: usize = 16;

/// Maximum number of VXLAN tunnels
const MAX_VXLAN: usize = 8;

/// Maximum output buffer for a single encapsulated packet
/// (Ethernet MTU 1500 + some headroom)
const MAX_PKT: usize = 1500;

// ---------------------------------------------------------------------------
// TunnelProto
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum TunnelProto {
    Ipip,
    Gre,
    Sit,
    Vxlan,
}

// ---------------------------------------------------------------------------
// TunnelConfig — Copy, const-constructible, stored in a static array
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct TunnelConfig {
    /// Unique tunnel identifier (0 = empty slot)
    pub tunnel_id: u32,
    pub proto: TunnelProto,
    /// Outer source IP
    pub local_ip: [u8; 4],
    /// Outer destination IP
    pub remote_ip: [u8; 4],
    /// Outer TTL (default 64)
    pub ttl: u8,
    /// Outer TOS/DSCP (0 = copy from inner)
    pub tos: u8,
    /// Enable path MTU discovery
    pub pmtu_disc: bool,
    /// GRE key value (ignored when use_key=false)
    pub key: u32,
    /// Include GRE key field in header
    pub use_key: bool,
    /// Include GRE sequence-number field in header
    pub sequence: bool,
    /// Current TX sequence counter (wrapping)
    pub tx_seq: u32,
    /// Last accepted RX sequence
    pub rx_seq: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub active: bool,
}

impl TunnelConfig {
    pub const fn empty() -> Self {
        TunnelConfig {
            tunnel_id: 0,
            proto: TunnelProto::Ipip,
            local_ip: [0; 4],
            remote_ip: [0; 4],
            ttl: 64,
            tos: 0,
            pmtu_disc: false,
            key: 0,
            use_key: false,
            sequence: false,
            tx_seq: 0,
            rx_seq: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_packets: 0,
            tx_packets: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// VXLAN per-tunnel config
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct VxlanConfig {
    /// VXLAN Network Identifier (24-bit)
    pub vni: u32,
    pub local_ip: [u8; 4],
    pub dst_port: u16,
    pub active: bool,
}

impl VxlanConfig {
    pub const fn empty() -> Self {
        VxlanConfig {
            vni: 0,
            local_ip: [0; 4],
            dst_port: VXLAN_PORT,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TUNNELS: Mutex<[TunnelConfig; MAX_TUNNELS]> =
    Mutex::new([TunnelConfig::empty(); MAX_TUNNELS]);

static VXLAN_TUNNELS: Mutex<[VxlanConfig; MAX_VXLAN]> =
    Mutex::new([VxlanConfig::empty(); MAX_VXLAN]);

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

pub fn init() {
    // Nothing to initialise at runtime — statics are const-initialised.
    serial_println!(
        "  Net: tunnel subsystem ready (IPIP/GRE/VXLAN, {} slots)",
        MAX_TUNNELS
    );
}

// ===========================================================================
// IPIP (IP-in-IP, RFC 2003)
// ===========================================================================

/// Encapsulate `inner_pkt` inside a new IPv4 header.
///
/// Writes the outer IPv4 header (20 bytes) followed by `inner_pkt` into `out`.
/// Returns the total byte count written, or 0 if `inner_pkt` is too large.
pub fn ipip_encap(tun: &TunnelConfig, inner_pkt: &[u8], out: &mut [u8; MAX_PKT]) -> usize {
    // Outer header (20) + inner payload must fit.
    let total = IPV4_HDR.saturating_add(inner_pkt.len());
    if total > MAX_PKT {
        return 0;
    }

    // Build outer IPv4 header: proto=4 (IPIP), length = total.
    let payload_len = inner_pkt.len() as u16;
    let ip_hdr = ipv4::build_header(
        Ipv4Addr(tun.local_ip),
        Ipv4Addr(tun.remote_ip),
        IPPROTO_IPIP,
        payload_len,
        tun.ttl,
    );
    let hdr_bytes = ipv4::header_to_bytes(&ip_hdr);

    // Write outer header.
    out[..IPV4_HDR].copy_from_slice(&hdr_bytes);
    // Copy inner packet after outer header.
    out[IPV4_HDR..total].copy_from_slice(inner_pkt);

    total
}

/// Parse the outer IPv4 header of an IPIP packet.
///
/// Returns `(inner_start_offset, inner_length)` for the encapsulated IP packet.
/// Returns `None` if the outer header is invalid or `protocol != 4`.
pub fn ipip_decap(data: &[u8], len: usize) -> Option<(usize, usize)> {
    if len < IPV4_HDR || data.len() < IPV4_HDR {
        return None;
    }
    let (ip_hdr, _payload) = ipv4::Ipv4Header::parse(&data[..len])?;

    // Must be IPIP (proto 4).
    if ip_hdr.protocol != IPPROTO_IPIP {
        return None;
    }

    let ihl = ip_hdr.ihl() as usize * 4;
    if ihl < IPV4_HDR || len < ihl {
        return None;
    }
    let inner_start = ihl;
    let inner_len = len.saturating_sub(ihl);

    if inner_len == 0 {
        return None;
    }

    Some((inner_start, inner_len))
}

// ===========================================================================
// GRE (RFC 2784 / RFC 2890)
// ===========================================================================

/// Build the GRE header into `out[..N]`.
///
/// Returns the number of bytes written (4, 8, 12, or 16 depending on flags).
/// The caller is responsible for providing a buffer of at least 20 bytes.
pub fn gre_build_header(tun: &TunnelConfig, inner_proto: u16, out: &mut [u8; 20]) -> usize {
    let mut flags: u16 = 0;

    // Key Present (bit 13)
    if tun.use_key {
        flags |= GRE_F_KEY;
    }
    // Sequence Number Present (bit 12)
    if tun.sequence {
        flags |= GRE_F_SEQUENCE;
    }
    // Checksum Present (bit 15) — not enabled via TunnelConfig; reserved for future.
    // flags |= GRE_F_CHECKSUM;   // omit unless explicitly requested

    // Flags word (big-endian).
    let fw = flags.to_be_bytes();
    out[0] = fw[0];
    out[1] = fw[1];

    // Protocol word (big-endian).
    let pw = inner_proto.to_be_bytes();
    out[2] = pw[0];
    out[3] = pw[1];

    let mut offset = 4usize;

    // Optional Key field.
    if tun.use_key {
        let kb = tun.key.to_be_bytes();
        out[offset] = kb[0];
        out[offset + 1] = kb[1];
        out[offset + 2] = kb[2];
        out[offset + 3] = kb[3];
        offset = offset.saturating_add(4);
    }

    // Optional Sequence Number field.
    if tun.sequence {
        let sb = tun.tx_seq.to_be_bytes();
        out[offset] = sb[0];
        out[offset + 1] = sb[1];
        out[offset + 2] = sb[2];
        out[offset + 3] = sb[3];
        offset = offset.saturating_add(4);
    }

    offset
}

/// Encapsulate `inner_pkt` inside an outer IPv4/GRE frame.
///
/// Mutates `tun.tx_seq` if sequence numbering is enabled.
/// Returns total byte count written into `out`, or 0 on overflow.
pub fn gre_encap(
    tun: &mut TunnelConfig,
    inner_pkt: &[u8],
    inner_proto: u16,
    out: &mut [u8; MAX_PKT],
) -> usize {
    // Build GRE header into a temporary 20-byte buffer.
    let mut gre_buf = [0u8; 20];
    let gre_len = gre_build_header(tun, inner_proto, &mut gre_buf);

    // Total payload = GRE header + inner packet.
    let gre_payload_len = gre_len.saturating_add(inner_pkt.len());

    // Outer IPv4 + GRE header + inner packet must fit in MAX_PKT.
    let total = IPV4_HDR.saturating_add(gre_payload_len);
    if total > MAX_PKT {
        return 0;
    }

    // Build outer IPv4 header (proto = 47, GRE).
    let ip_payload_len = gre_payload_len as u16;
    let ip_hdr = ipv4::build_header(
        Ipv4Addr(tun.local_ip),
        Ipv4Addr(tun.remote_ip),
        IPPROTO_GRE,
        ip_payload_len,
        tun.ttl,
    );
    let hdr_bytes = ipv4::header_to_bytes(&ip_hdr);

    // Write outer IPv4 header.
    out[..IPV4_HDR].copy_from_slice(&hdr_bytes);

    // Write GRE header.
    let gre_start = IPV4_HDR;
    let gre_end = gre_start.saturating_add(gre_len);
    out[gre_start..gre_end].copy_from_slice(&gre_buf[..gre_len]);

    // Write inner packet.
    let inner_start = gre_end;
    let inner_end = inner_start.saturating_add(inner_pkt.len());
    out[inner_start..inner_end].copy_from_slice(inner_pkt);

    // Advance TX sequence number (wrapping).
    if tun.sequence {
        tun.tx_seq = tun.tx_seq.wrapping_add(1);
    }

    total
}

/// Parse a raw GRE payload (starting at the GRE flags word, after the outer
/// IPv4 header has been stripped).
///
/// Returns `(inner_proto, inner_start_offset, inner_length)`.
/// Returns `None` on invalid or truncated header.
pub fn gre_decap(data: &[u8], len: usize) -> Option<(u16, usize, usize)> {
    // Need at least 4 bytes: flags(2) + protocol(2).
    if len < 4 || data.len() < 4 {
        return None;
    }

    let flags = u16::from_be_bytes([data[0], data[1]]);
    let protocol = u16::from_be_bytes([data[2], data[3]]);
    let mut offset = 4usize;

    // Checksum Present (bit 15) — skip 4 bytes (checksum u16 + reserved u16).
    if flags & GRE_F_CHECKSUM != 0 {
        if len < offset.saturating_add(4) {
            return None;
        }
        offset = offset.saturating_add(4);
    }

    // Key Present (bit 13) — skip 4 bytes.
    if flags & GRE_F_KEY != 0 {
        if len < offset.saturating_add(4) {
            return None;
        }
        offset = offset.saturating_add(4);
    }

    // Sequence Number Present (bit 12) — skip 4 bytes.
    if flags & GRE_F_SEQUENCE != 0 {
        if len < offset.saturating_add(4) {
            return None;
        }
        offset = offset.saturating_add(4);
    }

    if offset >= len {
        return None;
    }

    let inner_len = len.saturating_sub(offset);
    if inner_len == 0 {
        return None;
    }

    Some((protocol, offset, inner_len))
}

// ===========================================================================
// VXLAN (RFC 7348) — stub implementation
// ===========================================================================
//
// Frame layout:
//
//   [Outer Ethernet]  (added by caller, not here)
//   [Outer IPv4 hdr]  20 bytes
//   [Outer UDP hdr]    8 bytes  (src_port=variable, dst_port=4789)
//   [VXLAN hdr]        8 bytes  (flags=0x08, reserved=0, VNI[24], reserved=0)
//   [Inner Ethernet frame]
//
// vxlan_encap writes IPv4 + UDP + VXLAN + inner frame into `out`.
// vxlan_decap strips VXLAN header from a UDP payload.

/// UDP header size
const UDP_HDR: usize = 8;

/// VXLAN header size
const VXLAN_HDR: usize = 8;

/// Total outer overhead (IP + UDP + VXLAN)
const VXLAN_OVERHEAD: usize = IPV4_HDR + UDP_HDR + VXLAN_HDR;

/// Maximum VXLAN output buffer (slightly larger to hold full inner frame)
const MAX_VXLAN_PKT: usize = 1550;

/// Build and write a UDP header (8 bytes) at `out[offset..offset+8]`.
/// Does NOT include a checksum (zero checksum is valid in UDP for IPv4).
fn write_udp_header(
    out: &mut [u8; MAX_VXLAN_PKT],
    offset: usize,
    src_port: u16,
    dst_port: u16,
    udp_len: u16, // UDP header + payload
) {
    let sp = src_port.to_be_bytes();
    let dp = dst_port.to_be_bytes();
    let ln = udp_len.to_be_bytes();
    out[offset] = sp[0];
    out[offset + 1] = sp[1];
    out[offset + 2] = dp[0];
    out[offset + 3] = dp[1];
    out[offset + 4] = ln[0];
    out[offset + 5] = ln[1];
    out[offset + 6] = 0;
    out[offset + 7] = 0; // checksum = 0 (disabled)
}

/// Encapsulate `inner_frame` (Ethernet) in VXLAN/UDP/IPv4 into `out`.
///
/// `inner_len` must be <= `inner_frame.len()`.
/// Returns total byte count written, or 0 on overflow.
pub fn vxlan_encap(
    vni: u32,
    inner_frame: &[u8],
    inner_len: usize,
    out: &mut [u8; MAX_VXLAN_PKT],
) -> usize {
    let inner_len = inner_len.min(inner_frame.len());

    let total = VXLAN_OVERHEAD.saturating_add(inner_len);
    if total > MAX_VXLAN_PKT {
        return 0;
    }

    // Derive local IP from registered VXLAN config for this VNI.
    // If not found, use 0.0.0.0 — caller should check for a valid send.
    let local_ip = {
        let vx = VXLAN_TUNNELS.lock();
        let mut found = [0u8; 4];
        let mut i = 0usize;
        while i < MAX_VXLAN {
            if vx[i].active && vx[i].vni == (vni & 0x00FF_FFFF) {
                found = vx[i].local_ip;
                break;
            }
            i = i.saturating_add(1);
        }
        found
    };

    // Sizes
    let udp_len = (UDP_HDR + VXLAN_HDR + inner_len) as u16;
    let ip_payload = (UDP_HDR + VXLAN_HDR + inner_len) as u16;

    // Outer IPv4 header (proto=UDP=17).
    let ip_hdr = ipv4::build_header(
        Ipv4Addr(local_ip),
        Ipv4Addr([255, 255, 255, 255]), // multicast/broadcast placeholder
        17,                             // IPPROTO_UDP
        ip_payload,
        64,
    );
    let hdr_bytes = ipv4::header_to_bytes(&ip_hdr);
    out[..IPV4_HDR].copy_from_slice(&hdr_bytes);

    // UDP header.
    write_udp_header(out, IPV4_HDR, 0, VXLAN_PORT, udp_len);

    // VXLAN header: flags byte = 0x08 (I flag set), 3 reserved bytes, VNI (3 bytes), 1 reserved.
    let vx_off = IPV4_HDR + UDP_HDR;
    out[vx_off] = 0x08; // flags: I=1
    out[vx_off + 1] = 0; // reserved
    out[vx_off + 2] = 0; // reserved
    out[vx_off + 3] = 0; // reserved
                         // VNI is 24 bits, big-endian in bytes [4..6], byte[7] reserved.
    let vni_b = (vni & 0x00FF_FFFF).to_be_bytes(); // 4 bytes, use [1..4]
    out[vx_off + 4] = vni_b[1];
    out[vx_off + 5] = vni_b[2];
    out[vx_off + 6] = vni_b[3];
    out[vx_off + 7] = 0; // reserved

    // Inner Ethernet frame.
    let frame_off = vx_off + VXLAN_HDR;
    out[frame_off..frame_off + inner_len].copy_from_slice(&inner_frame[..inner_len]);

    total
}

/// Strip the VXLAN header from a UDP payload.
///
/// `udp_payload` is the UDP datagram payload (everything after the UDP header).
/// Returns `(VNI, inner_frame_offset)` or `None` on invalid header.
pub fn vxlan_decap(udp_payload: &[u8], len: usize) -> Option<(u32, usize)> {
    if len < VXLAN_HDR || udp_payload.len() < VXLAN_HDR {
        return None;
    }

    // Flags byte: I flag (bit 3) must be set.
    let flags = udp_payload[0];
    if flags & 0x08 == 0 {
        return None;
    }

    // VNI is at bytes [4..7], byte[7] is reserved.
    let vni: u32 =
        ((udp_payload[4] as u32) << 16) | ((udp_payload[5] as u32) << 8) | (udp_payload[6] as u32);

    // Inner frame starts immediately after the 8-byte VXLAN header.
    Some((vni, VXLAN_HDR))
}

/// Register a VXLAN tunnel for the given VNI.
/// Returns `true` on success, `false` if the table is full.
pub fn vxlan_register(vni: u32, local_ip: [u8; 4]) -> bool {
    let mut vx = VXLAN_TUNNELS.lock();
    // Check if already registered.
    let mut i = 0usize;
    while i < MAX_VXLAN {
        if vx[i].active && vx[i].vni == (vni & 0x00FF_FFFF) {
            // Update local IP.
            vx[i].local_ip = local_ip;
            return true;
        }
        i = i.saturating_add(1);
    }
    // Find empty slot.
    let mut j = 0usize;
    while j < MAX_VXLAN {
        if !vx[j].active {
            vx[j] = VxlanConfig {
                vni: vni & 0x00FF_FFFF,
                local_ip,
                dst_port: VXLAN_PORT,
                active: true,
            };
            return true;
        }
        j = j.saturating_add(1);
    }
    false
}

/// Deregister a VXLAN tunnel by VNI.
pub fn vxlan_deregister(vni: u32) {
    let mut vx = VXLAN_TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_VXLAN {
        if vx[i].active && vx[i].vni == (vni & 0x00FF_FFFF) {
            vx[i] = VxlanConfig::empty();
            return;
        }
        i = i.saturating_add(1);
    }
}

// ===========================================================================
// Tunnel management
// ===========================================================================

/// Add a tunnel to the global table.
/// Returns `Some(tunnel_id)` on success, `None` if the table is full.
pub fn tunnel_create(config: TunnelConfig) -> Option<u32> {
    let mut tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        if !tbl[i].active {
            tbl[i] = config;
            tbl[i].active = true;
            return Some(config.tunnel_id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Remove a tunnel from the global table.
/// Returns `true` if found and removed.
pub fn tunnel_destroy(tunnel_id: u32) -> bool {
    let mut tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        if tbl[i].active && tbl[i].tunnel_id == tunnel_id {
            tbl[i] = TunnelConfig::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Find a tunnel by matching local IP, remote IP, and protocol.
/// Returns the tunnel_id if found.
pub fn tunnel_find(local_ip: [u8; 4], remote_ip: [u8; 4], proto: TunnelProto) -> Option<u32> {
    let tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        let t = &tbl[i];
        if t.active
            && t.local_ip == local_ip
            && t.remote_ip == remote_ip
            && proto_eq(t.proto, proto)
        {
            return Some(t.tunnel_id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Retrieve a copy of a tunnel config by ID.
pub fn tunnel_find_by_id(tunnel_id: u32) -> Option<TunnelConfig> {
    let tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        if tbl[i].active && tbl[i].tunnel_id == tunnel_id {
            return Some(tbl[i]);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Encapsulate and transmit `inner_pkt` through the tunnel identified by `tunnel_id`.
///
/// Updates TX stats.  Returns `true` on success.
pub fn tunnel_send(tunnel_id: u32, inner_pkt: &[u8], inner_proto: u16) -> bool {
    // Snapshot the config so we can release the lock before sending.
    let mut cfg = match tunnel_find_by_id(tunnel_id) {
        Some(c) => c,
        None => return false,
    };

    let mut out = [0u8; MAX_PKT];
    let written = match cfg.proto {
        TunnelProto::Ipip => ipip_encap(&cfg, inner_pkt, &mut out),
        TunnelProto::Gre => gre_encap(&mut cfg, inner_pkt, inner_proto, &mut out),
        TunnelProto::Sit => {
            // SIT: IPv6-in-IPv4 — same layout as IPIP but with proto=41.
            // Re-use ipip_encap logic; the caller provides the IPv6 packet.
            let total = IPV4_HDR.saturating_add(inner_pkt.len());
            if total <= MAX_PKT {
                let hdr = ipv4::build_header(
                    Ipv4Addr(cfg.local_ip),
                    Ipv4Addr(cfg.remote_ip),
                    41, // IPPROTO_IPV6
                    inner_pkt.len() as u16,
                    cfg.ttl,
                );
                let hb = ipv4::header_to_bytes(&hdr);
                out[..IPV4_HDR].copy_from_slice(&hb);
                out[IPV4_HDR..total].copy_from_slice(inner_pkt);
                total
            } else {
                0
            }
        }
        TunnelProto::Vxlan => {
            // VXLAN send is handled separately via vxlan_encap.
            0
        }
    };

    if written == 0 {
        return false;
    }

    // Write back updated sequence counter if changed.
    if cfg.sequence {
        let mut tbl = TUNNELS.lock();
        let mut i = 0usize;
        while i < MAX_TUNNELS {
            if tbl[i].active && tbl[i].tunnel_id == tunnel_id {
                tbl[i].tx_seq = cfg.tx_seq;
                break;
            }
            i = i.saturating_add(1);
        }
    }

    // Resolve destination MAC and transmit.
    let dst_ip = Ipv4Addr(cfg.remote_ip);
    let dst_mac = arp::lookup(dst_ip).unwrap_or(super::MacAddr::BROADCAST);

    // Get our source MAC from the first configured interface.
    let src_mac = match super::primary_mac() {
        Some(m) => m,
        None => return false,
    };

    super::send_ip_frame_pub(src_mac, dst_mac, &out[..written]);

    // Update TX stats (saturating).
    let mut tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        if tbl[i].active && tbl[i].tunnel_id == tunnel_id {
            tbl[i].tx_packets = tbl[i].tx_packets.saturating_add(1);
            tbl[i].tx_bytes = tbl[i].tx_bytes.saturating_add(written as u64);
            break;
        }
        i = i.saturating_add(1);
    }

    true
}

/// Process an outer IP packet that may belong to a tunnel.
///
/// `outer_pkt`  — raw bytes starting at the outer IPv4 header.
/// `outer_len`  — byte count in `outer_pkt`.
/// `outer_proto`— the IP protocol number from the outer IP header (4 or 47).
///
/// Returns `true` if the packet was claimed by a tunnel.
pub fn tunnel_receive(outer_pkt: &[u8], outer_len: usize, outer_proto: u8) -> bool {
    // Parse outer IPv4 header.
    if outer_len < IPV4_HDR || outer_pkt.len() < IPV4_HDR {
        return false;
    }
    let (ip_hdr, _) = match ipv4::Ipv4Header::parse(&outer_pkt[..outer_len]) {
        Some(h) => h,
        None => return false,
    };
    let src_ip = ip_hdr.src_addr();
    let ihl = ip_hdr.ihl() as usize * 4;

    match outer_proto {
        // ----- IPIP -----
        IPPROTO_IPIP => {
            // Find tunnel matching src -> dst (reversed: remote is the peer src).
            let tun_id = {
                let tbl = TUNNELS.lock();
                let mut found = 0u32;
                let mut i = 0usize;
                while i < MAX_TUNNELS {
                    let t = &tbl[i];
                    if t.active && proto_eq(t.proto, TunnelProto::Ipip) && t.remote_ip == src_ip.0 {
                        found = t.tunnel_id;
                        break;
                    }
                    i = i.saturating_add(1);
                }
                found
            };
            if tun_id == 0 {
                return false;
            }

            // Decapsulate.
            let (inner_start, inner_len) = match ipip_decap(outer_pkt, outer_len) {
                Some(r) => r,
                None => return false,
            };

            // Update RX stats.
            update_rx_stats(tun_id, inner_len as u64);

            // Feed the inner IP packet back into the network stack.
            // `inner_start` is the absolute offset into `outer_pkt`.
            let inner_end = inner_start.saturating_add(inner_len);
            if inner_end <= outer_pkt.len() {
                super::process_frame_ip(&outer_pkt[inner_start..inner_end]);
            }
            true
        }

        // ----- GRE -----
        IPPROTO_GRE => {
            // GRE payload starts immediately after the outer IP header.
            if ihl >= outer_len {
                return false;
            }
            let gre_start = ihl;
            let gre_len = outer_len.saturating_sub(ihl);

            let tun_id = {
                let tbl = TUNNELS.lock();
                let mut found = 0u32;
                let mut i = 0usize;
                while i < MAX_TUNNELS {
                    let t = &tbl[i];
                    if t.active && proto_eq(t.proto, TunnelProto::Gre) && t.remote_ip == src_ip.0 {
                        found = t.tunnel_id;
                        break;
                    }
                    i = i.saturating_add(1);
                }
                found
            };
            if tun_id == 0 {
                return false;
            }

            let (inner_proto, inner_off, inner_len) =
                match gre_decap(&outer_pkt[gre_start..], gre_len) {
                    Some(r) => r,
                    None => return false,
                };

            update_rx_stats(tun_id, inner_len as u64);

            // Dispatch the inner payload based on EtherType.
            let abs_inner_start = gre_start.saturating_add(inner_off);
            let abs_inner_end = abs_inner_start.saturating_add(inner_len);
            if abs_inner_end <= outer_pkt.len() {
                let inner_data = &outer_pkt[abs_inner_start..abs_inner_end];
                match inner_proto {
                    GRE_PROTO_IPV4 => {
                        super::process_frame_ip(inner_data);
                    }
                    GRE_PROTO_ETHER => {
                        super::process_frame(inner_data);
                    }
                    _ => {}
                }
            }
            true
        }

        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Update RX stats for a tunnel identified by ID.
fn update_rx_stats(tunnel_id: u32, byte_count: u64) {
    let mut tbl = TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_TUNNELS {
        if tbl[i].active && tbl[i].tunnel_id == tunnel_id {
            tbl[i].rx_packets = tbl[i].rx_packets.saturating_add(1);
            tbl[i].rx_bytes = tbl[i].rx_bytes.saturating_add(byte_count);
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Compare two `TunnelProto` values (PartialEq is derived but non-Copy
/// fields could prevent use inside a lock — this helper avoids that).
#[inline(always)]
fn proto_eq(a: TunnelProto, b: TunnelProto) -> bool {
    match (a, b) {
        (TunnelProto::Ipip, TunnelProto::Ipip) => true,
        (TunnelProto::Gre, TunnelProto::Gre) => true,
        (TunnelProto::Sit, TunnelProto::Sit) => true,
        (TunnelProto::Vxlan, TunnelProto::Vxlan) => true,
        _ => false,
    }
}
