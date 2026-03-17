use super::ipv4;
use super::Ipv4Addr;
use crate::serial_println;
/// GRE tunneling (RFC 2784) for Genesis
///
/// Generic Routing Encapsulation wraps any inner protocol in IPv4 with
/// IP protocol number 47 (IPPROTO_GRE).
///
/// Header layout (variable, minimum 4 bytes):
///
/// ```text
///  0               1               2               3
///  0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |C| |K|S| Reserved0       | Ver |         Protocol Type         |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |      Checksum (optional)      |       Reserved1 (optional)    |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                         Key (optional)                        |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                    Sequence Number (optional)                  |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
///
/// Design constraints (bare-metal kernel rules):
///   - No heap: all storage is fixed-size static arrays
///   - No floats: no `as f64` / `as f32` anywhere
///   - No panics: all error paths return Option/bool/(usize, u16)
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile (N/A — pure protocol logic)
///
/// Inspired by: RFC 2784, RFC 2890, Linux ip_gre.c. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// IP protocol number for GRE (RFC 2784)
pub const IPPROTO_GRE: u8 = 47;

/// Minimum GRE header size: flags/version (2 bytes) + protocol type (2 bytes)
pub const GRE_HDR_MIN: usize = 4;

/// GRE flag: Checksum Present (bit 15)
pub const GRE_FLAG_CHECKSUM: u16 = 0x8000;

/// GRE flag: Key Present (bit 13)
pub const GRE_FLAG_KEY: u16 = 0x2000;

/// GRE flag: Sequence Number Present (bit 12)
pub const GRE_FLAG_SEQ: u16 = 0x1000;

/// Inner protocol: encapsulated IPv4
pub const GRE_PROTO_IP: u16 = 0x0800;

/// Inner protocol: transparent Ethernet bridging (GRETAP)
pub const GRE_PROTO_ETH: u16 = 0x6558;

/// Maximum number of GRE tunnels
pub const MAX_GRE_TUNNELS: usize = 8;

// ---------------------------------------------------------------------------
// Internal constants
// ---------------------------------------------------------------------------

/// IPv4 header size (no options)
const IPV4_HDR: usize = 20;

/// Maximum GRE header size:
/// 4 (base) + 4 (checksum+reserved) + 4 (key) + 4 (seq) = 16 bytes
const GRE_HDR_MAX: usize = 16;

// ---------------------------------------------------------------------------
// GreTunnel
// ---------------------------------------------------------------------------

/// Configuration and runtime state of a single GRE tunnel.
#[derive(Copy, Clone)]
pub struct GreTunnel {
    /// Outer source IPv4 address
    pub local_ip: [u8; 4],
    /// Outer destination IPv4 address
    pub remote_ip: [u8; 4],
    /// Inner protocol EtherType (GRE_PROTO_IP or GRE_PROTO_ETH)
    pub proto: u16,
    /// Optional tunnel key value (ignored when use_key = false)
    pub key: u32,
    /// Include the Key field in the GRE header
    pub use_key: bool,
    /// Include the Sequence Number field in the GRE header
    pub use_seq: bool,
    /// TX sequence counter (wrapping, per RFC 2890)
    pub tx_seq: u32,
    pub rx_pkts: u64,
    pub tx_pkts: u64,
    pub active: bool,
}

