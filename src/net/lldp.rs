use crate::net::NetworkDriver;
/// LLDP — Link Layer Discovery Protocol (IEEE 802.1AB)
///
/// Allows network devices to advertise identity and capabilities on the local
/// network segment using a dedicated EtherType (0x88CC).
///
/// Design constraints (bare-metal kernel rules):
///   - no_std — no standard library
///   - No heap — no Vec / Box / String — all fixed-size static arrays
///   - No float casts (as f32 / as f64)
///   - Saturating arithmetic on counters, wrapping_add on sequences
///   - No panic — all fallible paths return early or produce a safe default
///   - MMIO via read_volatile / write_volatile only
///
/// TLV wire format (IEEE 802.1AB §9.6):
///   bits[15:9] = TLV type  (7 bits)
///   bits[8:0]  = TLV length (9 bits, value length in bytes)
///   followed by `length` bytes of value data
///
/// Inspired by: IEEE 802.1AB-2016. All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

pub const LLDP_ETH_TYPE: u16 = 0x88CC;
pub const LLDP_MULTICAST_ADDR: [u8; 6] = [0x01, 0x80, 0xC2, 0x00, 0x00, 0x0E];
pub const LLDP_TTL_DEFAULT: u16 = 120; // seconds

pub const MAX_LLDP_NEIGHBORS: usize = 32;
pub const LLDP_MAX_STR: usize = 256;

// TLV type codes (IEEE 802.1AB §8.5)
pub const LLDP_TLV_END: u8 = 0;
pub const LLDP_TLV_CHASSIS_ID: u8 = 1;
pub const LLDP_TLV_PORT_ID: u8 = 2;
pub const LLDP_TLV_TTL: u8 = 3;
pub const LLDP_TLV_PORT_DESC: u8 = 4;
pub const LLDP_TLV_SYS_NAME: u8 = 5;
pub const LLDP_TLV_SYS_DESC: u8 = 6;
pub const LLDP_TLV_SYS_CAP: u8 = 7;

// Chassis ID subtype (§8.5.2)
pub const LLDP_CHASSIS_MAC: u8 = 4; // MAC address subtype
                                    // Port ID subtype (§8.5.3)
pub const LLDP_PORT_MAC: u8 = 3; // MAC address subtype for port ID

// How often to re-transmit LLDP frames (30 seconds in ms)
const LLDP_TX_INTERVAL_MS: u64 = 30_000;

// ---------------------------------------------------------------------------
// LldpNeighbor
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct LldpNeighbor {
    pub chassis_id: [u8; 64],
    pub chassis_id_len: u8,
    pub port_id: [u8; 64],
    pub port_id_len: u8,
    pub ttl: u16,
    pub sys_name: [u8; LLDP_MAX_STR],
    pub sys_name_len: u8,
    pub sys_desc: [u8; LLDP_MAX_STR],
    pub sys_desc_len: u8,
    pub capabilities: u16, // bitmask per IEEE 802.1AB §8.5.8
    pub src_mac: [u8; 6],
    pub rx_ifindex: u8,
    pub last_seen_ms: u64, // monotonic ms when last LLDP received
    pub active: bool,
}

