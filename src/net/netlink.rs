/// Netlink socket protocol — kernel-userspace configuration interface
///
/// Fixed-size, no-heap implementation. No Vec, Box, String, or alloc::*.
/// Uses static arrays and ring buffers throughout.
///
/// Provides the netlink message bus for routing, firewall, and generic
/// kernel-userspace communication. Messages follow the TLV format.
/// Supports multicast groups and dump operations.
///
/// Inspired by: Linux netlink (RFC 3549), libnl. All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const AF_NETLINK: u16 = 16;
pub const NETLINK_FD_BASE: i32 = 11000;

/// Netlink protocol families
pub const NETLINK_ROUTE: u32 = 0;
pub const NETLINK_USERSOCK: u32 = 2;
pub const NETLINK_AUDIT: u32 = 9;
pub const NETLINK_KOBJECT_UEVENT: u32 = 15;
pub const NETLINK_GENERIC: u32 = 16;

/// Message types — standard
pub const NLMSG_NOOP: u16 = 1;
pub const NLMSG_ERROR: u16 = 2;
pub const NLMSG_DONE: u16 = 3;
pub const NLMSG_OVERRUN: u16 = 4;

/// Route-specific message types
pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_GETLINK: u16 = 18;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
pub const RTM_GETADDR: u16 = 22;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;
pub const RTM_GETROUTE: u16 = 26;

/// Message flags
pub const NLM_F_REQUEST: u16 = 1;
pub const NLM_F_MULTI: u16 = 2;
pub const NLM_F_ACK: u16 = 4;
pub const NLM_F_DUMP: u16 = 0x300;

/// Netlink message header size in bytes
const NL_HDR_SIZE: usize = 16;

/// Maximum netlink sockets
const MAX_NL_SOCKETS: usize = 16;

/// RX ring buffer size per socket (bytes)
const NL_RX_BUF: usize = 8192;

// ---------------------------------------------------------------------------
// Netlink message header — repr(C, packed) for wire format
// ---------------------------------------------------------------------------

/// Netlink message header (matches kernel struct nlmsghdr)
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct NlMsgHdr {
    pub nlmsg_len: u32,
    pub nlmsg_type: u16,
    pub nlmsg_flags: u16,
    pub nlmsg_seq: u32,
    pub nlmsg_pid: u32,
}

impl NlMsgHdr {
    /// Serialize to bytes (little-endian, as Linux expects)
    pub fn to_bytes(self) -> [u8; NL_HDR_SIZE] {
        let mut b = [0u8; NL_HDR_SIZE];
        let l = self.nlmsg_len.to_le_bytes();
        let t = self.nlmsg_type.to_le_bytes();
        let f = self.nlmsg_flags.to_le_bytes();
        let s = self.nlmsg_seq.to_le_bytes();
        let p = self.nlmsg_pid.to_le_bytes();
        b[0] = l[0];
        b[1] = l[1];
        b[2] = l[2];
        b[3] = l[3];
        b[4] = t[0];
        b[5] = t[1];
        b[6] = f[0];
        b[7] = f[1];
        b[8] = s[0];
        b[9] = s[1];
        b[10] = s[2];
        b[11] = s[3];
        b[12] = p[0];
        b[13] = p[1];
        b[14] = p[2];
        b[15] = p[3];
        b
    }

    /// Parse from bytes (little-endian)
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < NL_HDR_SIZE {
            return None;
        }
        Some(NlMsgHdr {
            nlmsg_len: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            nlmsg_type: u16::from_le_bytes([b[4], b[5]]),
            nlmsg_flags: u16::from_le_bytes([b[6], b[7]]),
            nlmsg_seq: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            nlmsg_pid: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        })
    }
}

