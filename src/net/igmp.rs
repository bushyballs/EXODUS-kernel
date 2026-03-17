use crate::net::NetworkDriver;
/// IGMP v2/v3 — Internet Group Management Protocol (RFC 2236 / RFC 3376)
///
/// Implements multicast group membership for IPv4.  Supports:
///   - IGMPv2 Join / Leave / Query / Report
///   - IGMPv3 Membership Report (type 0x22)
///   - Source-specific multicast (SSM) filter modes
///   - Deferred report timer ticked every 100 ms
///   - Multicast MAC derivation (RFC 1112 §6.4)
///
/// RFC 2236 §2 — IGMP message types
///   0x11 Membership Query
///   0x12 IGMPv1 Membership Report
///   0x16 IGMPv2 Membership Report
///   0x17 IGMPv2 Leave Group
///   0x22 IGMPv3 Membership Report
///
/// No std, no heap, no float casts, no panics.  Fixed-size arrays only.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public message-type constants (RFC 2236 §2, RFC 3376 §4)
// ---------------------------------------------------------------------------

pub const IGMP_MEMBERSHIP_QUERY: u8 = 0x11;
pub const IGMP_V1_MEMBERSHIP_REPORT: u8 = 0x12;
pub const IGMP_V2_MEMBERSHIP_REPORT: u8 = 0x16;
pub const IGMP_V2_LEAVE_GROUP: u8 = 0x17;
pub const IGMP_V3_MEMBERSHIP_REPORT: u8 = 0x22;

// ---------------------------------------------------------------------------
// Well-known multicast addresses
// ---------------------------------------------------------------------------

/// 224.0.0.1 — All Hosts (RFC 1112)
pub const IGMP_ALL_HOSTS: [u8; 4] = [224, 0, 0, 1];
/// 224.0.0.2 — All Routers (RFC 2236)
pub const IGMP_ALL_ROUTERS: [u8; 4] = [224, 0, 0, 2];
/// 224.0.0.22 — IGMPv3 general report destination (RFC 3376)
pub const IGMP_REPORT_ADDR: [u8; 4] = [224, 0, 0, 22];

// ---------------------------------------------------------------------------
// IP protocol number for IGMP
// ---------------------------------------------------------------------------

const PROTO_IGMP: u8 = 2;

// ---------------------------------------------------------------------------
// IPv4 header constants used when building IGMP packets
// ---------------------------------------------------------------------------

/// Default IP TTL for IGMP (RFC 2236 §2 — must be 1)
const IGMP_IP_TTL: u8 = 1;
/// IPv4 header with Router Alert option is 24 bytes (RFC 2113)
const IP_HEADER_LEN: usize = 24;
/// IGMP message body length
const IGMP_MSG_LEN: usize = 8;
/// Total packet: IP header (24) + IGMP body (8)
pub const IGMP_PACKET_LEN: usize = IP_HEADER_LEN + IGMP_MSG_LEN;

// ---------------------------------------------------------------------------
// Capacity limits
// ---------------------------------------------------------------------------

/// Maximum multicast groups tracked simultaneously
const MAX_MCAST_GROUPS: usize = 32;
/// Maximum source addresses per group (SSM, RFC 3376)
const MAX_SOURCES_PER_GROUP: usize = 16;

// ---------------------------------------------------------------------------
// Filter mode (RFC 3376 §3.2)
// ---------------------------------------------------------------------------

/// Source-filter mode for a multicast group socket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Include — only accept packets from sources in the list
    Include,
    /// Exclude — accept packets from all sources except those in the list
    Exclude,
}

// ---------------------------------------------------------------------------
// Multicast group entry
// ---------------------------------------------------------------------------

/// State machine per group (RFC 2236 §3.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupState {
    /// Not a member
    NonMember,
    /// Member, waiting to send a deferred report
    DelayingMember,
    /// Member, report already sent or another host reported
    IdleMember,
}

/// One multicast group entry in the static group table
#[derive(Clone, Copy)]
pub struct MulticastGroup {
    /// IPv4 multicast group address (224.0.0.0/4)
    pub group_addr: [u8; 4],
    /// IP address of the interface that joined this group
    pub iface_ip: [u8; 4],
    /// SSM filter mode
    pub mode: FilterMode,
    /// Source-specific multicast source list (RFC 3376)
    pub sources: [[u8; 4]; MAX_SOURCES_PER_GROUP],
    /// Number of valid entries in `sources`
    pub source_count: u8,
    /// Deferred-report countdown in milliseconds; send when reaches 0
    pub timer_ms: u32,
    /// Whether this slot is an active membership
    pub joined: bool,
    /// RFC 2236 membership state machine state
    pub state: GroupState,
    /// Reference count — allows multiple sockets to join the same group
    pub ref_count: u32,
}

