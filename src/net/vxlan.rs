use super::ipv4;
use super::Ipv4Addr;
use crate::serial_println;
/// VXLAN (RFC 7348) overlay network for Genesis
///
/// VXLAN wraps Ethernet frames in UDP (port 4789) with an 8-byte VXLAN header.
///
/// Frame layout (encapsulated):
///   [Outer IPv4 hdr]   20 bytes  (proto=17, UDP)
///   [Outer UDP hdr]     8 bytes  (dst_port=4789)
///   [VXLAN hdr]         8 bytes  (I-flag | VNI)
///   [Inner Ethernet frame]
///
/// Design constraints (bare-metal kernel rules):
///   - No heap: all storage is fixed-size static arrays
///   - No floats: no `as f64` / `as f32` anywhere
///   - No panics: all error paths return Option/bool
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile (N/A — pure protocol logic)
///
/// Inspired by: RFC 7348, Linux vxlan.c. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// IANA-assigned UDP destination port for VXLAN
pub const VXLAN_UDP_PORT: u16 = 4789;

/// VXLAN header size in bytes (flags word + VNI word)
pub const VXLAN_HDR_SIZE: usize = 8;

/// "I" (VNI valid) flag — bit 27 of the 32-bit flags field (big-endian byte 0, bit 3)
pub const VXLAN_HDR_I_FLAG: u32 = 0x0800_0000;

/// Maximum number of VXLAN tunnels (local VTEP registrations)
pub const MAX_VXLAN_TUNNELS: usize = 8;

/// Maximum number of forwarding database entries (MAC -> VTEP IP)
pub const MAX_VXLAN_FDB_ENTRIES: usize = 128;

/// VXLAN Network Identifier type (only lower 24 bits are used)
pub type Vni = u32;

// ---------------------------------------------------------------------------
// Internal layout constants
// ---------------------------------------------------------------------------

/// IPv4 header size (no options)
const IPV4_HDR: usize = 20;

/// UDP header size
const UDP_HDR: usize = 8;

/// Total outer overhead per encapsulated frame
const VXLAN_OVERHEAD: usize = IPV4_HDR + UDP_HDR + VXLAN_HDR_SIZE;

/// IP protocol number: UDP
const IPPROTO_UDP: u8 = 17;

// ---------------------------------------------------------------------------
// VxlanTunnel — local VTEP (Virtual Tunnel Endpoint) registration
// ---------------------------------------------------------------------------

/// A VXLAN tunnel represents a local VTEP bound to a VNI.
#[derive(Copy, Clone)]
pub struct VxlanTunnel {
    pub vni: Vni,
    pub local_ip: [u8; 4],
    pub local_port: u16,
    /// Virtual interface index (opaque; caller-assigned)
    pub ifindex: u8,
    pub rx_pkts: u64,
    pub tx_pkts: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub active: bool,
}

