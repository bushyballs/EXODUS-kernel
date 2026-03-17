use super::MacAddr;
use crate::sync::Mutex;
/// Ethernet frame handling for Genesis
///
/// Parses and constructs Ethernet II frames (the standard for modern networks).
/// Frame format: [dst MAC 6B][src MAC 6B][EtherType 2B][payload 46-1500B][FCS 4B]
///
/// EtherType identifies the payload protocol:
///   0x0800 = IPv4
///   0x0806 = ARP
///   0x86DD = IPv6
///
/// Features:
///   - Ethernet frame building (dest MAC, src MAC, ethertype, payload)
///   - VLAN 802.1Q tag support (tag/untag)
///   - Frame validation (min/max size)
///   - CRC-32 check concept
///   - Promiscuous mode flag
///   - Frame statistics counters (rx/tx packets, bytes, errors, drops)
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// EtherType values
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHERTYPE_IPV6: u16 = 0x86DD;
pub const ETHERTYPE_VLAN: u16 = 0x8100; // 802.1Q VLAN tag
pub const ETHERTYPE_LLDP: u16 = 0x88CC; // Link Layer Discovery Protocol

/// Minimum Ethernet frame size (excluding FCS)
pub const MIN_FRAME_SIZE: usize = 60;

/// Maximum Ethernet frame size (excluding FCS)
pub const MAX_FRAME_SIZE: usize = 1514;

/// Maximum frame size with VLAN tag (excluding FCS)
pub const MAX_VLAN_FRAME_SIZE: usize = 1518;

/// Header size (without VLAN tag)
pub const HEADER_SIZE: usize = 14;

/// Header size with VLAN tag
pub const VLAN_HEADER_SIZE: usize = 18;

/// FCS (Frame Check Sequence) size
pub const FCS_SIZE: usize = 4;

/// Maximum payload size (MTU for standard Ethernet)
pub const MAX_PAYLOAD: usize = 1500;

/// Minimum payload size (padded to 46 bytes)
pub const MIN_PAYLOAD: usize = 46;

// ---------------------------------------------------------------------------
// Ethernet header
// ---------------------------------------------------------------------------

/// Ethernet header (14 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct EthernetHeader {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: [u8; 2], // big-endian
}

impl EthernetHeader {
    /// Parse an Ethernet header from raw bytes
    pub fn parse(data: &[u8]) -> Option<(&EthernetHeader, &[u8])> {
        if data.len() < HEADER_SIZE {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const EthernetHeader) };

        // Check for VLAN tag — if present, the real ethertype is 4 bytes later
        if header.ethertype_u16() == ETHERTYPE_VLAN {
            if data.len() < VLAN_HEADER_SIZE {
                return None;
            }
            // We still return the header as-is; the caller should check for VLAN
            let payload = &data[VLAN_HEADER_SIZE..];
            Some((header, payload))
        } else {
            let payload = &data[HEADER_SIZE..];
            Some((header, payload))
        }
    }

    /// Get the EtherType as a u16
    pub fn ethertype_u16(&self) -> u16 {
        u16::from_be_bytes(self.ethertype)
    }

    /// Get destination MAC
    pub fn dst_mac(&self) -> MacAddr {
        MacAddr(self.dst)
    }

    /// Get source MAC
    pub fn src_mac(&self) -> MacAddr {
        MacAddr(self.src)
    }

    /// Check if the destination is broadcast
    pub fn is_broadcast(&self) -> bool {
        self.dst == [0xFF; 6]
    }

    /// Check if the destination is multicast (bit 0 of first byte is set)
    pub fn is_multicast(&self) -> bool {
        self.dst[0] & 0x01 != 0
    }
}

// ---------------------------------------------------------------------------
// VLAN tag
// ---------------------------------------------------------------------------

/// 802.1Q VLAN tag (4 bytes inserted after src MAC)
#[derive(Debug, Clone, Copy)]
pub struct VlanTag {
    /// Tag Protocol Identifier (TPID = 0x8100)
    pub tpid: u16,
    /// Priority Code Point (3 bits, 0-7)
    pub pcp: u8,
    /// Drop Eligible Indicator (1 bit)
    pub dei: bool,
    /// VLAN Identifier (12 bits, 0-4095)
    pub vid: u16,
}

