/// IEEE 802.1Q VLAN tagging for Genesis
///
/// Provides VLAN tag insertion/removal, a static VLAN device table,
/// trunk/access port configuration, and priority code point (PCP) handling.
///
/// No heap allocations are used anywhere in this module.
///
/// Inspired by: IEEE 802.1Q, Linux VLAN (net/8021q/). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// 802.1Q EtherType / TPID — tag protocol identifier.
pub const VLAN_ETH_P: u16 = 0x8100;

/// Backward-compat alias.
pub const VLAN_ETHERTYPE: u16 = VLAN_ETH_P;

/// 802.1ad QinQ EtherType.
pub const ETHERTYPE_QINQ: u16 = 0x88A8;

/// Backward-compat alias used by older code.
pub const ETHERTYPE_8021Q: u16 = VLAN_ETH_P;

/// Maximum legal VLAN ID (12-bit field: 0–4095 per spec; 1–4094 valid).
pub const VLAN_MAX_VID: u16 = 4095;

/// Legacy alias.
pub const VLAN_ID_MAX: u16 = 4094;

/// Reserved VLAN ID meaning "no VLAN" (null VLAN).
pub const VLAN_ID_NONE: u16 = 0;

/// Default VLAN ID (802.1Q default).
pub const VLAN_ID_DEFAULT: u16 = 1;

/// Maximum number of virtual VLAN interfaces.
pub const VLAN_MAX_DEVICES: usize = 64;

/// Size of the 802.1Q tag inserted between src MAC and inner EtherType.
pub const VLAN_HDR_SIZE: usize = 4; // 2-byte TCI + 2-byte inner EtherType

/// Maximum number of VLAN sub-interfaces (legacy alias).
pub const MAX_VLAN_IFACES: usize = VLAN_MAX_DEVICES;

// ---------------------------------------------------------------------------
// TCI helpers (inline)
// ---------------------------------------------------------------------------

/// Build a Tag Control Information word from PCP, DEI, and VID.
///
/// Bits 15:13 = PCP (3 bits), bit 12 = DEI, bits 11:0 = VID.
#[inline]
pub fn vlan_tci_make(pcp: u8, dei: bool, vid: u16) -> u16 {
    (((pcp & 0x7) as u16) << 13) | (if dei { 1u16 << 12 } else { 0u16 }) | (vid & 0xFFF)
}

/// Extract VLAN ID (bits 11:0) from a TCI word.
#[inline]
pub fn vlan_tci_vid(tci: u16) -> u16 {
    tci & 0xFFF
}

/// Extract Priority Code Point (bits 15:13) from a TCI word.
#[inline]
pub fn vlan_tci_pcp(tci: u16) -> u8 {
    ((tci >> 13) & 0x7) as u8
}

/// Extract Drop Eligible Indicator (bit 12) from a TCI word.
#[inline]
pub fn vlan_tci_dei(tci: u16) -> bool {
    (tci >> 12) & 1 == 1
}

// ---------------------------------------------------------------------------
// Priority Code Point enum (802.1p)
// ---------------------------------------------------------------------------

/// Priority Code Point values (3-bit field, 0–7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pcp {
    BestEffort = 0,
    Background = 1,
    ExcellentEffort = 2,
    CriticalApps = 3,
    Video = 4,
    Voice = 5,
    InternetworkCtrl = 6,
    NetworkCtrl = 7,
}

impl Pcp {
    /// Construct a `Pcp` from its 3-bit numeric value.
    pub fn from_u8(val: u8) -> Self {
        match val & 0x07 {
            1 => Pcp::Background,
            2 => Pcp::ExcellentEffort,
            3 => Pcp::CriticalApps,
            4 => Pcp::Video,
            5 => Pcp::Voice,
            6 => Pcp::InternetworkCtrl,
            7 => Pcp::NetworkCtrl,
            _ => Pcp::BestEffort,
        }
    }
}

// ---------------------------------------------------------------------------
// VlanHeader — the 4-byte 802.1Q tag that sits after the two MAC addresses
// ---------------------------------------------------------------------------

