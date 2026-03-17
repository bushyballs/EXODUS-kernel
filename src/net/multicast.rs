use crate::sync::Mutex;
/// IP multicast — IGMP and group management
///
/// Manages multicast group membership and IGMP protocol for
/// efficient one-to-many delivery. Supports IGMPv2 with
/// join/leave, query/report, and multicast forwarding.
///
/// Inspired by: Linux IGMP (net/ipv4/igmp.c), RFC 2236 (IGMPv2).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IGMP protocol number
const PROTO_IGMP: u8 = 2;

/// IGMP message types
const IGMP_MEMBERSHIP_QUERY: u8 = 0x11;
const IGMP_V1_MEMBERSHIP_REPORT: u8 = 0x12;
const IGMP_V2_MEMBERSHIP_REPORT: u8 = 0x16;
const IGMP_LEAVE_GROUP: u8 = 0x17;

/// All-hosts multicast address (224.0.0.1)
const ALL_HOSTS: [u8; 4] = [224, 0, 0, 1];

/// All-routers multicast address (224.0.0.2)
const ALL_ROUTERS: [u8; 4] = [224, 0, 0, 2];

/// Maximum multicast groups per interface
const MAX_GROUPS_PER_IFACE: usize = 128;

/// Maximum interfaces tracked
const MAX_INTERFACES: usize = 32;

/// Default query interval (ticks)
const QUERY_INTERVAL: u64 = 1250; // ~125 seconds

/// Default max response time (tenths of seconds, from IGMP query)
const MAX_RESPONSE_TIME: u8 = 100; // 10 seconds

/// Group membership timeout (ticks)
const GROUP_MEMBERSHIP_TIMEOUT: u64 = 2600; // ~260 seconds

/// Unsolicited report interval (ticks)
const UNSOLICITED_REPORT_INTERVAL: u64 = 100; // ~10 seconds

// ---------------------------------------------------------------------------
// Multicast address helpers
// ---------------------------------------------------------------------------

/// Check if an IP address is a multicast address (224.0.0.0/4)
pub fn is_multicast(addr: &[u8; 4]) -> bool {
    addr[0] >= 224 && addr[0] <= 239
}

/// Check if a multicast address is link-local (224.0.0.0/24)
pub fn is_link_local_multicast(addr: &[u8; 4]) -> bool {
    addr[0] == 224 && addr[1] == 0 && addr[2] == 0
}

/// Convert multicast IP to multicast MAC (01:00:5E:XX:XX:XX)
pub fn multicast_ip_to_mac(ip: &[u8; 4]) -> [u8; 6] {
    [0x01, 0x00, 0x5E, ip[1] & 0x7F, ip[2], ip[3]]
}

// ---------------------------------------------------------------------------
// IGMP message format
// ---------------------------------------------------------------------------

/// IGMP message (8 bytes)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IgmpMessage {
    pub msg_type: u8,
    pub max_resp_time: u8,
    pub checksum: u16,
    pub group_addr: [u8; 4],
}

impl IgmpMessage {
    /// Build a membership report
    pub fn membership_report(group: [u8; 4]) -> Self {
        let mut msg = IgmpMessage {
            msg_type: IGMP_V2_MEMBERSHIP_REPORT,
            max_resp_time: 0,
            checksum: 0,
            group_addr: group,
        };
        msg.checksum = Self::compute_checksum(&msg);
        msg
    }

    /// Build a leave group message
    pub fn leave_group(group: [u8; 4]) -> Self {
        let mut msg = IgmpMessage {
            msg_type: IGMP_LEAVE_GROUP,
            max_resp_time: 0,
            checksum: 0,
            group_addr: group,
        };
        msg.checksum = Self::compute_checksum(&msg);
        msg
    }

    /// Build a general membership query
    pub fn general_query(max_resp_time: u8) -> Self {
        let mut msg = IgmpMessage {
            msg_type: IGMP_MEMBERSHIP_QUERY,
            max_resp_time,
            checksum: 0,
            group_addr: [0; 4],
        };
        msg.checksum = Self::compute_checksum(&msg);
        msg
    }

    /// Build a group-specific query
    pub fn group_query(group: [u8; 4], max_resp_time: u8) -> Self {
        let mut msg = IgmpMessage {
            msg_type: IGMP_MEMBERSHIP_QUERY,
            max_resp_time,
            checksum: 0,
            group_addr: group,
        };
        msg.checksum = Self::compute_checksum(&msg);
        msg
    }