// ---------------------------------------------------------------------------
// Netlink socket record — fixed-size, no heap
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct NetlinkSocket {
    pub fd: i32,
    pub nl_proto: u32,
    pub pid: u32,
    pub groups: u32,
    /// Receive ring buffer (raw bytes, variable-length messages concatenated)
    pub rx_buf: [u8; NL_RX_BUF],
    /// Write head (bytes enqueued, wraps at NL_RX_BUF)
    pub rx_head: u32,
    /// Read tail (bytes consumed, wraps at NL_RX_BUF)
    pub rx_tail: u32,
    pub active: bool,
}

impl NetlinkSocket {
    pub const fn empty() -> Self {
        NetlinkSocket {
            fd: -1,
            nl_proto: 0,
            pid: 0,
            groups: 0,
            rx_buf: [0u8; NL_RX_BUF],
            rx_head: 0,
            rx_tail: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NL_SOCKETS: Mutex<[NetlinkSocket; MAX_NL_SOCKETS]> =
    Mutex::new([NetlinkSocket::empty(); MAX_NL_SOCKETS]);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "  Net: Netlink subsystem initialized (no-heap, {} sockets)",
        MAX_NL_SOCKETS
    );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_by_fd(socks: &[NetlinkSocket; MAX_NL_SOCKETS], fd: i32) -> Option<usize> {
    let mut i = 0;
    while i < MAX_NL_SOCKETS {
        if socks[i].active && socks[i].fd == fd {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Bytes available in a socket's ring buffer (linear data at rx_tail..rx_head).
/// This implementation uses a simple linear fill model: rx_tail is the read cursor,
/// rx_head is the write cursor. When rx_head wraps, it resets to 0 along with rx_tail.
fn rx_available(sock: &NetlinkSocket) -> u32 {
    sock.rx_head.saturating_sub(sock.rx_tail)
}

/// Append bytes to a socket's RX ring. Returns false if insufficient space.
fn rx_enqueue(sock: &mut NetlinkSocket, data: &[u8]) -> bool {
    let available = (NL_RX_BUF as u32).saturating_sub(sock.rx_head);
    if data.len() as u32 > available {
        // Not enough space — reset ring (drop old unread data)
        sock.rx_head = 0;
        sock.rx_tail = 0;
        if data.len() > NL_RX_BUF {
            return false;
        }
    }
    let start = sock.rx_head as usize;
    let len = data.len().min(NL_RX_BUF.saturating_sub(start));
    let mut i = 0;
    while i < len {
        sock.rx_buf[start + i] = data[i];
        i = i.saturating_add(1);
    }
    sock.rx_head = sock.rx_head.saturating_add(len as u32);
    true
}

/// Read bytes from a socket's RX ring into buf. Returns bytes copied.
fn rx_dequeue(sock: &mut NetlinkSocket, buf: &mut [u8; 8192]) -> usize {
    let avail = rx_available(sock) as usize;
    if avail == 0 {
        return 0;
    }
    let copy_len = avail.min(8192);
    let tail = sock.rx_tail as usize;
    let mut i = 0;
    while i < copy_len {
        buf[i] = sock.rx_buf[tail + i];
        i = i.saturating_add(1);
    }
    sock.rx_tail = sock.rx_tail.saturating_add(copy_len as u32);
    if sock.rx_tail >= sock.rx_head {
        sock.rx_head = 0;
        sock.rx_tail = 0;
    }
    copy_len
}

// ---------------------------------------------------------------------------
// Response builders — write into a caller-supplied [u8; 8192] scratch buffer
// ---------------------------------------------------------------------------

/// Write a u32 LE into buf at pos. Returns new pos.
fn put_u32le(buf: &mut [u8; 8192], pos: usize, val: u32) -> usize {
    if pos.saturating_add(4) > 8192 {
        return pos;
    }
    let b = val.to_le_bytes();
    buf[pos] = b[0];
    buf[pos + 1] = b[1];
    buf[pos + 2] = b[2];
    buf[pos + 3] = b[3];
    pos.saturating_add(4)
}

/// Write a u16 LE into buf at pos. Returns new pos.
fn put_u16le(buf: &mut [u8; 8192], pos: usize, val: u16) -> usize {
    if pos.saturating_add(2) > 8192 {
        return pos;
    }
    let b = val.to_le_bytes();
    buf[pos] = b[0];
    buf[pos + 1] = b[1];
    pos.saturating_add(2)
}

/// Write a u8 into buf at pos. Returns new pos.
fn put_u8(buf: &mut [u8; 8192], pos: usize, val: u8) -> usize {
    if pos >= 8192 {
        return pos;
    }
    buf[pos] = val;
    pos.saturating_add(1)
}

/// Write raw bytes into buf at pos. Returns new pos.
fn put_bytes(buf: &mut [u8; 8192], pos: usize, data: &[u8]) -> usize {
    let avail = 8192usize.saturating_sub(pos);
    let n = data.len().min(avail);
    let mut i = 0;
    while i < n {
        buf[pos + i] = data[i];
        i = i.saturating_add(1);
    }
    pos.saturating_add(n)
}

/// Write a NL attribute (TLV) into buf at pos. Returns new pos.
/// NL attr: len(2 LE) + type(2 LE) + data + padding to 4-byte boundary.
fn put_nlattr(buf: &mut [u8; 8192], pos: usize, attr_type: u16, data: &[u8]) -> usize {
    let total_unaligned = 4 + data.len();
    let aligned = (total_unaligned.saturating_add(3)) & !3;
    if pos.saturating_add(aligned) > 8192 {
        return pos;
    }
    let len_val = total_unaligned as u16;
    let mut p = put_u16le(buf, pos, len_val);
    p = put_u16le(buf, p, attr_type);
    p = put_bytes(buf, p, data);
    // Pad to 4-byte boundary
    while p < pos.saturating_add(aligned) && p < 8192 {
        buf[p] = 0;
        p = p.saturating_add(1);
    }
    pos.saturating_add(aligned)
}

/// Write a u32 NL attribute.
fn put_nlattr_u32(buf: &mut [u8; 8192], pos: usize, attr_type: u16, val: u32) -> usize {
    let b = val.to_le_bytes();
    put_nlattr(buf, pos, attr_type, &b)
}

/// Write a NL message header at pos in buf. Returns new pos.
fn put_nlhdr(
    buf: &mut [u8; 8192],
    pos: usize,
    msg_len: u32,
    msg_type: u16,
    flags: u16,
    seq: u32,
    pid: u32,
) -> usize {
    let hdr = NlMsgHdr {
        nlmsg_len: msg_len,
        nlmsg_type: msg_type,
        nlmsg_flags: flags,
        nlmsg_seq: seq,
        nlmsg_pid: pid,
    };
    let hdr_bytes = hdr.to_bytes();
    put_bytes(buf, pos, &hdr_bytes)
}

/// Build a DONE message into buf at pos. Returns new pos.
fn build_done(buf: &mut [u8; 8192], pos: usize, seq: u32, pid: u32) -> usize {
    put_nlhdr(
        buf,
        pos,
        NL_HDR_SIZE as u32,
        NLMSG_DONE,
        NLM_F_MULTI,
        seq,
        pid,
    )
}

/// Build an ERROR response (errno=0 = ACK) into buf at pos. Returns new pos.
fn build_ack(buf: &mut [u8; 8192], pos: usize, seq: u32, pid: u32, errno: i32) -> usize {
    // ERROR message: header(16) + errno(4) + original-header(16) = 36 bytes
    let total = (NL_HDR_SIZE + 4 + NL_HDR_SIZE) as u32;
    let mut p = put_nlhdr(buf, pos, total, NLMSG_ERROR, 0, seq, pid);
    p = put_u32le(buf, p, errno as u32);
    // Blank original header
    let mut i = 0;
    while i < NL_HDR_SIZE && p < 8192 {
        buf[p] = 0;
        p = p.saturating_add(1);
        i = i.saturating_add(1);
    }
    p
}

// ---------------------------------------------------------------------------
// Response payload builders for RTM_GETLINK / RTM_GETADDR / RTM_GETROUTE
// ---------------------------------------------------------------------------

// ifinfomsg: family(1) + pad(1) + ifi_type(2) + ifi_index(4) + ifi_flags(4) + ifi_change(4) = 16
const IFINFO_SIZE: usize = 16;

// ifaddrmsg: family(1) + prefixlen(1) + flags(1) + scope(1) + ifi_index(4) = 8
const IFADDR_SIZE: usize = 8;

// rtmsg: family(1)+dst_len(1)+src_len(1)+tos(1)+table(1)+protocol(1)+scope(1)+type(1)+flags(4) = 12
const RTMSG_SIZE: usize = 12;

/// NL attribute types for NEWLINK
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_ADDRESS: u16 = 1; // MAC address

/// NL attribute types for NEWADDR
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;

/// NL attribute types for NEWROUTE
const RTA_DST: u16 = 1;
const RTA_GATEWAY: u16 = 5;
const RTA_OIF: u16 = 4;

/// Build a RTM_NEWLINK message for one interface into buf at pos.
/// Returns new pos. iface_idx is 1-based interface index.
pub fn nl_build_link_info(
    buf: &mut [u8; 8192],
    pos: &mut usize,
    iface_idx: u32,
    name: &[u8],
    mtu: u32,
    mac: &[u8; 6],
) {
    let start = *pos;

    // Reserve space for header (filled in at end)
    let hdr_pos = start;
    *pos = pos.saturating_add(NL_HDR_SIZE);

    // ifinfomsg sub-header
    // family=0 (AF_UNSPEC), pad=0, type=1 (ARPHRD_ETHER), index, flags=IFF_UP|IFF_RUNNING=0x43, change=0
    let flags_val: u32 = 0x43;
    if *pos + IFINFO_SIZE > 8192 {
        *pos = start;
        return;
    }
    buf[*pos] = 0;
    buf[*pos + 1] = 0; // family, pad
    buf[*pos + 2] = 1;
    buf[*pos + 3] = 0; // ifi_type = ARPHRD_ETHER (LE)
    let ib = iface_idx.to_le_bytes();
    buf[*pos + 4] = ib[0];
    buf[*pos + 5] = ib[1];
    buf[*pos + 6] = ib[2];
    buf[*pos + 7] = ib[3];
    let fb = flags_val.to_le_bytes();
    buf[*pos + 8] = fb[0];
    buf[*pos + 9] = fb[1];
    buf[*pos + 10] = fb[2];
    buf[*pos + 11] = fb[3];
    buf[*pos + 12] = 0;
    buf[*pos + 13] = 0;
    buf[*pos + 14] = 0;
    buf[*pos + 15] = 0; // change
    *pos = pos.saturating_add(IFINFO_SIZE);

    // IFLA_ADDRESS (MAC)
    *pos = put_nlattr(buf, *pos, IFLA_ADDRESS, mac);
    // IFLA_IFNAME (NUL-terminated)
    let name_len = name.len().min(15);
    let mut name_buf = [0u8; 16];
    let mut ni = 0;
    while ni < name_len {
        name_buf[ni] = name[ni];
        ni = ni.saturating_add(1);
    }
    *pos = put_nlattr(buf, *pos, IFLA_IFNAME, &name_buf[..name_len + 1]);
    // IFLA_MTU
    *pos = put_nlattr_u32(buf, *pos, IFLA_MTU, mtu);

    // Fill in header now that we know total length
    let msg_len = (*pos - start) as u32;
    let hdr = NlMsgHdr {
        nlmsg_len: msg_len,
        nlmsg_type: RTM_NEWLINK,
        nlmsg_flags: NLM_F_MULTI,
        nlmsg_seq: 1,
        nlmsg_pid: 0,
    };
    let hdr_bytes = hdr.to_bytes();
    let mut hi = 0;
    while hi < NL_HDR_SIZE && hdr_pos + hi < 8192 {
        buf[hdr_pos + hi] = hdr_bytes[hi];
        hi = hi.saturating_add(1);
    }
}

/// Build a RTM_NEWADDR message for one IP address into buf at pos. Returns new pos.
pub fn nl_build_addr_info(
    buf: &mut [u8; 8192],
    pos: &mut usize,
    ip: &[u8; 4],
    prefix: u8,
    iface_idx: u32,
) {
    let start = *pos;
    let hdr_pos = start;
    *pos = pos.saturating_add(NL_HDR_SIZE);

    // ifaddrmsg sub-header: family=AF_INET(2), prefixlen, flags=0, scope=0(RT_SCOPE_UNIVERSE), index
    if *pos + IFADDR_SIZE > 8192 {
        *pos = start;
        return;
    }
    buf[*pos] = 2; // AF_INET
    buf[*pos + 1] = prefix;
    buf[*pos + 2] = 0; // flags
    buf[*pos + 3] = 0; // scope = RT_SCOPE_UNIVERSE
    let ib = iface_idx.to_le_bytes();
    buf[*pos + 4] = ib[0];
    buf[*pos + 5] = ib[1];
    buf[*pos + 6] = ib[2];
    buf[*pos + 7] = ib[3];
    *pos = pos.saturating_add(IFADDR_SIZE);

    // IFA_ADDRESS
    *pos = put_nlattr(buf, *pos, IFA_ADDRESS, ip);
    // IFA_LOCAL (same as address for point-to-point)
    *pos = put_nlattr(buf, *pos, IFA_LOCAL, ip);

    let msg_len = (*pos - start) as u32;
    let hdr = NlMsgHdr {
        nlmsg_len: msg_len,
        nlmsg_type: RTM_NEWADDR,
        nlmsg_flags: NLM_F_MULTI,
        nlmsg_seq: 1,
        nlmsg_pid: 0,
    };
    let hdr_bytes = hdr.to_bytes();
    let mut hi = 0;
    while hi < NL_HDR_SIZE && hdr_pos + hi < 8192 {
        buf[hdr_pos + hi] = hdr_bytes[hi];
        hi = hi.saturating_add(1);
    }
}

/// Build a RTM_NEWROUTE message (default route) into buf at pos.
fn nl_build_route_info(
    buf: &mut [u8; 8192],
    pos: &mut usize,
    dst: &[u8; 4],
    dst_prefix: u8,
    gateway: &[u8; 4],
    iface_idx: u32,
) {
    let start = *pos;
    let hdr_pos = start;
    *pos = pos.saturating_add(NL_HDR_SIZE);

    // rtmsg sub-header
    if *pos + RTMSG_SIZE > 8192 {
        *pos = start;
        return;
    }
    buf[*pos] = 2; // family = AF_INET
    buf[*pos + 1] = dst_prefix;
    buf[*pos + 2] = 0; // src_len
    buf[*pos + 3] = 0; // tos
    buf[*pos + 4] = 254; // table = RT_TABLE_MAIN
    buf[*pos + 5] = 3; // protocol = RTPROT_BOOT
    buf[*pos + 6] = 0; // scope = RT_SCOPE_UNIVERSE
    buf[*pos + 7] = 1; // type = RTN_UNICAST
    buf[*pos + 8] = 0;
    buf[*pos + 9] = 0;
    buf[*pos + 10] = 0;
    buf[*pos + 11] = 0; // flags
    *pos = pos.saturating_add(RTMSG_SIZE);

    if dst_prefix > 0 {
        *pos = put_nlattr(buf, *pos, RTA_DST, dst);
    }
    *pos = put_nlattr(buf, *pos, RTA_GATEWAY, gateway);
    *pos = put_nlattr_u32(buf, *pos, RTA_OIF, iface_idx);

    let msg_len = (*pos - start) as u32;
    let hdr = NlMsgHdr {
        nlmsg_len: msg_len,
        nlmsg_type: RTM_NEWROUTE,
        nlmsg_flags: NLM_F_MULTI,
        nlmsg_seq: 1,
        nlmsg_pid: 0,
    };
    let hdr_bytes = hdr.to_bytes();
    let mut hi = 0;
    while hi < NL_HDR_SIZE && hdr_pos + hi < 8192 {
        buf[hdr_pos + hi] = hdr_bytes[hi];
        hi = hi.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Kernel dispatch — build responses to RTM_GET* requests
// ---------------------------------------------------------------------------

/// Process an incoming netlink message from userspace and queue a response.
/// Called from netlink_send with the lock released on the socket table.
fn dispatch(sock_idx: usize, socks: &mut [NetlinkSocket; MAX_NL_SOCKETS], msg: &[u8]) {
    let hdr = match NlMsgHdr::from_bytes(msg) {
        Some(h) => h,
        None => return,
    };
    let pid = socks[sock_idx].pid;
    let proto = socks[sock_idx].nl_proto;
    let seq = hdr.nlmsg_seq;
    let flags = hdr.nlmsg_flags;
    let is_dump = (flags & NLM_F_DUMP) == NLM_F_DUMP;

    let mut resp = [0u8; 8192];
    let mut pos = 0usize;

    match hdr.nlmsg_type {
        NLMSG_NOOP => {
            // No response needed
            return;
        }
        NLMSG_DONE => {
            return;
        }
        RTM_GETLINK if proto == NETLINK_ROUTE => {
            // Enumerate interfaces — no-heap snapshot
            let mut snaps = [super::IfaceSnapshot::empty(); 8];
            let count = super::snapshot_interfaces(&mut snaps);
            let mut iface_idx = 1u32;
            let mut si = 0;
            while si < count {
                let snap = &snaps[si];
                // Build interface name as "ethN\0" (N = iface_idx - 1)
                let digit = (b'0').saturating_add(((iface_idx - 1) & 0x7) as u8);
                let name: [u8; 5] = [b'e', b't', b'h', digit, 0];
                nl_build_link_info(
                    &mut resp,
                    &mut pos,
                    iface_idx,
                    &name,
                    snap.mtu as u32,
                    &snap.mac,
                );
                iface_idx = iface_idx.saturating_add(1);
                si = si.saturating_add(1);
            }
            pos = build_done(&mut resp, pos, seq, pid);
        }
        RTM_GETADDR if proto == NETLINK_ROUTE => {
            let mut snaps = [super::IfaceSnapshot::empty(); 8];
            let count = super::snapshot_interfaces(&mut snaps);
            let mut iface_idx = 1u32;
            let mut si = 0;
            while si < count {
                let snap = &snaps[si];
                if snap.ip != [0u8; 4] {
                    let prefix = u32::from_be_bytes(snap.netmask).count_ones() as u8;
                    nl_build_addr_info(&mut resp, &mut pos, &snap.ip, prefix, iface_idx);
                }
                iface_idx = iface_idx.saturating_add(1);
                si = si.saturating_add(1);
            }
            pos = build_done(&mut resp, pos, seq, pid);
        }
        RTM_GETROUTE if proto == NETLINK_ROUTE => {
            let mut snaps = [super::IfaceSnapshot::empty(); 8];
            let count = super::snapshot_interfaces(&mut snaps);
            let mut iface_idx = 1u32;
            let mut si = 0;
            while si < count {
                let snap = &snaps[si];
                // Default route (dst=0/0)
                if snap.gateway != [0u8; 4] {
                    let dst = [0u8; 4];
                    nl_build_route_info(&mut resp, &mut pos, &dst, 0, &snap.gateway, iface_idx);
                }
                // Network route (dst = ip & mask)
                if snap.ip != [0u8; 4] && snap.netmask != [0u8; 4] {
                    let prefix = u32::from_be_bytes(snap.netmask).count_ones() as u8;
                    let net_u32 = u32::from_be_bytes(snap.ip) & u32::from_be_bytes(snap.netmask);
                    let dst = net_u32.to_be_bytes();
                    let gw = [0u8; 4];
                    nl_build_route_info(&mut resp, &mut pos, &dst, prefix, &gw, iface_idx);
                }
                iface_idx = iface_idx.saturating_add(1);
                si = si.saturating_add(1);
            }
            pos = build_done(&mut resp, pos, seq, pid);
        }
        _ => {
            // If ACK requested, send one
            if flags & NLM_F_ACK != 0 {
                pos = build_ack(&mut resp, pos, seq, pid, 0);
            } else {
                return;
            }
        }
    }

    if pos > 0 {
        rx_enqueue(&mut socks[sock_idx], &resp[..pos]);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a Netlink socket. Returns fd >= NETLINK_FD_BASE, or -1 on error.
pub fn netlink_socket(nl_proto: u32) -> i32 {
    let mut socks = NL_SOCKETS.lock();
    let mut i = 0;
    while i < MAX_NL_SOCKETS {
        if !socks[i].active {
            socks[i] = NetlinkSocket::empty();
            socks[i].active = true;
            socks[i].fd = NETLINK_FD_BASE + i as i32;
            socks[i].nl_proto = nl_proto;
            // Assign a PID derived from the fd slot
            socks[i].pid = (1000 + i) as u32;
            return socks[i].fd;
        }
        i = i.saturating_add(1);
    }
    -1
}

/// Bind a Netlink socket to a PID and multicast groups.
pub fn netlink_bind(fd: i32, pid: u32, groups: u32) -> i32 {
    let mut socks = NL_SOCKETS.lock();
    match find_by_fd(&*socks, fd) {
        Some(idx) => {
            socks[idx].pid = pid;
            socks[idx].groups = groups;
            0
        }
        None => -1,
    }
}

/// Send a Netlink message. Dispatches to kernel handler and queues response.
/// Returns bytes sent, or negative errno on error.
pub fn netlink_send(fd: i32, msg: &[u8]) -> isize {
    if msg.len() < NL_HDR_SIZE {
        return -22; // EINVAL
    }

    let mut socks = NL_SOCKETS.lock();
    let idx = match find_by_fd(&*socks, fd) {
        Some(i) => i,
        None => return -9, // EBADF
    };

    dispatch(idx, &mut *socks, msg);
    msg.len() as isize
}

/// Receive a Netlink message (non-blocking).
/// Returns bytes copied into buf, or negative errno.
pub fn netlink_recv(fd: i32, buf: &mut [u8; 8192]) -> isize {
    let mut socks = NL_SOCKETS.lock();
    let idx = match find_by_fd(&*socks, fd) {
        Some(i) => i,
        None => return -9, // EBADF
    };

    let n = rx_dequeue(&mut socks[idx], buf);
    if n == 0 {
        return -11; // EAGAIN
    }
    n as isize
}

/// Close a Netlink socket.
pub fn netlink_close(fd: i32) {
    let mut socks = NL_SOCKETS.lock();
    if let Some(idx) = find_by_fd(&*socks, fd) {
        socks[idx] = NetlinkSocket::empty();
    }
}

/// Returns true if fd is a Netlink socket fd.
pub fn netlink_is_fd(fd: i32) -> bool {
    if fd < NETLINK_FD_BASE {
        return false;
    }
    let socks = NL_SOCKETS.lock();
    find_by_fd(&*socks, fd).is_some()
}

/// Broadcast a Netlink message to all sockets subscribed to a multicast group.
/// group_bit: a single bit from the groups bitmask (1 << group_number).
pub fn netlink_broadcast(group_bit: u32, msg: &[u8]) {
    let mut socks = NL_SOCKETS.lock();
    let mut i = 0;
    while i < MAX_NL_SOCKETS {
        if socks[i].active && socks[i].groups & group_bit != 0 {
            rx_enqueue(&mut socks[i], msg);
        }
        i = i.saturating_add(1);
    }
}