impl VlanTag {
    /// Create a new VLAN tag with the given VID
    pub fn new(vid: u16) -> Self {
        VlanTag {
            tpid: ETHERTYPE_VLAN,
            pcp: 0,
            dei: false,
            vid: vid & 0x0FFF,
        }
    }

    /// Create a VLAN tag with priority
    pub fn with_priority(vid: u16, pcp: u8) -> Self {
        VlanTag {
            tpid: ETHERTYPE_VLAN,
            pcp: pcp & 0x07,
            dei: false,
            vid: vid & 0x0FFF,
        }
    }

    /// Parse a VLAN tag from bytes at position (after src MAC, before real ethertype)
    pub fn parse(data: &[u8]) -> Option<VlanTag> {
        if data.len() < 4 {
            return None;
        }
        let tpid = u16::from_be_bytes([data[0], data[1]]);
        if tpid != ETHERTYPE_VLAN {
            return None;
        }
        let tci = u16::from_be_bytes([data[2], data[3]]);
        Some(VlanTag {
            tpid,
            pcp: ((tci >> 13) & 0x07) as u8,
            dei: (tci >> 12) & 0x01 != 0,
            vid: tci & 0x0FFF,
        })
    }

    /// Serialize the VLAN tag to 4 bytes
    pub fn to_bytes(&self) -> [u8; 4] {
        let tci: u16 =
            ((self.pcp as u16 & 0x07) << 13) | ((self.dei as u16) << 12) | (self.vid & 0x0FFF);
        let mut buf = [0u8; 4];
        buf[0..2].copy_from_slice(&self.tpid.to_be_bytes());
        buf[2..4].copy_from_slice(&tci.to_be_bytes());
        buf
    }
}

// ---------------------------------------------------------------------------
// Frame building
// ---------------------------------------------------------------------------

/// Build an Ethernet frame (without VLAN tag).
/// Returns the number of bytes written to `buf`.
pub fn build_frame(
    dst: MacAddr,
    src: MacAddr,
    ethertype: u16,
    payload: &[u8],
    buf: &mut [u8],
) -> usize {
    let total = HEADER_SIZE + payload.len();
    assert!(buf.len() >= total);

    // Destination MAC
    buf[0..6].copy_from_slice(&dst.0);
    // Source MAC
    buf[6..12].copy_from_slice(&src.0);
    // EtherType
    buf[12..14].copy_from_slice(&ethertype.to_be_bytes());
    // Payload
    buf[14..14 + payload.len()].copy_from_slice(payload);

    total
}

/// Build an Ethernet frame with a VLAN tag.
/// Returns the number of bytes written to `buf`.
pub fn build_vlan_frame(
    dst: MacAddr,
    src: MacAddr,
    vlan: &VlanTag,
    ethertype: u16,
    payload: &[u8],
    buf: &mut [u8],
) -> usize {
    let total = VLAN_HEADER_SIZE + payload.len();
    assert!(buf.len() >= total);

    // Destination MAC
    buf[0..6].copy_from_slice(&dst.0);
    // Source MAC
    buf[6..12].copy_from_slice(&src.0);
    // VLAN tag (4 bytes)
    let vlan_bytes = vlan.to_bytes();
    buf[12..16].copy_from_slice(&vlan_bytes);
    // Real EtherType
    buf[16..18].copy_from_slice(&ethertype.to_be_bytes());
    // Payload
    buf[18..18 + payload.len()].copy_from_slice(payload);

    total
}

/// Build a frame as a Vec (allocating). Pads to minimum frame size.
pub fn build_frame_vec(dst: MacAddr, src: MacAddr, ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let raw_len = HEADER_SIZE + payload.len();
    let total = if raw_len < MIN_FRAME_SIZE {
        MIN_FRAME_SIZE
    } else {
        raw_len
    };
    let mut frame = alloc::vec![0u8; total];
    build_frame(dst, src, ethertype, payload, &mut frame);
    frame
}

// ---------------------------------------------------------------------------
// VLAN tag/untag operations
// ---------------------------------------------------------------------------