impl LldpNeighbor {
    pub const fn empty() -> Self {
        LldpNeighbor {
            chassis_id: [0u8; 64],
            chassis_id_len: 0,
            port_id: [0u8; 64],
            port_id_len: 0,
            ttl: 0,
            sys_name: [0u8; LLDP_MAX_STR],
            sys_name_len: 0,
            sys_desc: [0u8; LLDP_MAX_STR],
            sys_desc_len: 0,
            capabilities: 0,
            src_mac: [0u8; 6],
            rx_ifindex: 0,
            last_seen_ms: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static storage
// ---------------------------------------------------------------------------

const EMPTY_NEIGHBOR: LldpNeighbor = LldpNeighbor::empty();
static LLDP_NEIGHBORS: Mutex<[LldpNeighbor; MAX_LLDP_NEIGHBORS]> =
    Mutex::new([EMPTY_NEIGHBOR; MAX_LLDP_NEIGHBORS]);

/// Monotonic ms timestamp of the last LLDP frame we transmitted.
static LAST_SEND_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// TLV encoding helper
// ---------------------------------------------------------------------------

/// Write one LLDP TLV at position `pos` inside `buf`.
///
/// TLV header is 2 bytes big-endian: bits[15:9]=type, bits[8:0]=length.
/// Returns the new write position after the TLV, or 0 on overflow.
fn write_tlv(buf: &mut [u8], pos: usize, tlv_type: u8, value: &[u8]) -> usize {
    let vlen = value.len();
    // Max TLV value length is 511 bytes (9-bit length field).
    if vlen > 511 {
        return 0;
    }
    let needed = pos.saturating_add(2).saturating_add(vlen);
    if needed > buf.len() {
        return 0;
    }
    let header: u16 = ((tlv_type as u16) << 9) | (vlen as u16 & 0x01FF);
    buf[pos] = (header >> 8) as u8;
    buf[pos + 1] = (header & 0xFF) as u8;
    let data_start = pos + 2;
    buf[data_start..data_start + vlen].copy_from_slice(value);
    data_start + vlen
}

// ---------------------------------------------------------------------------
// Frame builder
// ---------------------------------------------------------------------------

/// Build a complete LLDP Ethernet frame into `out_buf`.
///
/// Layout:
///   [0..6]   Destination MAC  — LLDP nearest-bridge multicast 01:80:C2:00:00:0E
///   [6..12]  Source MAC       — `src_mac`
///   [12..14] EtherType        — 0x88CC
///   [14..]   LLDP PDU (TLVs):
///              ChassisID (MAC subtype) + PortID (MAC subtype) +
///              TTL + PortDescription + SystemName + EndOfLLDPDU
///
/// Returns the total frame length in bytes, or 0 on error.
pub fn lldp_build_frame(
    src_mac: [u8; 6],
    sys_name: &[u8],
    port_desc: &[u8],
    out_buf: &mut [u8; 512],
) -> usize {
    // Zero the output buffer.
    for b in out_buf.iter_mut() {
        *b = 0;
    }

    // Ethernet header (14 bytes)
    out_buf[0..6].copy_from_slice(&LLDP_MULTICAST_ADDR);
    out_buf[6..12].copy_from_slice(&src_mac);
    out_buf[12] = (LLDP_ETH_TYPE >> 8) as u8;
    out_buf[13] = (LLDP_ETH_TYPE & 0xFF) as u8;

    let mut pos: usize = 14; // start of LLDP PDU

    // --- ChassisID TLV: subtype (1 B) + MAC address (6 B) = 7 bytes ---
    {
        let mut val = [0u8; 7];
        val[0] = LLDP_CHASSIS_MAC;
        val[1..7].copy_from_slice(&src_mac);
        pos = write_tlv(out_buf, pos, LLDP_TLV_CHASSIS_ID, &val);
        if pos == 0 {
            return 0;
        }
    }

    // --- PortID TLV: subtype (1 B) + MAC address (6 B) = 7 bytes ---
    {
        let mut val = [0u8; 7];
        val[0] = LLDP_PORT_MAC;
        val[1..7].copy_from_slice(&src_mac);
        pos = write_tlv(out_buf, pos, LLDP_TLV_PORT_ID, &val);
        if pos == 0 {
            return 0;
        }
    }

    // --- TTL TLV: 2 bytes big-endian ---
    {
        let ttl_val = [
            (LLDP_TTL_DEFAULT >> 8) as u8,
            (LLDP_TTL_DEFAULT & 0xFF) as u8,
        ];
        pos = write_tlv(out_buf, pos, LLDP_TLV_TTL, &ttl_val);
        if pos == 0 {
            return 0;
        }
    }

    // --- PortDescription TLV (optional) ---
    if !port_desc.is_empty() {
        let truncated_len = port_desc.len().min(255);
        pos = write_tlv(
            out_buf,
            pos,
            LLDP_TLV_PORT_DESC,
            &port_desc[..truncated_len],
        );
        if pos == 0 {
            return 0;
        }
    }

    // --- SystemName TLV (optional) ---
    if !sys_name.is_empty() {
        let truncated_len = sys_name.len().min(255);
        pos = write_tlv(out_buf, pos, LLDP_TLV_SYS_NAME, &sys_name[..truncated_len]);
        if pos == 0 {
            return 0;
        }
    }

    // --- End-of-LLDPDU TLV: type=0, length=0 (2 zero bytes) ---
    if pos.saturating_add(2) > 512 {
        return 0;
    }
    out_buf[pos] = 0;
    out_buf[pos + 1] = 0;
    pos = pos.saturating_add(2);

    pos // total frame length
}

// ---------------------------------------------------------------------------
// Frame parser
// ---------------------------------------------------------------------------

/// Parse an incoming LLDP Ethernet frame and update the neighbor table.
///
/// `frame` is a raw buffer including the full Ethernet header (14 bytes)
/// followed by the LLDP PDU.  `len` is the valid byte count within `frame`.
///
/// Returns `true` if the frame contained a valid LLDP PDU and was processed.
pub fn lldp_parse_frame(frame: &[u8], len: usize, current_ms: u64) -> bool {
    // Minimum viable frame: 14 (Eth) + 2 (End TLV) = 16 bytes.
    if len < 16 || len > frame.len() {
        return false;
    }

    // Verify EtherType
    let ethertype = ((frame[12] as u16) << 8) | (frame[13] as u16);
    if ethertype != LLDP_ETH_TYPE {
        return false;
    }

    let src_mac: [u8; 6] = [frame[6], frame[7], frame[8], frame[9], frame[10], frame[11]];

    let mut neighbor = LldpNeighbor::empty();
    neighbor.src_mac = src_mac;
    neighbor.last_seen_ms = current_ms;

    let mut pos: usize = 14; // start of LLDP PDU
    let mut has_chassis = false;
    let mut has_port = false;
    let mut has_ttl = false;

    loop {
        // Need at least 2 bytes for TLV header
        if pos.saturating_add(2) > len {
            break;
        }
        let header = ((frame[pos] as u16) << 8) | (frame[pos + 1] as u16);
        let tlv_type = ((header >> 9) & 0x7F) as u8;
        let tlv_len = (header & 0x01FF) as usize;
        pos = pos.saturating_add(2);

        // End-of-LLDPDU: stop parsing
        if tlv_type == LLDP_TLV_END {
            break;
        }

        // Guard: enough bytes remaining for this TLV's value?
        if pos.saturating_add(tlv_len) > len {
            break;
        }

        let value = &frame[pos..pos + tlv_len];

        match tlv_type {
            LLDP_TLV_CHASSIS_ID => {
                let copy_len = tlv_len.min(64);
                neighbor.chassis_id[..copy_len].copy_from_slice(&value[..copy_len]);
                neighbor.chassis_id_len = copy_len as u8;
                has_chassis = true;
            }
            LLDP_TLV_PORT_ID => {
                let copy_len = tlv_len.min(64);
                neighbor.port_id[..copy_len].copy_from_slice(&value[..copy_len]);
                neighbor.port_id_len = copy_len as u8;
                has_port = true;
            }
            LLDP_TLV_TTL => {
                if tlv_len >= 2 {
                    neighbor.ttl = ((value[0] as u16) << 8) | (value[1] as u16);
                }
                has_ttl = true;
            }
            LLDP_TLV_SYS_NAME => {
                let copy_len = tlv_len.min(LLDP_MAX_STR);
                neighbor.sys_name[..copy_len].copy_from_slice(&value[..copy_len]);
                neighbor.sys_name_len = copy_len as u8;
            }
            LLDP_TLV_SYS_DESC => {
                let copy_len = tlv_len.min(LLDP_MAX_STR);
                neighbor.sys_desc[..copy_len].copy_from_slice(&value[..copy_len]);
                neighbor.sys_desc_len = copy_len as u8;
            }
            LLDP_TLV_SYS_CAP => {
                // 2 bytes: declared capabilities bitmask
                if tlv_len >= 2 {
                    neighbor.capabilities = ((value[0] as u16) << 8) | (value[1] as u16);
                }
            }
            _ => { /* Unknown / reserved TLV — skip */ }
        }

        pos = pos.saturating_add(tlv_len);
    }

    // IEEE 802.1AB §9.2: ChassisID + PortID + TTL are mandatory.
    if !has_chassis || !has_port || !has_ttl {
        return false;
    }

    neighbor.active = true;

    let mut table = LLDP_NEIGHBORS.lock();

    // Refresh existing entry matching on chassis_id bytes
    for slot in table.iter_mut() {
        if slot.active
            && slot.chassis_id_len == neighbor.chassis_id_len
            && slot.chassis_id[..slot.chassis_id_len as usize]
                == neighbor.chassis_id[..neighbor.chassis_id_len as usize]
        {
            *slot = neighbor;
            return true;
        }
    }

    // Insert into the first empty slot
    for slot in table.iter_mut() {
        if !slot.active {
            *slot = neighbor;
            return true;
        }
    }

    // Table full — evict the oldest entry (smallest last_seen_ms)
    let mut oldest_idx = 0usize;
    let mut oldest_ms = u64::MAX;
    for (i, slot) in table.iter().enumerate() {
        if slot.last_seen_ms < oldest_ms {
            oldest_ms = slot.last_seen_ms;
            oldest_idx = i;
        }
    }
    table[oldest_idx] = neighbor;
    true
}

// ---------------------------------------------------------------------------
// Neighbor accessors
// ---------------------------------------------------------------------------

/// Copy up to `max` active neighbors into `out`.
///
/// Returns the number of entries written.
pub fn lldp_get_neighbors(out: &mut [LldpNeighbor], max: usize) -> usize {
    let table = LLDP_NEIGHBORS.lock();
    let mut count = 0usize;
    for slot in table.iter() {
        if count >= max || count >= out.len() {
            break;
        }
        if slot.active {
            out[count] = *slot;
            count = count.saturating_add(1);
        }
    }
    count
}

// ---------------------------------------------------------------------------
// TTL expiry
// ---------------------------------------------------------------------------

/// Mark neighbors as inactive once their TTL has elapsed.
///
/// `current_ms - last_seen_ms > ttl * 1000` → inactive.
/// TTL == 0 means the neighbor sent a shutdown LLDPDU — expire immediately.
pub fn lldp_expire_neighbors(current_ms: u64) {
    let mut table = LLDP_NEIGHBORS.lock();
    for slot in table.iter_mut() {
        if !slot.active {
            continue;
        }
        if slot.ttl == 0 {
            slot.active = false;
            continue;
        }
        let ttl_ms = (slot.ttl as u64).saturating_mul(1000);
        let elapsed = current_ms.saturating_sub(slot.last_seen_ms);
        if elapsed > ttl_ms {
            slot.active = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Periodic tick
// ---------------------------------------------------------------------------

/// Periodic LLDP timer tick.
///
/// Expires stale neighbor entries on every call, then — if 30 s have elapsed
/// since the last transmission — builds and sends a fresh LLDP frame via the
/// e1000 NIC driver.
///
/// `current_ms` — monotonic system time in milliseconds since boot.
pub fn lldp_tick(current_ms: u64) {
    lldp_expire_neighbors(current_ms);

    let last = LAST_SEND_MS.load(Ordering::Relaxed);
    if current_ms.saturating_sub(last) < LLDP_TX_INTERVAL_MS {
        return;
    }
    LAST_SEND_MS.store(current_ms, Ordering::Relaxed);

    // Acquire the NIC driver once: read MAC, build frame, transmit.
    // Mirrors the IGMP send_report_now() pattern — single lock acquisition.
    {
        let driver = crate::drivers::e1000::driver().lock();
        if let Some(ref nic) = *driver {
            let mac = nic.mac_addr().0;
            let sys_name = b"genesis-aios";
            let port_desc = b"eth0";
            let mut frame = [0u8; 512];
            let flen = lldp_build_frame(mac, sys_name, port_desc, &mut frame);
            if flen > 0 {
                let _ = nic.send(&frame[..flen]);
                serial_println!("[lldp] sent LLDP frame ({} bytes)", flen);
            }
        }
    } // driver lock released
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the LLDP subsystem.
///
/// Clears the neighbor table and resets the transmit timer.
pub fn init() {
    LAST_SEND_MS.store(0, Ordering::Relaxed);
    let mut table = LLDP_NEIGHBORS.lock();
    for slot in table.iter_mut() {
        *slot = LldpNeighbor::empty();
    }
    drop(table);
    serial_println!("[lldp] LLDP neighbor discovery initialized");
}