impl VxlanTunnel {
    pub const fn empty() -> Self {
        VxlanTunnel {
            vni: 0,
            local_ip: [0; 4],
            local_port: VXLAN_UDP_PORT,
            ifindex: 0,
            rx_pkts: 0,
            tx_pkts: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// VxlanFdbEntry — forwarding database: MAC address -> remote VTEP IP
// ---------------------------------------------------------------------------

/// Maps a (VNI, MAC) pair to the remote VTEP's IP address and UDP port.
#[derive(Copy, Clone)]
pub struct VxlanFdbEntry {
    pub vni: Vni,
    pub mac: [u8; 6],
    pub remote_vtep: [u8; 4],
    /// UDP port on the remote VTEP (default: 4789)
    pub port: u16,
    pub active: bool,
}

impl VxlanFdbEntry {
    pub const fn empty() -> Self {
        VxlanFdbEntry {
            vni: 0,
            mac: [0; 6],
            remote_vtep: [0; 4],
            port: VXLAN_UDP_PORT,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static VXLAN_TUNNELS: Mutex<[VxlanTunnel; MAX_VXLAN_TUNNELS]> =
    Mutex::new([VxlanTunnel::empty(); MAX_VXLAN_TUNNELS]);

static VXLAN_FDB: Mutex<[VxlanFdbEntry; MAX_VXLAN_FDB_ENTRIES]> =
    Mutex::new([VxlanFdbEntry::empty(); MAX_VXLAN_FDB_ENTRIES]);

// ---------------------------------------------------------------------------
// Tunnel lifecycle
// ---------------------------------------------------------------------------

/// Register a new VXLAN tunnel for the given VNI and local IP.
///
/// Returns `Some(index)` into the tunnel table on success, or `None` if the
/// table is full or the VNI is already registered.
pub fn vxlan_create(vni: Vni, local_ip: [u8; 4]) -> Option<u32> {
    let vni = vni & 0x00FF_FFFF; // mask to 24 bits
    let mut tbl = VXLAN_TUNNELS.lock();
    let mut i = 0usize;
    // Reject duplicate VNI registrations.
    while i < MAX_VXLAN_TUNNELS {
        if tbl[i].active && tbl[i].vni == vni {
            return None;
        }
        i = i.saturating_add(1);
    }
    // Find an empty slot.
    let mut j = 0usize;
    while j < MAX_VXLAN_TUNNELS {
        if !tbl[j].active {
            tbl[j] = VxlanTunnel {
                vni,
                local_ip,
                local_port: VXLAN_UDP_PORT,
                ifindex: j as u8,
                rx_pkts: 0,
                tx_pkts: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                active: true,
            };
            return Some(j as u32);
        }
        j = j.saturating_add(1);
    }
    None
}

/// Destroy the tunnel at `idx`, clearing its slot.
///
/// Returns `true` if the slot was active, `false` otherwise.
pub fn vxlan_destroy(idx: u32) -> bool {
    if idx as usize >= MAX_VXLAN_TUNNELS {
        return false;
    }
    let mut tbl = VXLAN_TUNNELS.lock();
    if !tbl[idx as usize].active {
        return false;
    }
    tbl[idx as usize] = VxlanTunnel::empty();
    true
}

// ---------------------------------------------------------------------------
// Encapsulation
// ---------------------------------------------------------------------------

/// Encapsulate `inner_frame` (an Ethernet frame) in VXLAN/UDP/IPv4.
///
/// Builds the full outer packet (IPv4 + UDP + VXLAN + inner) into `out_buf`.
/// Returns the total number of bytes written, or 0 on any error (unknown
/// tunnel index, frame too large to fit).
///
/// The VXLAN header layout (RFC 7348):
///
/// ```text
///  0               1               2               3
///  0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |R|R|R|R|I|R|R|R|            Reserved                           |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// |                VXLAN Network Identifier (VNI) |   Reserved    |
/// +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// ```
///
/// The 32-bit flags word is stored big-endian.  The I-bit is bit 27 of the
/// little-endian u32, which maps to bit 3 of byte 0 in the wire format.
/// `VXLAN_HDR_I_FLAG = 0x0800_0000` → byte 0 on wire = 0x08.
pub fn vxlan_encap(tunnel_idx: u32, inner_frame: &[u8], out_buf: &mut [u8; 1600]) -> usize {
    if tunnel_idx as usize >= MAX_VXLAN_TUNNELS {
        return 0;
    }

    // Snapshot the tunnel state under lock.
    let (vni, local_ip) = {
        let tbl = VXLAN_TUNNELS.lock();
        let t = &tbl[tunnel_idx as usize];
        if !t.active {
            return 0;
        }
        (t.vni, t.local_ip)
    };

    let inner_len = inner_frame.len();
    let total = VXLAN_OVERHEAD.saturating_add(inner_len);
    if total > 1600 {
        return 0;
    }

    // --- Build VXLAN header (8 bytes) ---
    let mut vxlan_hdr = [0u8; 8];
    let flags: u32 = VXLAN_HDR_I_FLAG;
    vxlan_hdr[0..4].copy_from_slice(&flags.to_be_bytes());
    // VNI occupies the upper 3 bytes of the second 32-bit word.
    vxlan_hdr[4] = ((vni >> 16) & 0xFF) as u8;
    vxlan_hdr[5] = ((vni >> 8) & 0xFF) as u8;
    vxlan_hdr[6] = (vni & 0xFF) as u8;
    vxlan_hdr[7] = 0; // reserved

    // --- Outer UDP header (8 bytes) ---
    // udp_len = UDP header + VXLAN header + inner frame
    let udp_payload_len = VXLAN_HDR_SIZE.saturating_add(inner_len);
    let udp_total_len = UDP_HDR.saturating_add(udp_payload_len);

    // --- Outer IPv4 header (20 bytes) ---
    let ip_payload_len = udp_total_len as u16;
    let ip_hdr = ipv4::build_header(
        Ipv4Addr(local_ip),
        Ipv4Addr([255, 255, 255, 255]), // broadcast/multicast; caller sets actual dest
        IPPROTO_UDP,
        ip_payload_len,
        64,
    );
    let hdr_bytes = ipv4::header_to_bytes(&ip_hdr);

    // Write outer IPv4 header.
    out_buf[..IPV4_HDR].copy_from_slice(&hdr_bytes);

    // Write UDP header at IPV4_HDR offset.
    let udp_off = IPV4_HDR;
    let udp_len_u16 = udp_total_len as u16;
    let sp = (0u16).to_be_bytes(); // source port: ephemeral / 0
    let dp = VXLAN_UDP_PORT.to_be_bytes();
    let ln = udp_len_u16.to_be_bytes();
    out_buf[udp_off] = sp[0];
    out_buf[udp_off + 1] = sp[1];
    out_buf[udp_off + 2] = dp[0];
    out_buf[udp_off + 3] = dp[1];
    out_buf[udp_off + 4] = ln[0];
    out_buf[udp_off + 5] = ln[1];
    out_buf[udp_off + 6] = 0;
    out_buf[udp_off + 7] = 0; // checksum = 0

    // Write VXLAN header.
    let vx_off = udp_off + UDP_HDR;
    out_buf[vx_off..vx_off + VXLAN_HDR_SIZE].copy_from_slice(&vxlan_hdr);

    // Write inner Ethernet frame.
    let inner_off = vx_off + VXLAN_HDR_SIZE;
    out_buf[inner_off..inner_off + inner_len].copy_from_slice(inner_frame);

    // Update TX stats.
    {
        let mut tbl = VXLAN_TUNNELS.lock();
        if tbl[tunnel_idx as usize].active {
            tbl[tunnel_idx as usize].tx_pkts = tbl[tunnel_idx as usize].tx_pkts.saturating_add(1);
            tbl[tunnel_idx as usize].tx_bytes = tbl[tunnel_idx as usize]
                .tx_bytes
                .saturating_add(total as u64);
        }
    }

    total
}

// ---------------------------------------------------------------------------
// Decapsulation
// ---------------------------------------------------------------------------

/// Strip the outer VXLAN/UDP headers from `packet` and write the inner
/// Ethernet frame into `inner_buf`.
///
/// `packet` is expected to begin at the outer IPv4 header.
/// Returns the length of the inner frame on success, or 0 on any error
/// (truncated packet, missing I-flag, inner frame too large).
pub fn vxlan_decap(packet: &[u8], len: usize, inner_buf: &mut [u8; 1514]) -> usize {
    // Minimum: IPv4 hdr + UDP hdr + VXLAN hdr
    if len < VXLAN_OVERHEAD || packet.len() < VXLAN_OVERHEAD {
        return 0;
    }

    // Skip the outer IPv4 header.
    // We trust the caller to verify the outer IPv4 fields; here we just
    // advance past a fixed 20-byte header (IHL=5, no options assumed).
    let udp_off = IPV4_HDR;

    // Skip the UDP header.
    let vx_off = udp_off + UDP_HDR;

    // Parse VXLAN header: validate I-flag.
    let flags_word = u32::from_be_bytes([
        packet[vx_off],
        packet[vx_off + 1],
        packet[vx_off + 2],
        packet[vx_off + 3],
    ]);
    if flags_word & VXLAN_HDR_I_FLAG == 0 {
        // I-flag not set — VNI is not valid; drop.
        return 0;
    }

    // Extract VNI from bytes [4..6] of the VXLAN header.
    let vni: u32 = ((packet[vx_off + 4] as u32) << 16)
        | ((packet[vx_off + 5] as u32) << 8)
        | (packet[vx_off + 6] as u32);

    // Inner frame starts right after the 8-byte VXLAN header.
    let inner_off = vx_off + VXLAN_HDR_SIZE;
    let inner_len = len.saturating_sub(inner_off);
    if inner_len == 0 || inner_len > 1514 {
        return 0;
    }

    inner_buf[..inner_len].copy_from_slice(&packet[inner_off..inner_off + inner_len]);

    // Update RX stats for the tunnel matching this VNI.
    {
        let mut tbl = VXLAN_TUNNELS.lock();
        let mut i = 0usize;
        while i < MAX_VXLAN_TUNNELS {
            if tbl[i].active && tbl[i].vni == (vni & 0x00FF_FFFF) {
                tbl[i].rx_pkts = tbl[i].rx_pkts.saturating_add(1);
                tbl[i].rx_bytes = tbl[i].rx_bytes.saturating_add(inner_len as u64);
                break;
            }
            i = i.saturating_add(1);
        }
    }

    inner_len
}

// ---------------------------------------------------------------------------
// Forwarding Database (FDB)
// ---------------------------------------------------------------------------

/// Learn or update the MAC-to-VTEP mapping for `(vni, mac)`.
///
/// If an entry for this (VNI, MAC) pair already exists it is updated in
/// place.  Otherwise a new slot is allocated.
/// Returns `true` on success, `false` if the FDB is full.
pub fn vxlan_fdb_learn(vni: Vni, mac: [u8; 6], vtep_ip: [u8; 4]) -> bool {
    let vni = vni & 0x00FF_FFFF;
    let mut fdb = VXLAN_FDB.lock();

    // Update existing entry if found.
    let mut i = 0usize;
    while i < MAX_VXLAN_FDB_ENTRIES {
        if fdb[i].active && fdb[i].vni == vni && fdb[i].mac == mac {
            fdb[i].remote_vtep = vtep_ip;
            return true;
        }
        i = i.saturating_add(1);
    }

    // Allocate a new entry.
    let mut j = 0usize;
    while j < MAX_VXLAN_FDB_ENTRIES {
        if !fdb[j].active {
            fdb[j] = VxlanFdbEntry {
                vni,
                mac,
                remote_vtep: vtep_ip,
                port: VXLAN_UDP_PORT,
                active: true,
            };
            return true;
        }
        j = j.saturating_add(1);
    }

    false
}

/// Look up the remote VTEP IP for a `(vni, mac)` pair.
///
/// Returns `Some(vtep_ip)` if found, `None` otherwise.
pub fn vxlan_fdb_lookup(vni: Vni, mac: [u8; 6]) -> Option<[u8; 4]> {
    let vni = vni & 0x00FF_FFFF;
    let fdb = VXLAN_FDB.lock();
    let mut i = 0usize;
    while i < MAX_VXLAN_FDB_ENTRIES {
        if fdb[i].active && fdb[i].vni == vni && fdb[i].mac == mac {
            return Some(fdb[i].remote_vtep);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Remove the FDB entry for `(vni, mac)`.
///
/// Returns `true` if an entry was found and removed.
pub fn vxlan_fdb_delete(vni: Vni, mac: [u8; 6]) -> bool {
    let vni = vni & 0x00FF_FFFF;
    let mut fdb = VXLAN_FDB.lock();
    let mut i = 0usize;
    while i < MAX_VXLAN_FDB_ENTRIES {
        if fdb[i].active && fdb[i].vni == vni && fdb[i].mac == mac {
            fdb[i] = VxlanFdbEntry::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

/// Initialize the VXLAN subsystem.
///
/// State is const-initialized; this call exists to emit the boot banner.
pub fn init() {
    serial_println!("[vxlan] VXLAN overlay network initialized");
}