/// Add a VLAN tag to an existing frame.
/// Takes a standard Ethernet frame and returns a VLAN-tagged frame.
pub fn vlan_tag_frame(frame: &[u8], vlan: &VlanTag) -> Vec<u8> {
    if frame.len() < HEADER_SIZE {
        return Vec::from(frame);
    }

    let mut tagged = Vec::with_capacity(frame.len() + 4);
    // Copy dst + src MAC (12 bytes)
    tagged.extend_from_slice(&frame[0..12]);
    // Insert VLAN tag
    tagged.extend_from_slice(&vlan.to_bytes());
    // Copy original ethertype + payload
    tagged.extend_from_slice(&frame[12..]);

    tagged
}

/// Remove a VLAN tag from a tagged frame.
/// Returns (VlanTag, untagged_frame) if the frame has a VLAN tag.
pub fn vlan_untag_frame(frame: &[u8]) -> Option<(VlanTag, Vec<u8>)> {
    if frame.len() < VLAN_HEADER_SIZE {
        return None;
    }

    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype != ETHERTYPE_VLAN {
        return None;
    }

    let vlan = VlanTag::parse(&frame[12..16])?;

    let mut untagged = Vec::with_capacity(frame.len() - 4);
    // Copy dst + src MAC
    untagged.extend_from_slice(&frame[0..12]);
    // Copy real ethertype + payload (skip VLAN tag at 12..16)
    untagged.extend_from_slice(&frame[16..]);

    Some((vlan, untagged))
}

/// Extract the actual ethertype from a frame (handles VLAN-tagged frames).
pub fn get_ethertype(frame: &[u8]) -> Option<u16> {
    if frame.len() < HEADER_SIZE {
        return None;
    }
    let et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == ETHERTYPE_VLAN {
        if frame.len() < VLAN_HEADER_SIZE {
            return None;
        }
        Some(u16::from_be_bytes([frame[16], frame[17]]))
    } else {
        Some(et)
    }
}

// ---------------------------------------------------------------------------
// Frame validation
// ---------------------------------------------------------------------------

/// Validate an Ethernet frame.
/// Checks minimum and maximum size.
/// Returns Ok(payload_offset) or an error description.
pub fn validate_frame(frame: &[u8]) -> Result<usize, &'static str> {
    if frame.len() < MIN_FRAME_SIZE {
        return Err("Frame too short (< 60 bytes)");
    }
    if frame.len() > MAX_VLAN_FRAME_SIZE {
        return Err("Frame too long (> 1518 bytes)");
    }

    let et = u16::from_be_bytes([frame[12], frame[13]]);
    if et == ETHERTYPE_VLAN {
        if frame.len() < VLAN_HEADER_SIZE {
            return Err("VLAN frame too short");
        }
        Ok(VLAN_HEADER_SIZE)
    } else {
        Ok(HEADER_SIZE)
    }
}

// ---------------------------------------------------------------------------
// CRC-32 (Ethernet FCS)
// ---------------------------------------------------------------------------

/// CRC-32 lookup table for Ethernet FCS.
/// Generated from polynomial 0xEDB88320 (reflected).
const CRC32_TABLE: [u32; 256] = generate_crc32_table();

/// Generate CRC-32 lookup table at compile time.
const fn generate_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
}

/// Compute CRC-32 for Ethernet FCS.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        let index = ((crc ^ byte as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[index];
    }
    crc ^ 0xFFFFFFFF
}

/// Verify the FCS of an Ethernet frame.
/// The last 4 bytes should be the CRC-32 of everything before them.
pub fn verify_fcs(frame_with_fcs: &[u8]) -> bool {
    if frame_with_fcs.len() < HEADER_SIZE + FCS_SIZE {
        return false;
    }
    let data_len = frame_with_fcs.len() - FCS_SIZE;
    let computed = crc32(&frame_with_fcs[..data_len]);
    let stored = u32::from_le_bytes([
        frame_with_fcs[data_len],
        frame_with_fcs[data_len + 1],
        frame_with_fcs[data_len + 2],
        frame_with_fcs[data_len + 3],
    ]);
    computed == stored
}