    /// Serialize to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0] = self.msg_type;
        buf[1] = self.max_resp_time;
        buf[2..4].copy_from_slice(&self.checksum.to_be_bytes());
        buf[4..8].copy_from_slice(&self.group_addr);
        buf
    }

    /// Parse from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        Some(IgmpMessage {
            msg_type: data[0],
            max_resp_time: data[1],
            checksum: u16::from_be_bytes([data[2], data[3]]),
            group_addr: [data[4], data[5], data[6], data[7]],
        })
    }

    /// Compute IGMP checksum
    fn compute_checksum(msg: &IgmpMessage) -> u16 {
        let bytes = msg.to_bytes();
        let mut sum: u32 = 0;
        // Set checksum field to 0 for computation
        sum += u16::from_be_bytes([bytes[0], bytes[1]]) as u32;
        // Skip checksum bytes [2..4]
        sum += u16::from_be_bytes([bytes[4], bytes[5]]) as u32;
        sum += u16::from_be_bytes([bytes[6], bytes[7]]) as u32;
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        !(sum as u16)
    }

    /// Verify checksum
    pub fn verify_checksum(&self) -> bool {
        let bytes = self.to_bytes();
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 1 < bytes.len() {
            sum += u16::from_be_bytes([bytes[i], bytes[i + 1]]) as u32;
            i = i.saturating_add(2);
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        sum as u16 == 0xFFFF
    }
}

// ---------------------------------------------------------------------------
// Group membership
// ---------------------------------------------------------------------------

/// Multicast group membership state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    /// Idle member (not queried recently)
    IdleMember,
    /// Delaying member (timer running, will send report)
    DelayingMember,
    /// Non-member (not joined)
    NonMember,
}

/// A single multicast group entry
#[derive(Debug, Clone)]
pub struct MulticastGroupEntry {
    /// Group address
    pub group_addr: [u8; 4],
    /// Interface index
    pub iface_index: u32,
    /// Membership state
    pub state: GroupState,
    /// Timer value (tick when report should be sent)
    pub timer: u64,
    /// Last report tick
    pub last_report: u64,
    /// Number of members (reference count)
    pub ref_count: u32,
    /// Whether we are the querier on this interface
    pub is_querier: bool,
}

/// Multicast forwarding entry
#[derive(Debug, Clone)]
pub struct McastForwardEntry {
    pub group_addr: [u8; 4],
    pub src_addr: [u8; 4],
    /// Incoming interface
    pub iface_in: u32,
    /// Outgoing interfaces (bitmask)
    pub oif_list: Vec<u32>,
    /// Last activity tick
    pub last_used: u64,
}

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

struct MulticastInner {
    /// Per-interface group memberships
    groups: Vec<MulticastGroupEntry>,
    /// Multicast forwarding cache
    mfc: Vec<McastForwardEntry>,
    /// Tick counter
    tick: u64,
    /// Whether multicast routing is enabled
    routing_enabled: bool,
    /// Whether we are a querier
    is_querier: bool,
    /// Query interval
    query_interval: u64,
    /// Last query sent tick
    last_query_tick: u64,
}

static MULTICAST: Mutex<Option<MulticastInner>> = Mutex::new(None);

/// Initialize the multicast/IGMP subsystem
pub fn init() {
    *MULTICAST.lock() = Some(MulticastInner {
        groups: Vec::new(),
        mfc: Vec::new(),
        tick: 0,
        routing_enabled: false,
        is_querier: false,
        query_interval: QUERY_INTERVAL,
        last_query_tick: 0,
    });
    serial_println!("  Net: multicast/IGMP subsystem initialized");
}

// ---------------------------------------------------------------------------
// Group management
// ---------------------------------------------------------------------------