impl GreTunnel {
    pub const fn empty() -> Self {
        GreTunnel {
            local_ip: [0; 4],
            remote_ip: [0; 4],
            proto: GRE_PROTO_IP,
            key: 0,
            use_key: false,
            use_seq: false,
            tx_seq: 0,
            rx_pkts: 0,
            tx_pkts: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GRE_TUNNELS: Mutex<[GreTunnel; MAX_GRE_TUNNELS]> =
    Mutex::new([GreTunnel::empty(); MAX_GRE_TUNNELS]);

// ---------------------------------------------------------------------------
// Tunnel lifecycle
// ---------------------------------------------------------------------------

/// Create a new GRE tunnel.
///
/// `key` is the tunnel key value; pass 0 and the key field will not be
/// included in the header (`use_key` is set to `true` only when `key != 0`).
///
/// Returns `Some(index)` on success, `None` if the table is full.
pub fn gre_tunnel_create(local: [u8; 4], remote: [u8; 4], proto: u16, key: u32) -> Option<u32> {
    let use_key = key != 0;
    let mut tbl = GRE_TUNNELS.lock();
    let mut i = 0usize;
    while i < MAX_GRE_TUNNELS {
        if !tbl[i].active {
            tbl[i] = GreTunnel {
                local_ip: local,
                remote_ip: remote,
                proto,
                key,
                use_key,
                use_seq: false,
                tx_seq: 0,
                rx_pkts: 0,
                tx_pkts: 0,
                active: true,
            };
            return Some(i as u32);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Destroy the tunnel at `idx`.
///
/// Returns `true` if the slot was active and has been cleared.
pub fn gre_tunnel_destroy(idx: u32) -> bool {
    if idx as usize >= MAX_GRE_TUNNELS {
        return false;
    }
    let mut tbl = GRE_TUNNELS.lock();
    if !tbl[idx as usize].active {
        return false;
    }
    tbl[idx as usize] = GreTunnel::empty();
    true
}

// ---------------------------------------------------------------------------
// Encapsulation
// ---------------------------------------------------------------------------

/// Encapsulate `inner` in GRE and write the full packet (IPv4 + GRE + inner)
/// into `out`.
///
/// Returns the total number of bytes written, or 0 on error (unknown tunnel,
/// packet too large, buffer overflow).
pub fn gre_encap(tunnel_idx: u32, inner: &[u8], out: &mut [u8; 1600]) -> usize {
    if tunnel_idx as usize >= MAX_GRE_TUNNELS {
        return 0;
    }

    // Snapshot under lock, then release before building the packet.
    let (local_ip, remote_ip, proto, use_key, key, use_seq, tx_seq) = {
        let tbl = GRE_TUNNELS.lock();
        let t = &tbl[tunnel_idx as usize];
        if !t.active {
            return 0;
        }
        (
            t.local_ip,
            t.remote_ip,
            t.proto,
            t.use_key,
            t.key,
            t.use_seq,
            t.tx_seq,
        )
    };

    // --- Build GRE header into a stack buffer ---
    let mut gre_buf = [0u8; GRE_HDR_MAX];
    let mut flags: u16 = 0;

    if use_key {
        flags |= GRE_FLAG_KEY;
    }
    if use_seq {
        flags |= GRE_FLAG_SEQ;
    }

    // Bytes 0-1: flags + version (version = 0)
    let fw = flags.to_be_bytes();
    gre_buf[0] = fw[0];
    gre_buf[1] = fw[1];

    // Bytes 2-3: inner protocol type (EtherType)
    let pw = proto.to_be_bytes();
    gre_buf[2] = pw[0];
    gre_buf[3] = pw[1];

    let mut gre_len = GRE_HDR_MIN; // 4 bytes so far

    // Optional Key field (4 bytes)
    if use_key {
        let kb = key.to_be_bytes();
        gre_buf[gre_len] = kb[0];
        gre_buf[gre_len + 1] = kb[1];
        gre_buf[gre_len + 2] = kb[2];
        gre_buf[gre_len + 3] = kb[3];
        gre_len = gre_len.saturating_add(4);
    }

    // Optional Sequence Number field (4 bytes)
    if use_seq {
        let sb = tx_seq.to_be_bytes();
        gre_buf[gre_len] = sb[0];
        gre_buf[gre_len + 1] = sb[1];
        gre_buf[gre_len + 2] = sb[2];
        gre_buf[gre_len + 3] = sb[3];
        gre_len = gre_len.saturating_add(4);
    }

    // --- Size check ---
    let total = IPV4_HDR.saturating_add(gre_len).saturating_add(inner.len());
    if total > 1600 {
        return 0;
    }

    // --- Outer IPv4 header ---
    let ip_payload_len = (gre_len.saturating_add(inner.len())) as u16;
    let ip_hdr = ipv4::build_header(
        Ipv4Addr(local_ip),
        Ipv4Addr(remote_ip),
        IPPROTO_GRE,
        ip_payload_len,
        64,
    );
    let hdr_bytes = ipv4::header_to_bytes(&ip_hdr);

    // Write IPv4 header.
    out[..IPV4_HDR].copy_from_slice(&hdr_bytes);

    // Write GRE header.
    let gre_start = IPV4_HDR;
    let gre_end = gre_start.saturating_add(gre_len);
    out[gre_start..gre_end].copy_from_slice(&gre_buf[..gre_len]);

    // Write inner payload.
    let inner_start = gre_end;
    let inner_end = inner_start.saturating_add(inner.len());
    out[inner_start..inner_end].copy_from_slice(inner);

    // --- Advance TX sequence number and update stats under lock ---
    {
        let mut tbl = GRE_TUNNELS.lock();
        if tbl[tunnel_idx as usize].active {
            if use_seq {
                tbl[tunnel_idx as usize].tx_seq = tbl[tunnel_idx as usize].tx_seq.wrapping_add(1);
            }
            tbl[tunnel_idx as usize].tx_pkts = tbl[tunnel_idx as usize].tx_pkts.saturating_add(1);
        }
    }

    total
}

// ---------------------------------------------------------------------------
// Decapsulation
// ---------------------------------------------------------------------------

/// Strip the outer IPv4 + GRE headers from `packet` and write the inner
/// payload into `inner_buf`.
///
/// `packet` begins at the outer IPv4 header.
/// Returns `(inner_len, inner_proto)`.  Both fields are 0 on any error
/// (truncated packet, unknown flags, inner frame too large).
pub fn gre_decap(packet: &[u8], len: usize, inner_buf: &mut [u8; 1514]) -> (usize, u16) {
    // Need at least IPv4 header + minimum GRE header.
    let min_len = IPV4_HDR.saturating_add(GRE_HDR_MIN);
    if len < min_len || packet.len() < min_len {
        return (0, 0);
    }

    // Skip the outer IPv4 header (fixed 20 bytes, IHL=5 assumed).
    let gre_off = IPV4_HDR;
    let gre_data = &packet[gre_off..];
    let gre_avail = len.saturating_sub(gre_off);

    if gre_avail < GRE_HDR_MIN {
        return (0, 0);
    }

    // Parse GRE flags and protocol.
    let flags = u16::from_be_bytes([gre_data[0], gre_data[1]]);
    let inner_proto = u16::from_be_bytes([gre_data[2], gre_data[3]]);
    let mut gre_hdr_len = GRE_HDR_MIN;

    // Checksum Present (bit 15): skip 4 bytes (checksum u16 + reserved u16).
    if flags & GRE_FLAG_CHECKSUM != 0 {
        gre_hdr_len = gre_hdr_len.saturating_add(4);
        if gre_avail < gre_hdr_len {
            return (0, 0);
        }
    }

    // Key Present (bit 13): skip 4 bytes.
    if flags & GRE_FLAG_KEY != 0 {
        gre_hdr_len = gre_hdr_len.saturating_add(4);
        if gre_avail < gre_hdr_len {
            return (0, 0);
        }
    }

    // Sequence Number Present (bit 12): skip 4 bytes.
    if flags & GRE_FLAG_SEQ != 0 {
        gre_hdr_len = gre_hdr_len.saturating_add(4);
        if gre_avail < gre_hdr_len {
            return (0, 0);
        }
    }

    // Inner payload follows GRE header.
    let inner_off = gre_off.saturating_add(gre_hdr_len);
    if inner_off >= len {
        return (0, 0);
    }
    let inner_len = len.saturating_sub(inner_off);
    if inner_len == 0 || inner_len > 1514 {
        return (0, 0);
    }

    inner_buf[..inner_len].copy_from_slice(&packet[inner_off..inner_off + inner_len]);

    (inner_len, inner_proto)
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return `(rx_pkts, tx_pkts)` for the tunnel at `idx`.
///
/// Returns `None` if the slot is inactive or out of range.
pub fn gre_get_stats(idx: u32) -> Option<(u64, u64)> {
    if idx as usize >= MAX_GRE_TUNNELS {
        return None;
    }
    let tbl = GRE_TUNNELS.lock();
    let t = &tbl[idx as usize];
    if !t.active {
        return None;
    }
    Some((t.rx_pkts, t.tx_pkts))
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

/// Initialize the GRE subsystem.
///
/// State is const-initialized; this call exists to emit the boot banner.
pub fn init() {
    serial_println!("[gre] GRE tunneling initialized");
}