/// Append FCS to a frame.
pub fn append_fcs(frame: &mut Vec<u8>) {
    let fcs = crc32(frame);
    frame.extend_from_slice(&fcs.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Promiscuous mode
// ---------------------------------------------------------------------------

/// Global promiscuous mode flag.
/// When true, the NIC should receive all frames, not just those addressed to us.
static PROMISCUOUS_MODE: Mutex<bool> = Mutex::new(false);

/// Set promiscuous mode.
pub fn set_promiscuous(enabled: bool) {
    *PROMISCUOUS_MODE.lock() = enabled;
}

/// Check if promiscuous mode is enabled.
pub fn is_promiscuous() -> bool {
    *PROMISCUOUS_MODE.lock()
}

/// Check if a frame should be accepted based on the destination MAC.
/// In promiscuous mode, all frames are accepted.
/// Otherwise, only frames addressed to `our_mac`, broadcast, or multicast are accepted.
pub fn should_accept(frame: &[u8], our_mac: MacAddr) -> bool {
    if is_promiscuous() {
        return true;
    }
    if frame.len() < 6 {
        return false;
    }
    let dst = [frame[0], frame[1], frame[2], frame[3], frame[4], frame[5]];
    // Accept: our MAC, broadcast, or multicast
    dst == our_mac.0 || dst == [0xFF; 6] || (dst[0] & 0x01) != 0
}

// ---------------------------------------------------------------------------
// Frame statistics
// ---------------------------------------------------------------------------

/// Ethernet frame statistics counters
struct FrameStats {
    rx_packets: u64,
    rx_bytes: u64,
    rx_errors: u64,
    rx_dropped: u64,
    rx_multicast: u64,
    tx_packets: u64,
    tx_bytes: u64,
    tx_errors: u64,
    tx_dropped: u64,
}

static FRAME_STATS: Mutex<FrameStats> = Mutex::new(FrameStats {
    rx_packets: 0,
    rx_bytes: 0,
    rx_errors: 0,
    rx_dropped: 0,
    rx_multicast: 0,
    tx_packets: 0,
    tx_bytes: 0,
    tx_errors: 0,
    tx_dropped: 0,
});

/// Record a received frame.
pub fn stat_rx(bytes: usize, is_multicast: bool) {
    let mut stats = FRAME_STATS.lock();
    stats.rx_packets = stats.rx_packets.saturating_add(1);
    stats.rx_bytes = stats.rx_bytes.saturating_add(bytes as u64);
    if is_multicast {
        stats.rx_multicast = stats.rx_multicast.saturating_add(1);
    }
}

/// Record a receive error.
pub fn stat_rx_error() {
    let mut s = FRAME_STATS.lock();
    s.rx_errors = s.rx_errors.saturating_add(1);
}

/// Record a receive drop.
pub fn stat_rx_drop() {
    let mut s = FRAME_STATS.lock();
    s.rx_dropped = s.rx_dropped.saturating_add(1);
}

/// Record a transmitted frame.
pub fn stat_tx(bytes: usize) {
    let mut stats = FRAME_STATS.lock();
    stats.tx_packets = stats.tx_packets.saturating_add(1);
    stats.tx_bytes = stats.tx_bytes.saturating_add(bytes as u64);
}

/// Record a transmit error.
pub fn stat_tx_error() {
    let mut s = FRAME_STATS.lock();
    s.tx_errors = s.tx_errors.saturating_add(1);
}

/// Record a transmit drop.
pub fn stat_tx_drop() {
    let mut s = FRAME_STATS.lock();
    s.tx_dropped = s.tx_dropped.saturating_add(1);
}

/// Get a snapshot of frame statistics.
pub fn get_stats() -> (u64, u64, u64, u64, u64, u64, u64, u64, u64) {
    let s = FRAME_STATS.lock();
    (
        s.rx_packets,
        s.rx_bytes,
        s.rx_errors,
        s.rx_dropped,
        s.rx_multicast,
        s.tx_packets,
        s.tx_bytes,
        s.tx_errors,
        s.tx_dropped,
    )
}

/// Reset all statistics.
pub fn reset_stats() {
    let mut s = FRAME_STATS.lock();
    s.rx_packets = 0;
    s.rx_bytes = 0;
    s.rx_errors = 0;
    s.rx_dropped = 0;
    s.rx_multicast = 0;
    s.tx_packets = 0;
    s.tx_bytes = 0;
    s.tx_errors = 0;
    s.tx_dropped = 0;
}