/// The two-field 802.1Q tag that follows dst+src MAC in a tagged frame.
///
///  +---------------------------------------------+
///  | tci (16 bits)              | inner_ethertype |
///  | [PCP(3)][DEI(1)][VID(12)] | e.g. 0x0800 IPv4|
///  +---------------------------------------------+
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VlanHeader {
    /// Tag Control Information: PCP(15:13) | DEI(12) | VID(11:0).
    pub tci: u16,
    /// EtherType of the payload following this tag.
    pub inner_ethertype: u16,
}

impl VlanHeader {
    /// Extract the VLAN Identifier (bits 11:0 of TCI).
    #[inline]
    pub fn vid(&self) -> u16 {
        self.tci & 0x0FFF
    }

    /// Extract the Priority Code Point (bits 15:13 of TCI).
    #[inline]
    pub fn pcp(&self) -> u8 {
        ((self.tci >> 13) & 0x07) as u8
    }

    /// Extract the Drop Eligible Indicator (bit 12 of TCI).
    #[inline]
    pub fn dei(&self) -> bool {
        (self.tci >> 12) & 1 != 0
    }
}

// ---------------------------------------------------------------------------
// VlanTag — parsed representation of an 802.1Q tag
// ---------------------------------------------------------------------------

/// Parsed 802.1Q VLAN tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VlanTag {
    /// Priority Code Point (3 bits, 0–7).
    pub pcp: u8,
    /// Drop Eligible Indicator (1 bit).
    pub dei: bool,
    /// VLAN Identifier (12 bits, 1–4094).
    pub vid: u16,
}

impl VlanTag {
    /// Construct a new tag, masking fields to their valid widths.
    pub fn new(vid: u16, pcp: u8, dei: bool) -> Self {
        VlanTag {
            pcp: pcp & 0x07,
            dei,
            vid: vid & 0x0FFF,
        }
    }

    /// Encode the tag to a 2-byte TCI value (big-endian ready).
    pub fn to_tci(&self) -> u16 {
        ((self.pcp as u16 & 0x07) << 13) | ((self.dei as u16) << 12) | (self.vid & 0x0FFF)
    }

    /// Decode a TCI value back into a `VlanTag`.
    pub fn from_tci(tci: u16) -> Self {
        VlanTag {
            pcp: ((tci >> 13) & 0x07) as u8,
            dei: (tci >> 12) & 1 != 0,
            vid: tci & 0x0FFF,
        }
    }
}

// ---------------------------------------------------------------------------
// VlanDevice — a virtual VLAN interface bound to a parent physical interface
// ---------------------------------------------------------------------------

/// A logical VLAN device (e.g. eth0.100).
///
/// Must be `Copy` for storage in `static Mutex<[VlanDevice; N]>`.
#[derive(Clone, Copy)]
pub struct VlanDevice {
    /// VLAN ID this interface belongs to.
    pub vid: u16,
    /// Index of the parent physical interface.
    pub parent_ifindex: u8,
    /// This VLAN interface's own index.
    pub ifindex: u8,
    pub rx_pkts: u64,
    pub tx_pkts: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// True when this slot is in use and the interface is up.
    pub active: bool,
}