/// Join a multicast group on an interface
pub fn join_group(group_addr: [u8; 4], iface_index: u32) -> Result<(), McastError> {
    if !is_multicast(&group_addr) {
        return Err(McastError::InvalidAddress);
    }

    let mut guard = MULTICAST.lock();
    let inner = guard.as_mut().ok_or(McastError::NotInitialized)?;

    // Check if already joined
    if let Some(entry) = inner
        .groups
        .iter_mut()
        .find(|g| g.group_addr == group_addr && g.iface_index == iface_index)
    {
        entry.ref_count = entry.ref_count.saturating_add(1);
        return Ok(());
    }

    // Check limits
    let iface_count = inner
        .groups
        .iter()
        .filter(|g| g.iface_index == iface_index)
        .count();
    if iface_count >= MAX_GROUPS_PER_IFACE {
        return Err(McastError::TooManyGroups);
    }

    inner.groups.push(MulticastGroupEntry {
        group_addr,
        iface_index,
        state: GroupState::DelayingMember,
        timer: inner.tick + UNSOLICITED_REPORT_INTERVAL,
        last_report: 0,
        ref_count: 1,
        is_querier: false,
    });

    serial_println!(
        "  IGMP: joined {}.{}.{}.{} on iface {}",
        group_addr[0],
        group_addr[1],
        group_addr[2],
        group_addr[3],
        iface_index
    );

    Ok(())
}

/// Leave a multicast group on an interface
pub fn leave_group(group_addr: [u8; 4], iface_index: u32) -> Result<(), McastError> {
    let mut guard = MULTICAST.lock();
    let inner = guard.as_mut().ok_or(McastError::NotInitialized)?;

    let idx = inner
        .groups
        .iter()
        .position(|g| g.group_addr == group_addr && g.iface_index == iface_index)
        .ok_or(McastError::NotMember)?;

    inner.groups[idx].ref_count = inner.groups[idx].ref_count.saturating_sub(1);
    if inner.groups[idx].ref_count == 0 {
        inner.groups.remove(idx);
        serial_println!(
            "  IGMP: left {}.{}.{}.{} on iface {}",
            group_addr[0],
            group_addr[1],
            group_addr[2],
            group_addr[3],
            iface_index
        );
    }

    Ok(())
}

/// Check if we are a member of a group on an interface
pub fn is_member(group_addr: &[u8; 4], iface_index: u32) -> bool {
    let guard = MULTICAST.lock();
    match guard.as_ref() {
        Some(inner) => inner
            .groups
            .iter()
            .any(|g| g.group_addr == *group_addr && g.iface_index == iface_index),
        None => false,
    }
}