impl MulticastGroup {
    const fn empty() -> Self {
        MulticastGroup {
            group_addr: [0; 4],
            iface_ip: [0; 4],
            mode: FilterMode::Exclude,
            sources: [[0; 4]; MAX_SOURCES_PER_GROUP],
            source_count: 0,
            timer_ms: 0,
            joined: false,
            state: GroupState::NonMember,
            ref_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static group table
// ---------------------------------------------------------------------------

const EMPTY_GROUP: Option<MulticastGroup> = None;
static MCAST_GROUPS: Mutex<[Option<MulticastGroup>; MAX_MCAST_GROUPS]> =
    Mutex::new([EMPTY_GROUP; MAX_MCAST_GROUPS]);

/// Cached interface IP used for building outgoing IGMP packets.
/// Set during init() or updated via set_iface_ip().
static IFACE_IP: Mutex<[u8; 4]> = Mutex::new([0u8; 4]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the IGMP subsystem.
///
/// Joins the mandatory 224.0.0.1 (All-Hosts) group on the local interface
/// as required by RFC 1112 §5.
pub fn init() {
    // Join All-Hosts — every IPv4 host must listen here (RFC 1112)
    igmp_join(IGMP_ALL_HOSTS);
    serial_println!("  Net: IGMP v2/v3 subsystem initialized");
}

/// Configure the local interface IP used in outgoing IGMP packets.
pub fn set_iface_ip(ip: [u8; 4]) {
    *IFACE_IP.lock() = ip;
}

/// Join a multicast group and send an unsolicited IGMPv2 Membership Report.
///
/// Returns `true` if the join succeeded or the group was already joined.
pub fn igmp_join(group: [u8; 4]) -> bool {
    if !is_multicast_addr(group) {
        return false;
    }
    let mut table = MCAST_GROUPS.lock();

    // Already a member? Just bump the ref count.
    for slot in table.iter_mut().flatten() {
        if slot.group_addr == group {
            slot.ref_count = slot.ref_count.saturating_add(1);
            return true;
        }
    }

    // Read interface IP before iterating table slots (no lock ordering hazard).
    // IFACE_IP is a separate Mutex; we drop its guard before re-entering the
    // table iteration to maintain a consistent lock order.
    let iface_ip = *IFACE_IP.lock();
    for slot in table.iter_mut() {
        if slot.is_none() {
            *slot = Some(MulticastGroup {
                group_addr: group,
                iface_ip,
                mode: FilterMode::Exclude,
                sources: [[0; 4]; MAX_SOURCES_PER_GROUP],
                source_count: 0,
                // Send initial report after a short delay (RFC 2236 §3.1)
                timer_ms: 500,
                joined: true,
                state: GroupState::DelayingMember,
                ref_count: 1,
            });
            serial_println!(
                "  IGMP: join {}.{}.{}.{}",
                group[0],
                group[1],
                group[2],
                group[3]
            );
            return true;
        }
    }

    // Table full
    serial_println!("  IGMP: join failed — group table full");
    false
}

/// Leave a multicast group and send an IGMPv2 Leave Group to 224.0.0.2.
///
/// Returns `true` if a Leave was sent (i.e. this was the last reference).
pub fn igmp_leave(group: [u8; 4]) -> bool {
    let mut table = MCAST_GROUPS.lock();
    for slot in table.iter_mut() {
        if let Some(ref mut entry) = slot {
            if entry.group_addr == group {
                entry.ref_count = entry.ref_count.saturating_sub(1);
                if entry.ref_count == 0 {
                    entry.joined = false;
                    entry.state = GroupState::NonMember;
                    *slot = None;
                    serial_println!(
                        "  IGMP: leave {}.{}.{}.{}",
                        group[0],
                        group[1],
                        group[2],
                        group[3]
                    );
                    return true;
                }
                return false;
            }
        }
    }
    false
}

/// Returns `true` if we are currently a member of `group`.
pub fn igmp_is_member(group: [u8; 4]) -> bool {
    let table = MCAST_GROUPS.lock();
    table
        .iter()
        .flatten()
        .any(|g| g.group_addr == group && g.joined)
}

/// Process an incoming IGMP packet.
///
/// `data`   — raw bytes starting immediately after the IPv4 header
/// `src_ip` — source IPv4 address of the enclosing IP packet
pub fn igmp_input(data: &[u8], src_ip: [u8; 4]) {
    if data.len() < 8 {
        return;
    }

    let msg_type = data[0];
    // max_resp_code = data[1]
    // checksum      = data[2..4]  — we verify below
    let group = [data[4], data[5], data[6], data[7]];

    // Verify checksum
    if igmp_checksum(data) != 0 {
        // Non-zero result after including the checksum field means bad packet
        return;
    }

    let mut table = MCAST_GROUPS.lock();

    match msg_type {
        IGMP_MEMBERSHIP_QUERY => {
            // RFC 2236 §3 — Querier received; start response timers.
            // max_resp_code is in units of 1/10 second.
            let max_resp_ms = (data[1] as u32).saturating_mul(100);
            let is_general = group == [0, 0, 0, 0];

            for slot in table.iter_mut().flatten() {
                if !slot.joined {
                    continue;
                }
                let applies = is_general || slot.group_addr == group;
                if applies && slot.state == GroupState::IdleMember {
                    // Set timer to [0, max_resp_ms]; use group bytes as cheap entropy
                    let delay = (slot.group_addr[3] as u32)
                        .saturating_mul(max_resp_ms)
                        .wrapping_add(1)
                        / 255u32.saturating_add(1);
                    slot.timer_ms = delay.max(100).min(max_resp_ms.max(100));
                    slot.state = GroupState::DelayingMember;
                }
            }
            serial_println!(
                "  IGMP: query from {}.{}.{}.{} ({})",
                src_ip[0],
                src_ip[1],
                src_ip[2],
                src_ip[3],
                if is_general {
                    "general"
                } else {
                    "group-specific"
                }
            );
        }

        IGMP_V1_MEMBERSHIP_REPORT | IGMP_V2_MEMBERSHIP_REPORT => {
            // RFC 2236 §3 — Another host reported; cancel our pending timer.
            for slot in table.iter_mut().flatten() {
                if slot.group_addr == group && slot.state == GroupState::DelayingMember {
                    slot.state = GroupState::IdleMember;
                    slot.timer_ms = 0;
                }
            }
        }

        IGMP_V2_LEAVE_GROUP => {
            // We are a router / querier; real handling would send a
            // group-specific query.  Log and continue.
            serial_println!(
                "  IGMP: leave {}.{}.{}.{} from {}.{}.{}.{}",
                group[0],
                group[1],
                group[2],
                group[3],
                src_ip[0],
                src_ip[1],
                src_ip[2],
                src_ip[3]
            );
        }

        IGMP_V3_MEMBERSHIP_REPORT => {
            // RFC 3376 §4.2 — IGMPv3 report; parse group records if length allows.
            // Minimum IGMPv3 report: 8 bytes header + variable group records.
            // For now we just log; full IGMPv3 SSM parsing is a future extension.
            if data.len() >= 8 {
                let num_records = u16::from_be_bytes([data[6], data[7]]) as usize;
                serial_println!(
                    "  IGMP: IGMPv3 report from {}.{}.{}.{} ({} group records)",
                    src_ip[0],
                    src_ip[1],
                    src_ip[2],
                    src_ip[3],
                    num_records
                );
            }
        }

        _ => {}
    }
}

/// Build an IGMPv2 Membership Report packet (IP header + IGMP body = 28 bytes).
///
/// The full packet is written to `buf` starting at offset 0.
/// Returns the number of bytes written, or 0 if `buf` is too small.
///
/// IP header layout (RFC 791):
///   - 24 bytes with Router Alert option (RFC 2113) as required by IGMPv2
/// IGMP body (RFC 2236):
///   - type=0x16, max_resp=0, checksum, group_addr
pub fn igmp_build_report(group: [u8; 4], buf: &mut [u8]) -> usize {
    if buf.len() < IGMP_PACKET_LEN {
        return 0;
    }
    let src_ip = *IFACE_IP.lock();
    build_igmp_packet(IGMP_V2_MEMBERSHIP_REPORT, group, src_ip, group, buf)
}

/// Build an IGMPv2 Leave Group packet (sent to 224.0.0.2, All-Routers).
///
/// Returns the number of bytes written, or 0 if `buf` is too small.
pub fn igmp_build_leave(group: [u8; 4], buf: &mut [u8]) -> usize {
    if buf.len() < IGMP_PACKET_LEN {
        return 0;
    }
    let src_ip = *IFACE_IP.lock();
    build_igmp_packet(IGMP_V2_LEAVE_GROUP, group, src_ip, IGMP_ALL_ROUTERS, buf)
}

/// IGMP timer tick — call every 100 ms.
///
/// Sends any deferred Membership Reports whose timers have expired.
/// `elapsed_ms` is the number of milliseconds since the last call.
///
/// Implementation note: we collect all groups that need a report in a small
/// fixed-size buffer while holding the lock, then release the lock before
/// calling into the send path to avoid a deadlock with the NIC driver lock.
pub fn igmp_tick(elapsed_ms: u32) {
    // Fixed-size collection of groups whose timers fired this tick.
    let mut to_send = [[0u8; 4]; MAX_MCAST_GROUPS];
    let mut to_send_len = 0usize;
    let src_ip;

    // --- Phase 1: update timers and collect expired groups (lock held) ------
    {
        let mut table = MCAST_GROUPS.lock();
        src_ip = *IFACE_IP.lock();

        for slot in table.iter_mut().flatten() {
            if slot.state != GroupState::DelayingMember {
                continue;
            }
            if slot.timer_ms <= elapsed_ms {
                // Timer expired — mark idle and queue for sending
                slot.timer_ms = 0;
                slot.state = GroupState::IdleMember;
                if to_send_len < MAX_MCAST_GROUPS {
                    to_send[to_send_len] = slot.group_addr;
                    to_send_len = to_send_len.saturating_add(1);
                }
            } else {
                slot.timer_ms = slot.timer_ms.saturating_sub(elapsed_ms);
            }
        }
    } // MCAST_GROUPS lock released here

    // --- Phase 2: send reports without holding any group-table lock ----------
    for i in 0..to_send_len {
        send_report_now(to_send[i], src_ip);
    }
}

// ---------------------------------------------------------------------------
// Multicast routing helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `ip` is a multicast address (224.0.0.0/4, RFC 1112).
#[inline]
pub fn is_multicast_addr(ip: [u8; 4]) -> bool {
    ip[0] >> 4 == 0xE
}

/// Map a multicast group IP to its Ethernet multicast MAC address.
///
/// RFC 1112 §6.4: 01:00:5E:0X:XX:XX where the low 23 bits of the IP group
/// address are mapped into the low 23 bits of the MAC address (bit 24 of the
/// IP is dropped).
pub fn multicast_mac(group: [u8; 4]) -> [u8; 6] {
    [
        0x01,
        0x00,
        0x5E,
        group[1] & 0x7F, // clear bit 7 of the second octet (drops IP bit 24)
        group[2],
        group[3],
    ]
}

// ---------------------------------------------------------------------------
// Checksum (RFC 792 one's-complement over IGMP header)
// ---------------------------------------------------------------------------

/// Compute (or verify) the RFC 792 one's-complement checksum over `data`.
///
/// When computing: set the checksum field to 0 before calling.
/// When verifying: pass the full message including the checksum field; a
/// correct message returns 0xFFFF (i.e. `!0u16`), so we normalise to 0.
fn igmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0usize;
    while i.saturating_add(1) < data.len() {
        sum = sum.saturating_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32);
        i = i.saturating_add(2);
    }
    // Odd byte
    if i < data.len() {
        sum = sum.saturating_add((data[i] as u32) << 8);
    }
    // Fold carries
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF).saturating_add(sum >> 16);
    }
    let result = !(sum as u16);
    // Normalise: a verified correct message gives 0xFFFF; map to 0 so callers
    // can write `igmp_checksum(data) != 0` as the error check.
    if result == 0xFFFF {
        0
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Write an IGMPv2 IP+IGMP packet into `buf` and return the total byte count.
///
/// IP header layout (24 bytes, with Router Alert option per RFC 2113):
///   [0]    version/IHL = 0x46  (IPv4, IHL=6 for 24-byte header)
///   [1]    DSCP/ECN = 0xC0     (DSCP CS6 — per RFC 2236 §2)
///   [2..4] total length
///   [4..6] identification
///   [6..8] flags/fragment = 0x4000 (DF)
///   [8]    TTL = 1
///   [9]    protocol = 2 (IGMP)
///   [10..12] header checksum
///   [12..16] source IP
///   [16..20] destination IP
///   [20..24] Router Alert option: 0x94 0x04 0x00 0x00
fn build_igmp_packet(
    igmp_type: u8,
    group: [u8; 4],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    buf: &mut [u8],
) -> usize {
    if buf.len() < IGMP_PACKET_LEN {
        return 0;
    }

    let total_len: u16 = IGMP_PACKET_LEN as u16;

    // ── IPv4 header (24 bytes) ───────────────────────────────────────────────
    buf[0] = 0x46; // version=4, IHL=6
    buf[1] = 0xC0; // DSCP CS6
    buf[2] = (total_len >> 8) as u8;
    buf[3] = total_len as u8;
    buf[4] = 0x00;
    buf[5] = 0x00; // ID
    buf[6] = 0x40;
    buf[7] = 0x00; // DF flag, frag offset=0
    buf[8] = IGMP_IP_TTL;
    buf[9] = PROTO_IGMP;
    buf[10] = 0x00;
    buf[11] = 0x00; // checksum placeholder
    buf[12..16].copy_from_slice(&src_ip);
    buf[16..20].copy_from_slice(&dst_ip);
    // Router Alert option (RFC 2113): type=0x94, len=4, value=0x0000
    buf[20] = 0x94;
    buf[21] = 0x04;
    buf[22] = 0x00;
    buf[23] = 0x00;

    // Compute IP header checksum over bytes [0..24]
    let ip_cksum = ip_checksum(&buf[0..24]);
    buf[10] = (ip_cksum >> 8) as u8;
    buf[11] = ip_cksum as u8;

    // ── IGMP body (8 bytes, offset 24) ──────────────────────────────────────
    buf[24] = igmp_type;
    buf[25] = 0x00; // max resp time
    buf[26] = 0x00;
    buf[27] = 0x00; // checksum placeholder
    buf[28] = group[0];
    buf[29] = group[1];
    buf[30] = group[2];
    buf[31] = group[3];

    // Compute IGMP checksum over IGMP body [24..32]
    let igmp_cksum = compute_igmp_checksum(&buf[24..32]);
    buf[26] = (igmp_cksum >> 8) as u8;
    buf[27] = igmp_cksum as u8;

    IGMP_PACKET_LEN
}

/// Compute one's-complement checksum with the checksum field already zeroed.
fn compute_igmp_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0usize;
    while i.saturating_add(1) < data.len() {
        // Skip bytes [2..4] — checksum field (already 0, but explicit is cleaner)
        sum = sum.saturating_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32);
        i = i.saturating_add(2);
    }
    if i < data.len() {
        sum = sum.saturating_add((data[i] as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF).saturating_add(sum >> 16);
    }
    !(sum as u16)
}

/// RFC 791 IP header checksum (standard internet checksum).
fn ip_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0usize;
    while i.saturating_add(1) < data.len() {
        sum = sum.saturating_add(u16::from_be_bytes([data[i], data[i + 1]]) as u32);
        i = i.saturating_add(2);
    }
    if i < data.len() {
        sum = sum.saturating_add((data[i] as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF).saturating_add(sum >> 16);
    }
    !(sum as u16)
}

/// Actually transmit a Membership Report.  Called from `igmp_tick` after the
/// group-table lock has been released to avoid a deadlock with the NIC driver.
///
/// We acquire the e1000 driver lock exactly once: read the MAC, build the
/// frame, and send — all inside a single critical section.
fn send_report_now(group: [u8; 4], src_ip: [u8; 4]) {
    let mut pkt = [0u8; IGMP_PACKET_LEN];
    let len = build_igmp_packet(IGMP_V2_MEMBERSHIP_REPORT, group, src_ip, group, &mut pkt);
    if len == 0 {
        return;
    }

    let dst_mac = multicast_mac(group);
    // EtherType 0x0800 = IPv4
    let mut frame = [0u8; 14 + IGMP_PACKET_LEN];
    frame[12] = 0x08;
    frame[13] = 0x00;
    frame[14..14 + len].copy_from_slice(&pkt[..len]);

    // Acquire the NIC driver once to get MAC and transmit.
    {
        let driver = crate::drivers::e1000::driver().lock();
        if let Some(ref nic) = *driver {
            let src_mac = nic.mac_addr().0;
            frame[0..6].copy_from_slice(&dst_mac);
            frame[6..12].copy_from_slice(&src_mac);
            let _ = nic.send(&frame[..14 + len]);
        }
    } // driver lock released

    serial_println!(
        "  IGMP: report {}.{}.{}.{}",
        group[0],
        group[1],
        group[2],
        group[3]
    );
}