impl VlanDevice {
    pub const fn empty() -> Self {
        VlanDevice {
            vid: 0,
            parent_ifindex: 0,
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
// Port mode (access / trunk / hybrid)
// ---------------------------------------------------------------------------

/// How a port handles VLAN tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMode {
    /// Access port: single VLAN, frames sent/received untagged.
    Access,
    /// Trunk port: carries multiple VLANs, frames are tagged.
    Trunk,
    /// Hybrid: mix of tagged and untagged VLANs.
    Hybrid,
}

// ---------------------------------------------------------------------------
// Port configuration table
// ---------------------------------------------------------------------------

const MAX_PORT_CONFIGS: usize = 16;
const MAX_ALLOWED_VLANS: usize = 32;

/// A static port VLAN configuration record.
#[derive(Clone, Copy)]
pub struct VlanPortConfig {
    pub iface_id: u32,
    pub mode: PortMode,
    pub native_vlan: u16,
    pub allowed_count: usize,
    pub allowed_vlans: [u16; MAX_ALLOWED_VLANS],
    pub default_pcp: u8,
    pub active: bool,
}

impl VlanPortConfig {
    pub const fn empty() -> Self {
        VlanPortConfig {
            iface_id: 0,
            mode: PortMode::Access,
            native_vlan: VLAN_ID_DEFAULT,
            allowed_count: 0,
            allowed_vlans: [0u16; MAX_ALLOWED_VLANS],
            default_pcp: 0,
            active: false,
        }
    }

    pub fn access(iface_id: u32, vid: u16) -> Self {
        let mut cfg = Self::empty();
        cfg.iface_id = iface_id;
        cfg.mode = PortMode::Access;
        cfg.native_vlan = vid;
        cfg.allowed_vlans[0] = vid;
        cfg.allowed_count = 1;
        cfg.active = true;
        cfg
    }

    pub fn trunk(iface_id: u32, allowed: &[u16], native: u16) -> Self {
        let mut cfg = Self::empty();
        cfg.iface_id = iface_id;
        cfg.mode = PortMode::Trunk;
        cfg.native_vlan = native;
        cfg.active = true;
        let count = if allowed.len() < MAX_ALLOWED_VLANS {
            allowed.len()
        } else {
            MAX_ALLOWED_VLANS
        };
        let mut i = 0;
        while i < count {
            cfg.allowed_vlans[i] = allowed[i];
            i += 1;
        }
        cfg.allowed_count = count;
        cfg
    }

    pub fn is_vlan_allowed(&self, vid: u16) -> bool {
        let mut i = 0;
        while i < self.allowed_count {
            if self.allowed_vlans[i] == vid {
                return true;
            }
            i += 1;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// VLAN database (VID → active flag)
// ---------------------------------------------------------------------------

const MAX_VLAN_DB: usize = 64;

#[derive(Clone, Copy)]
struct VlanDbEntry {
    vid: u16,
    active: bool,
}

impl VlanDbEntry {
    const fn empty() -> Self {
        VlanDbEntry {
            vid: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Consolidated global state
// ---------------------------------------------------------------------------

struct VlanState {
    db: [VlanDbEntry; MAX_VLAN_DB],
    db_count: usize,
    ports: [VlanPortConfig; MAX_PORT_CONFIGS],
}

impl VlanState {
    const fn new() -> Self {
        VlanState {
            db: [VlanDbEntry::empty(); MAX_VLAN_DB],
            db_count: 0,
            ports: [VlanPortConfig::empty(); MAX_PORT_CONFIGS],
        }
    }
}

static VLAN_STATE: Mutex<VlanState> = Mutex::new(VlanState::new());
static VLAN_DEVICES: Mutex<[VlanDevice; VLAN_MAX_DEVICES]> =
    Mutex::new([VlanDevice::empty(); VLAN_MAX_DEVICES]);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the VLAN subsystem.
pub fn init() {
    {
        let mut st = VLAN_STATE.lock();
        st.db[0] = VlanDbEntry {
            vid: VLAN_ID_DEFAULT,
            active: true,
        };
        st.db_count = 1;
    }
    serial_println!("[vlan] IEEE 802.1Q VLAN driver initialized");
}

// ---------------------------------------------------------------------------
// VlanDevice management (spec API)
// ---------------------------------------------------------------------------

/// Add a VLAN device for the given VID on `parent_ifindex`.
///
/// Returns the new VLAN interface ifindex on success, or `None` if the
/// (vid, parent_ifindex) pair already exists or the table is full.
pub fn vlan_device_add(vid: u16, parent_ifindex: u8) -> Option<u8> {
    if vid == 0 || vid > VLAN_MAX_VID {
        return None;
    }
    let mut devs = VLAN_DEVICES.lock();
    // Reject duplicate (same vid+parent).
    let mut i = 0;
    while i < VLAN_MAX_DEVICES {
        if devs[i].active && devs[i].vid == vid && devs[i].parent_ifindex == parent_ifindex {
            return None;
        }
        i += 1;
    }
    // Find a free slot; use slot index as ifindex.
    let mut j = 0;
    while j < VLAN_MAX_DEVICES {
        if !devs[j].active {
            devs[j] = VlanDevice {
                vid,
                parent_ifindex,
                ifindex: j as u8,
                rx_pkts: 0,
                tx_pkts: 0,
                rx_bytes: 0,
                tx_bytes: 0,
                active: true,
            };
            serial_println!(
                "[vlan] added VID {} on parent {} -> ifindex {}",
                vid,
                parent_ifindex,
                j
            );
            return Some(j as u8);
        }
        j += 1;
    }
    None
}

/// Remove a VLAN device identified by (vid, parent_ifindex).
///
/// Returns `true` on success.
pub fn vlan_device_del(vid: u16, parent_ifindex: u8) -> bool {
    let mut devs = VLAN_DEVICES.lock();
    let mut i = 0;
    while i < VLAN_MAX_DEVICES {
        if devs[i].active && devs[i].vid == vid && devs[i].parent_ifindex == parent_ifindex {
            devs[i] = VlanDevice::empty();
            serial_println!("[vlan] removed VID {} on parent {}", vid, parent_ifindex);
            return true;
        }
        i += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Frame-level tagging / untagging (spec API)
// ---------------------------------------------------------------------------

/// Tag an outgoing Ethernet frame with an 802.1Q header.
///
/// Input layout:  `[dst(6)][src(6)][ethertype(2)][payload…]`
/// Output layout: `[dst(6)][src(6)][0x8100(2)][TCI(2)][ethertype(2)][payload…]`
///
/// Returns total length written into `out`, or 0 on error.
pub fn vlan_tag_frame(
    vid: u16,
    pcp: u8,
    in_frame: &[u8],
    in_len: usize,
    out: &mut [u8; 1518],
) -> usize {
    if in_len < 14 || in_len > 1514 {
        return 0;
    }
    let new_len = in_len.saturating_add(4);
    if new_len > 1518 {
        return 0;
    }

    let tci = vlan_tci_make(pcp, false, vid);

    // dst[6] + src[6]
    let mut k = 0;
    while k < 12 {
        out[k] = in_frame[k];
        k += 1;
    }
    // TPID = 0x8100
    out[12] = (VLAN_ETH_P >> 8) as u8;
    out[13] = (VLAN_ETH_P & 0xFF) as u8;
    // TCI
    out[14] = (tci >> 8) as u8;
    out[15] = (tci & 0xFF) as u8;
    // original EtherType + payload (bytes 12..in_len of input)
    let rest_len = in_len - 12;
    let mut m = 0;
    while m < rest_len {
        out[16 + m] = in_frame[12 + m];
        m += 1;
    }
    new_len
}

/// Strip an 802.1Q tag from a received frame.
///
/// Checks that EtherType at offset 12 == 0x8100, extracts TCI, reassembles
/// the frame without the 4-byte tag, and returns `(new_len, vid)`.
/// Returns `(0, 0)` if the frame is too short or not tagged.
pub fn vlan_untag_frame(frame: &[u8], len: usize, out: &mut [u8; 1514]) -> (usize, u16) {
    if len < 18 {
        return (0, 0);
    }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != VLAN_ETH_P {
        return (0, 0);
    }
    let tci = u16::from_be_bytes([frame[14], frame[15]]);
    let vid = vlan_tci_vid(tci);

    let new_len = len.saturating_sub(4);
    if new_len < 14 {
        return (0, 0);
    }
    if new_len > 1514 {
        return (0, 0);
    }

    // dst[6] + src[6]
    let mut k = 0;
    while k < 12 {
        out[k] = frame[k];
        k += 1;
    }
    // bytes 16..len (inner EtherType + payload)
    let tail = len - 16;
    let mut m = 0;
    while m < tail {
        out[12 + m] = frame[16 + m];
        m += 1;
    }
    (new_len, vid)
}

/// Process an incoming frame: if tagged, look up the VLAN device and update stats.
pub fn vlan_rx(frame: &[u8], len: usize, src_ifindex: u8) {
    if len < 18 {
        return;
    }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != VLAN_ETH_P && tpid != ETHERTYPE_QINQ {
        return;
    }
    let tci = u16::from_be_bytes([frame[14], frame[15]]);
    let vid = vlan_tci_vid(tci);

    let mut devs = VLAN_DEVICES.lock();
    let mut i = 0;
    while i < VLAN_MAX_DEVICES {
        if devs[i].active && devs[i].vid == vid && devs[i].parent_ifindex == src_ifindex {
            devs[i].rx_pkts = devs[i].rx_pkts.saturating_add(1);
            devs[i].rx_bytes = devs[i].rx_bytes.saturating_add(len as u64);
            return;
        }
        i += 1;
    }
    serial_println!(
        "[vlan] rx: no device for VID {} on ifindex {} — drop",
        vid,
        src_ifindex
    );
}

/// Tag a frame and record TX stats for the matching VLAN device.
///
/// Returns the tagged frame length written into a caller-supplied stack buffer;
/// callers that do not need the actual bytes may pass a dummy buffer.
pub fn vlan_tx(vid: u16, parent_ifindex: u8, frame: &[u8], len: usize) -> usize {
    if len < 14 || len > 1514 {
        return 0;
    }
    let tagged_len = len.saturating_add(4);
    if tagged_len > 1518 {
        return 0;
    }

    {
        let mut devs = VLAN_DEVICES.lock();
        let mut i = 0;
        while i < VLAN_MAX_DEVICES {
            if devs[i].active && devs[i].vid == vid && devs[i].parent_ifindex == parent_ifindex {
                devs[i].tx_pkts = devs[i].tx_pkts.saturating_add(1);
                devs[i].tx_bytes = devs[i].tx_bytes.saturating_add(tagged_len as u64);
                break;
            }
            i += 1;
        }
    }
    tagged_len
}

// ---------------------------------------------------------------------------
// Legacy / extended API (kept for existing callers in the codebase)
// ---------------------------------------------------------------------------

/// Create a VLAN in the internal database.
pub fn create_vlan(vid: u16) -> bool {
    if vid == 0 || vid > VLAN_ID_MAX {
        return false;
    }
    let mut st = VLAN_STATE.lock();
    let mut i = 0;
    while i < st.db_count {
        if st.db[i].vid == vid {
            return false;
        }
        i += 1;
    }
    if st.db_count >= MAX_VLAN_DB {
        return false;
    }
    let idx = st.db_count;
    st.db[idx] = VlanDbEntry { vid, active: true };
    st.db_count = st.db_count.saturating_add(1);
    serial_println!("[vlan] created VID {}", vid);
    true
}

/// Remove a VLAN from the internal database. Cannot remove the default VLAN.
pub fn delete_vlan(vid: u16) -> bool {
    if vid == VLAN_ID_DEFAULT {
        return false;
    }
    let mut st = VLAN_STATE.lock();
    let mut found = MAX_VLAN_DB; // sentinel
    let mut i = 0;
    while i < st.db_count {
        if st.db[i].vid == vid {
            found = i;
            break;
        }
        i += 1;
    }
    if found == MAX_VLAN_DB {
        return false;
    }
    let last = st.db_count.saturating_sub(1);
    st.db[found] = st.db[last];
    st.db[last] = VlanDbEntry::empty();
    st.db_count = last;
    true
}

/// Register or replace a port VLAN configuration.
pub fn set_port_config(config: VlanPortConfig) {
    let mut st = VLAN_STATE.lock();
    let mut i = 0;
    while i < MAX_PORT_CONFIGS {
        if st.ports[i].active && st.ports[i].iface_id == config.iface_id {
            st.ports[i] = config;
            return;
        }
        i += 1;
    }
    let mut j = 0;
    while j < MAX_PORT_CONFIGS {
        if !st.ports[j].active {
            st.ports[j] = config;
            return;
        }
        j += 1;
    }
}

/// Retrieve the port configuration for `iface_id`, if any.
pub fn get_port_config(iface_id: u32) -> Option<VlanPortConfig> {
    let st = VLAN_STATE.lock();
    let mut i = 0;
    while i < MAX_PORT_CONFIGS {
        if st.ports[i].active && st.ports[i].iface_id == iface_id {
            return Some(st.ports[i]);
        }
        i += 1;
    }
    None
}

/// Add a VLAN sub-interface binding `vlan_id` to a parent interface index.
///
/// Returns `true` on success.
pub fn vlan_add(vlan_id: u16, parent_idx: u32) -> bool {
    if vlan_id == 0 || vlan_id > VLAN_ID_MAX {
        return false;
    }
    if parent_idx > 0xFF {
        return false;
    }
    vlan_device_add(vlan_id, parent_idx as u8).is_some()
}

/// Remove a VLAN sub-interface by its VLAN ID (any parent).
pub fn vlan_remove(vlan_id: u16) -> bool {
    let mut devs = VLAN_DEVICES.lock();
    let mut i = 0;
    while i < VLAN_MAX_DEVICES {
        if devs[i].active && devs[i].vid == vlan_id {
            let parent = devs[i].parent_ifindex;
            devs[i] = VlanDevice::empty();
            serial_println!("[vlan] removed interface VID {} parent {}", vlan_id, parent);
            return true;
        }
        i += 1;
    }
    false
}

/// Insert an 802.1Q tag into `frame` (legacy name, delegates to `vlan_tag_frame`).
///
/// Returns the new frame length on success, or 0.
pub fn vlan_output(vlan_id: u16, frame: &[u8], frame_len: usize, out: &mut [u8; 1518]) -> usize {
    vlan_tag_frame(vlan_id, 0, frame, frame_len, out)
}

/// Check whether a frame carries an 802.1Q tag.
pub fn vlan_is_tagged(frame: &[u8], len: usize) -> bool {
    if len < 14 {
        return false;
    }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    tpid == VLAN_ETH_P || tpid == ETHERTYPE_QINQ
}

/// Extract the VLAN ID from a tagged frame (returns 0 when untagged or short).
pub fn vlan_get_id(frame: &[u8]) -> u16 {
    if frame.len() < 16 {
        return 0;
    }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != VLAN_ETH_P && tpid != ETHERTYPE_QINQ {
        return 0;
    }
    let tci = u16::from_be_bytes([frame[14], frame[15]]);
    tci & 0x0FFF
}

/// Insert an 802.1Q tag into a frame, returning the result in a fixed
/// 1518-byte output buffer. Returns the new frame length on success, or 0.
pub fn tag_frame_static(frame: &[u8], vid: u16, pcp: u8, dei: bool, out: &mut [u8; 1518]) -> usize {
    if frame.len() < 14 || frame.len() > 1514 {
        return 0;
    }
    let tci = vlan_tci_make(pcp, dei, vid);
    let new_len = frame.len().saturating_add(4);
    if new_len > 1518 {
        return 0;
    }

    let mut k = 0;
    while k < 12 {
        out[k] = frame[k];
        k += 1;
    }
    out[12] = (VLAN_ETH_P >> 8) as u8;
    out[13] = (VLAN_ETH_P & 0xFF) as u8;
    out[14] = (tci >> 8) as u8;
    out[15] = (tci & 0xFF) as u8;
    let rest = frame.len() - 12;
    let mut m = 0;
    while m < rest {
        out[16 + m] = frame[12 + m];
        m += 1;
    }
    new_len
}

/// Thin wrapper kept for callers that used the old `is_tagged()` name.
pub fn is_tagged(frame: &[u8]) -> bool {
    vlan_is_tagged(frame, frame.len())
}

/// Extract the VLAN ID from a tagged frame (returns `None` when untagged).
pub fn get_vid(frame: &[u8]) -> Option<u16> {
    let vid = vlan_get_id(frame);
    if vid == 0 {
        None
    } else {
        Some(vid)
    }
}

/// Strip a 4-byte 802.1Q tag from `frame`, writing the result into `out`.
/// Returns the new length, or `None` if the frame is too short.
fn strip_tag(frame: &[u8], out: &mut [u8; 1518]) -> Option<usize> {
    if frame.len() < 18 {
        return None;
    }
    let new_len = frame.len().saturating_sub(4);
    if new_len < 14 {
        return None;
    }
    let mut k = 0;
    while k < 12 {
        out[k] = frame[k];
        k += 1;
    }
    let tail = frame.len() - 16;
    let copy = if tail < (1518 - 12) { tail } else { 1518 - 12 };
    let mut m = 0;
    while m < copy {
        out[12 + m] = frame[16 + m];
        m += 1;
    }
    Some(new_len)
}

/// Apply ingress VLAN policy for the given physical interface.
///
/// Returns `(vid, untagged_frame_len)` into `out_buf`, or `None` to drop.
pub fn ingress_process_static(
    iface_id: u32,
    frame: &[u8],
    out_buf: &mut [u8; 1518],
) -> Option<(u16, usize)> {
    let cfg = get_port_config(iface_id)?;
    let tagged = is_tagged(frame);
    match cfg.mode {
        PortMode::Access => {
            if tagged {
                let vid = vlan_get_id(frame);
                if vid != cfg.native_vlan {
                    return None;
                }
                let untagged_len = strip_tag(frame, out_buf)?;
                Some((cfg.native_vlan, untagged_len))
            } else {
                let copy_len = if frame.len() < 1518 {
                    frame.len()
                } else {
                    1518
                };
                let mut k = 0;
                while k < copy_len {
                    out_buf[k] = frame[k];
                    k += 1;
                }
                Some((cfg.native_vlan, copy_len))
            }
        }
        PortMode::Trunk | PortMode::Hybrid => {
            if tagged {
                let vid = vlan_get_id(frame);
                if !cfg.is_vlan_allowed(vid) {
                    return None;
                }
                let untagged_len = strip_tag(frame, out_buf)?;
                Some((vid, untagged_len))
            } else {
                let copy_len = if frame.len() < 1518 {
                    frame.len()
                } else {
                    1518
                };
                let mut k = 0;
                while k < copy_len {
                    out_buf[k] = frame[k];
                    k += 1;
                }
                Some((cfg.native_vlan, copy_len))
            }
        }
    }
}

/// Apply egress VLAN policy for the given physical interface.
///
/// Returns the new frame length written into `out_buf`, or 0 to drop.
pub fn egress_process_static(
    iface_id: u32,
    vid: u16,
    frame: &[u8],
    out_buf: &mut [u8; 1518],
) -> usize {
    let cfg = match get_port_config(iface_id) {
        Some(c) => c,
        None => return 0,
    };
    if !cfg.is_vlan_allowed(vid) {
        return 0;
    }
    match cfg.mode {
        PortMode::Access => {
            if vid != cfg.native_vlan {
                return 0;
            }
            let copy_len = if frame.len() < 1518 {
                frame.len()
            } else {
                1518
            };
            let mut k = 0;
            while k < copy_len {
                out_buf[k] = frame[k];
                k += 1;
            }
            copy_len
        }
        PortMode::Trunk | PortMode::Hybrid => {
            if vid == cfg.native_vlan {
                let copy_len = if frame.len() < 1518 {
                    frame.len()
                } else {
                    1518
                };
                let mut k = 0;
                while k < copy_len {
                    out_buf[k] = frame[k];
                    k += 1;
                }
                copy_len
            } else {
                vlan_output(vid, frame, frame.len(), out_buf)
            }
        }
    }
}

/// Process an incoming 802.1Q-tagged Ethernet frame (legacy entry point).
///
/// Looks up the matching VLAN interface, strips the tag, and dispatches
/// the de-tagged frame through the network stack via `crate::net::process_frame`.
pub fn vlan_input(frame: &[u8], len: usize) {
    if len < 18 {
        return;
    }
    let tpid = u16::from_be_bytes([frame[12], frame[13]]);
    if tpid != VLAN_ETH_P && tpid != ETHERTYPE_QINQ {
        return;
    }
    let tci = u16::from_be_bytes([frame[14], frame[15]]);
    let vid = tci & 0x0FFF;

    {
        let devs = VLAN_DEVICES.lock();
        let mut found = false;
        let mut i = 0;
        while i < VLAN_MAX_DEVICES {
            if devs[i].active && devs[i].vid == vid {
                found = true;
                break;
            }
            i += 1;
        }
        if !found {
            serial_println!("[vlan] no interface for VID {} — dropping frame", vid);
            return;
        }
    }

    let untagged_len = len.saturating_sub(4);
    if untagged_len < 14 {
        return;
    }
    let mut untagged = [0u8; 1518];
    let mut k = 0;
    while k < 12 {
        untagged[k] = frame[k];
        k += 1;
    }
    let tail = len - 16;
    let copy = if tail < (1518 - 12) { tail } else { 1518 - 12 };
    let mut m = 0;
    while m < copy {
        untagged[12 + m] = frame[16 + m];
        m += 1;
    }
    crate::net::process_frame(&untagged[..untagged_len]);
}