/// List all group memberships
pub fn list_groups() -> Vec<(u32, [u8; 4], u32)> {
    let guard = MULTICAST.lock();
    match guard.as_ref() {
        Some(inner) => inner
            .groups
            .iter()
            .map(|g| (g.iface_index, g.group_addr, g.ref_count))
            .collect(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// IGMP packet processing
// ---------------------------------------------------------------------------

/// Process an incoming IGMP message
pub fn process_igmp(src_ip: [u8; 4], iface_index: u32, data: &[u8]) -> Option<Vec<u8>> {
    let msg = IgmpMessage::from_bytes(data)?;

    let mut guard = MULTICAST.lock();
    let inner = guard.as_mut()?;
    inner.tick = inner.tick.saturating_add(1);

    match msg.msg_type {
        IGMP_MEMBERSHIP_QUERY => {
            process_query(inner, &msg, iface_index);
            None
        }
        IGMP_V2_MEMBERSHIP_REPORT | IGMP_V1_MEMBERSHIP_REPORT => {
            process_report(inner, &msg, src_ip, iface_index);
            None
        }
        IGMP_LEAVE_GROUP => {
            process_leave(inner, &msg, iface_index);
            None
        }
        _ => None,
    }
}

/// Handle a membership query
fn process_query(inner: &mut MulticastInner, msg: &IgmpMessage, iface_index: u32) {
    let max_resp = if msg.max_resp_time == 0 {
        100
    } else {
        msg.max_resp_time as u64
    };
    let max_resp_ticks = max_resp; // approximately 1:1 mapping

    if msg.group_addr == [0; 4] {
        // General query: set timers on all groups for this interface
        for group in &mut inner.groups {
            if group.iface_index == iface_index {
                group.state = GroupState::DelayingMember;
                // Random delay: use simple tick-based pseudo-random
                let delay = (inner.tick % max_resp_ticks) + 1;
                group.timer = inner.tick + delay;
            }
        }
        serial_println!("  IGMP: general query on iface {}", iface_index);
    } else {
        // Group-specific query
        if let Some(group) = inner
            .groups
            .iter_mut()
            .find(|g| g.group_addr == msg.group_addr && g.iface_index == iface_index)
        {
            group.state = GroupState::DelayingMember;
            let delay = (inner.tick % max_resp_ticks) + 1;
            group.timer = inner.tick + delay;
        }
    }
}

/// Handle a membership report from another host
fn process_report(
    inner: &mut MulticastInner,
    msg: &IgmpMessage,
    _src_ip: [u8; 4],
    iface_index: u32,
) {
    // If another host reported the same group, cancel our pending report
    for group in &mut inner.groups {
        if group.group_addr == msg.group_addr && group.iface_index == iface_index {
            if group.state == GroupState::DelayingMember {
                group.state = GroupState::IdleMember;
            }
        }
    }
}

/// Handle a leave message (for querier)
fn process_leave(_inner: &mut MulticastInner, msg: &IgmpMessage, iface_index: u32) {
    serial_println!(
        "  IGMP: leave {}.{}.{}.{} on iface {}",
        msg.group_addr[0],
        msg.group_addr[1],
        msg.group_addr[2],
        msg.group_addr[3],
        iface_index
    );
    // As querier, we would send a group-specific query
    // For simplicity, we just note it
}

/// Build an IGMP membership report to send
pub fn build_report(group_addr: [u8; 4]) -> Vec<u8> {
    IgmpMessage::membership_report(group_addr)
        .to_bytes()
        .to_vec()
}

/// Build an IGMP leave message to send
pub fn build_leave(group_addr: [u8; 4]) -> Vec<u8> {
    IgmpMessage::leave_group(group_addr).to_bytes().to_vec()
}

// ---------------------------------------------------------------------------
// Timer processing
// ---------------------------------------------------------------------------

/// Timer tick — call periodically to send pending reports
///
/// Returns a list of (iface_index, group_addr, igmp_bytes) to send
pub fn timer_tick() -> Vec<(u32, [u8; 4], Vec<u8>)> {
    let mut guard = MULTICAST.lock();
    let inner = match guard.as_mut() {
        Some(i) => i,
        None => return Vec::new(),
    };
    inner.tick = inner.tick.saturating_add(1);
    let tick = inner.tick;

    let mut reports = Vec::new();

    for group in &mut inner.groups {
        if group.state == GroupState::DelayingMember && tick >= group.timer {
            // Send membership report
            let report = IgmpMessage::membership_report(group.group_addr);
            reports.push((
                group.iface_index,
                group.group_addr,
                report.to_bytes().to_vec(),
            ));
            group.state = GroupState::IdleMember;
            group.last_report = tick;
        }
    }

    // Expire forwarding cache entries
    inner
        .mfc
        .retain(|e| tick.wrapping_sub(e.last_used) < GROUP_MEMBERSHIP_TIMEOUT);

    reports
}

// ---------------------------------------------------------------------------
// Multicast forwarding
// ---------------------------------------------------------------------------

/// Add a multicast forwarding cache entry
pub fn add_mfc_entry(group_addr: [u8; 4], src_addr: [u8; 4], iface_in: u32, oif_list: Vec<u32>) {
    let mut guard = MULTICAST.lock();
    if let Some(inner) = guard.as_mut() {
        // Remove existing entry
        inner
            .mfc
            .retain(|e| !(e.group_addr == group_addr && e.src_addr == src_addr));
        inner.mfc.push(McastForwardEntry {
            group_addr,
            src_addr,
            iface_in,
            oif_list,
            last_used: inner.tick,
        });
    }
}

/// Look up forwarding interfaces for a multicast packet
pub fn mfc_lookup(group_addr: &[u8; 4], src_addr: &[u8; 4], iface_in: u32) -> Vec<u32> {
    let mut guard = MULTICAST.lock();
    let inner = match guard.as_mut() {
        Some(i) => i,
        None => return Vec::new(),
    };

    if let Some(entry) = inner
        .mfc
        .iter_mut()
        .find(|e| e.group_addr == *group_addr && e.src_addr == *src_addr && e.iface_in == iface_in)
    {
        entry.last_used = inner.tick;
        entry.oif_list.clone()
    } else {
        Vec::new()
    }
}

/// Enable/disable multicast routing
pub fn set_routing_enabled(enabled: bool) {
    let mut guard = MULTICAST.lock();
    if let Some(inner) = guard.as_mut() {
        inner.routing_enabled = enabled;
        serial_println!(
            "  IGMP: multicast routing {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McastError {
    NotInitialized,
    InvalidAddress,
    NotMember,
    TooManyGroups,
    InterfaceNotFound,
}
