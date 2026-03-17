/// rtnetlink — kernel-side rtnetlink routing netlink interface
///
/// Processes RTM_* messages from userspace (or internal callers) to:
///   RTM_GETLINK   — enumerate network interfaces
///   RTM_NEWLINK   — create/modify a link
///   RTM_DELLINK   — delete a link
///   RTM_GETADDR   — enumerate IP addresses
///   RTM_NEWADDR   — add an IP address
///   RTM_DELADDR   — remove an IP address
///   RTM_GETROUTE  — enumerate routes
///   RTM_NEWROUTE  — add a route
///   RTM_DELROUTE  — delete a route
///
/// Message encoding: netlink header + rtattr TLV chain (all little-endian).
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Netlink / rtnetlink constants (match Linux UAPI)
// ---------------------------------------------------------------------------

pub const NLMSG_DONE: u16 = 3;
pub const NLMSG_ERROR: u16 = 2;
pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_GETLINK: u16 = 18;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
pub const RTM_GETADDR: u16 = 22;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;
pub const RTM_GETROUTE: u16 = 26;

// rtattr types for links
pub const IFLA_IFNAME: u16 = 3;
pub const IFLA_MTU: u16 = 4;
pub const IFLA_OPERSTATE: u16 = 16;

// rtattr types for addresses
pub const IFA_ADDRESS: u16 = 1;
pub const IFA_LOCAL: u16 = 2;
pub const IFA_LABEL: u16 = 3;

// rtattr types for routes
pub const RTA_DST: u16 = 1;
pub const RTA_GATEWAY: u16 = 5;
pub const RTA_OIF: u16 = 4; // output interface index

// Address families
pub const AF_INET: u8 = 2;
pub const AF_INET6: u8 = 10;

const MAX_LINKS: usize = 16;
const MAX_ADDRS: usize = 32;
const MAX_ROUTES: usize = 32;

// ---------------------------------------------------------------------------
// Internal tables
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct IfInfoMsg {
    pub index: u32,
    pub name: [u8; 16],
    pub name_len: u8,
    pub mtu: u32,
    pub flags: u32,    // IFF_UP etc.
    pub operstate: u8, // IF_OPER_UP etc.
    pub active: bool,
}

impl IfInfoMsg {
    pub const fn empty() -> Self {
        IfInfoMsg {
            index: 0,
            name: [0u8; 16],
            name_len: 0,
            mtu: 1500,
            flags: 0,
            operstate: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct IfAddrMsg {
    pub family: u8,
    pub prefix: u8,
    pub if_index: u32,
    pub address: [u8; 16], // IPv4 in [0..4], IPv6 in [0..16]
    pub local: [u8; 16],
    pub label: [u8; 16],
    pub label_len: u8,
    pub active: bool,
}

impl IfAddrMsg {
    pub const fn empty() -> Self {
        IfAddrMsg {
            family: AF_INET,
            prefix: 0,
            if_index: 0,
            address: [0u8; 16],
            local: [0u8; 16],
            label: [0u8; 16],
            label_len: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct RtMsg {
    pub family: u8,
    pub dst_len: u8,
    pub src_len: u8,
    pub tos: u8,
    pub table: u8,
    pub protocol: u8,
    pub scope: u8,
    pub rt_type: u8,
    pub dst: [u8; 16],
    pub gateway: [u8; 16],
    pub oif: u32,
    pub active: bool,
}

impl RtMsg {
    pub const fn empty() -> Self {
        RtMsg {
            family: AF_INET,
            dst_len: 0,
            src_len: 0,
            tos: 0,
            table: 0xFE, // RT_TABLE_MAIN
            protocol: 2, // RTPROT_KERNEL
            scope: 0,
            rt_type: 1, // RTN_UNICAST
            dst: [0u8; 16],
            gateway: [0u8; 16],
            oif: 0,
            active: false,
        }
    }
}

const EMPTY_LINK: IfInfoMsg = IfInfoMsg::empty();
const EMPTY_ADDR: IfAddrMsg = IfAddrMsg::empty();
const EMPTY_ROUTE: RtMsg = RtMsg::empty();

static LINKS: Mutex<[IfInfoMsg; MAX_LINKS]> = Mutex::new([EMPTY_LINK; MAX_LINKS]);
static ADDRS: Mutex<[IfAddrMsg; MAX_ADDRS]> = Mutex::new([EMPTY_ADDR; MAX_ADDRS]);
static ROUTES: Mutex<[RtMsg; MAX_ROUTES]> = Mutex::new([EMPTY_ROUTE; MAX_ROUTES]);
static NEXT_IFINDEX: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Serialisation helpers
// ---------------------------------------------------------------------------

fn write_u16_le(buf: &mut [u8], off: usize, v: u16) -> usize {
    if off.saturating_add(2) > buf.len() {
        return off;
    }
    buf[off] = (v & 0xFF) as u8;
    buf[off + 1] = (v >> 8) as u8;
    off.saturating_add(2)
}

fn write_u32_le(buf: &mut [u8], off: usize, v: u32) -> usize {
    if off.saturating_add(4) > buf.len() {
        return off;
    }
    buf[off] = (v & 0xFF) as u8;
    buf[off + 1] = ((v >> 8) & 0xFF) as u8;
    buf[off + 2] = ((v >> 16) & 0xFF) as u8;
    buf[off + 3] = (v >> 24) as u8;
    off.saturating_add(4)
}

/// Write an rtattr TLV. Returns new offset.
fn write_rta(buf: &mut [u8], off: usize, rta_type: u16, data: &[u8]) -> usize {
    let rta_len = (4usize).saturating_add(data.len()) as u16;
    let mut o = write_u16_le(buf, off, rta_len);
    o = write_u16_le(buf, o, rta_type);
    let end = o.saturating_add(data.len());
    if end <= buf.len() {
        let mut k = 0usize;
        while k < data.len() {
            buf[o + k] = data[k];
            k = k.saturating_add(1);
        }
    }
    // rtattr must be 4-byte aligned
    let aligned = (end.saturating_add(3)) & !3;
    aligned.min(buf.len())
}

/// Write a netlink message header. Returns new offset.
fn write_nlmsghdr(buf: &mut [u8], off: usize, msg_len: u32, msg_type: u16, seq: u32) -> usize {
    let mut o = write_u32_le(buf, off, msg_len);
    o = write_u16_le(buf, o, msg_type);
    o = write_u16_le(buf, o, 0x0002); // NLM_F_MULTI
    o = write_u32_le(buf, o, seq);
    write_u32_le(buf, o, 0) // pid=0 (kernel)
}

// ---------------------------------------------------------------------------
// Public API: link table management
// ---------------------------------------------------------------------------

pub fn rtnetlink_newlink(name: &[u8], mtu: u32, flags: u32) -> Option<u32> {
    let idx = NEXT_IFINDEX.fetch_add(1, Ordering::Relaxed);
    let mut links = LINKS.lock();
    let mut i = 0usize;
    while i < MAX_LINKS {
        if !links[i].active {
            links[i] = IfInfoMsg::empty();
            links[i].index = idx;
            let nlen = name.len().min(15);
            let mut k = 0usize;
            while k < nlen {
                links[i].name[k] = name[k];
                k = k.saturating_add(1);
            }
            links[i].name_len = nlen as u8;
            links[i].mtu = mtu;
            links[i].flags = flags;
            links[i].active = true;
            return Some(idx);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn rtnetlink_dellink(if_index: u32) -> bool {
    let mut links = LINKS.lock();
    let mut i = 0usize;
    while i < MAX_LINKS {
        if links[i].active && links[i].index == if_index {
            links[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API: address table management
// ---------------------------------------------------------------------------

pub fn rtnetlink_newaddr(if_index: u32, family: u8, prefix: u8, addr: &[u8]) -> bool {
    let mut addrs = ADDRS.lock();
    let mut i = 0usize;
    while i < MAX_ADDRS {
        if !addrs[i].active {
            addrs[i] = IfAddrMsg::empty();
            addrs[i].family = family;
            addrs[i].prefix = prefix;
            addrs[i].if_index = if_index;
            let alen = addr.len().min(16);
            let mut k = 0usize;
            while k < alen {
                addrs[i].address[k] = addr[k];
                addrs[i].local[k] = addr[k];
                k = k.saturating_add(1);
            }
            addrs[i].active = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn rtnetlink_deladdr(if_index: u32, family: u8, addr: &[u8]) -> bool {
    let mut addrs = ADDRS.lock();
    let mut i = 0usize;
    while i < MAX_ADDRS {
        if addrs[i].active && addrs[i].if_index == if_index && addrs[i].family == family {
            let alen = addr.len().min(16);
            let mut k = 0usize;
            let mut matches = true;
            while k < alen {
                if addrs[i].address[k] != addr[k] {
                    matches = false;
                    break;
                }
                k = k.saturating_add(1);
            }
            if matches {
                addrs[i].active = false;
                return true;
            }
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API: route table management
// ---------------------------------------------------------------------------

pub fn rtnetlink_newroute(family: u8, dst: &[u8], dst_len: u8, gw: &[u8], oif: u32) -> bool {
    let mut routes = ROUTES.lock();
    let mut i = 0usize;
    while i < MAX_ROUTES {
        if !routes[i].active {
            routes[i] = RtMsg::empty();
            routes[i].family = family;
            routes[i].dst_len = dst_len;
            routes[i].oif = oif;
            let dlen = dst.len().min(16);
            let glen = gw.len().min(16);
            let mut k = 0usize;
            while k < dlen {
                routes[i].dst[k] = dst[k];
                k = k.saturating_add(1);
            }
            k = 0;
            while k < glen {
                routes[i].gateway[k] = gw[k];
                k = k.saturating_add(1);
            }
            routes[i].active = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn rtnetlink_delroute(family: u8, dst: &[u8], dst_len: u8) -> bool {
    let mut routes = ROUTES.lock();
    let mut i = 0usize;
    while i < MAX_ROUTES {
        if routes[i].active && routes[i].family == family && routes[i].dst_len == dst_len {
            let dlen = dst.len().min(16);
            let mut k = 0usize;
            let mut matches = true;
            while k < dlen {
                if routes[i].dst[k] != dst[k] {
                    matches = false;
                    break;
                }
                k = k.saturating_add(1);
            }
            if matches {
                routes[i].active = false;
                return true;
            }
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Message dispatch
// ---------------------------------------------------------------------------

/// Process an RTM_* request and write response into `resp`.
/// Returns bytes written to resp, or 0 on unrecognised type.
pub fn rtnetlink_dispatch(msg_type: u16, seq: u32, resp: &mut [u8; 4096]) -> usize {
    match msg_type {
        RTM_GETLINK => {
            let links = LINKS.lock();
            let mut off = 0usize;
            let mut i = 0usize;
            while i < MAX_LINKS {
                if links[i].active {
                    // Write a minimal ifinfomsg (16 bytes) + IFLA_IFNAME attr
                    let name_data = &links[i].name[..links[i].name_len as usize];
                    // Reserve space for nlmsghdr (16) + ifinfomsg (16)
                    let hdr_off = off;
                    off = off.saturating_add(16); // nlmsghdr placeholder
                                                  // ifinfomsg: family, pad, type, index, flags, change
                    if off.saturating_add(16) <= 4096 {
                        resp[off] = 0;
                        resp[off + 1] = 0; // family, pad
                        off = off.saturating_add(2);
                        off = write_u16_le(resp, off, 0); // ifi_type
                        off = write_u32_le(resp, off, links[i].index);
                        off = write_u32_le(resp, off, links[i].flags);
                        off = write_u32_le(resp, off, 0xFFFF_FFFF_u32); // change mask
                    }
                    off = write_rta(resp, off, IFLA_IFNAME, name_data);
                    off = write_rta(resp, off, IFLA_MTU, &links[i].mtu.to_le_bytes());
                    let msg_len = (off - hdr_off) as u32;
                    write_nlmsghdr(resp, hdr_off, msg_len, RTM_NEWLINK, seq);
                }
                i = i.saturating_add(1);
            }
            // NLMSG_DONE
            if off.saturating_add(16) <= 4096 {
                off = write_nlmsghdr(resp, off, 16, NLMSG_DONE, seq);
            }
            off
        }
        RTM_GETADDR => {
            let addrs = ADDRS.lock();
            let mut off = 0usize;
            let mut i = 0usize;
            while i < MAX_ADDRS {
                if addrs[i].active {
                    let hdr_off = off;
                    off = off.saturating_add(16);
                    // ifaddrmsg: family, prefixlen, flags, scope, index
                    if off.saturating_add(8) <= 4096 {
                        resp[off] = addrs[i].family;
                        resp[off + 1] = addrs[i].prefix;
                        resp[off + 2] = 0;
                        resp[off + 3] = 0; // flags, scope
                        off = off.saturating_add(4);
                        off = write_u32_le(resp, off, addrs[i].if_index);
                    }
                    let addr_len = if addrs[i].family == AF_INET6 { 16 } else { 4 };
                    off = write_rta(resp, off, IFA_ADDRESS, &addrs[i].address[..addr_len]);
                    off = write_rta(resp, off, IFA_LOCAL, &addrs[i].local[..addr_len]);
                    let msg_len = (off - hdr_off) as u32;
                    write_nlmsghdr(resp, hdr_off, msg_len, RTM_NEWADDR, seq);
                }
                i = i.saturating_add(1);
            }
            if off.saturating_add(16) <= 4096 {
                off = write_nlmsghdr(resp, off, 16, NLMSG_DONE, seq);
            }
            off
        }
        RTM_GETROUTE => {
            let routes = ROUTES.lock();
            let mut off = 0usize;
            let mut i = 0usize;
            while i < MAX_ROUTES {
                if routes[i].active {
                    let hdr_off = off;
                    off = off.saturating_add(16);
                    // rtmsg: family, dst_len, src_len, tos, table, protocol, scope, type
                    if off.saturating_add(8) <= 4096 {
                        resp[off] = routes[i].family;
                        resp[off + 1] = routes[i].dst_len;
                        resp[off + 2] = routes[i].src_len;
                        resp[off + 3] = routes[i].tos;
                        resp[off + 4] = routes[i].table;
                        resp[off + 5] = routes[i].protocol;
                        resp[off + 6] = routes[i].scope;
                        resp[off + 7] = routes[i].rt_type;
                        off = off.saturating_add(8);
                    }
                    let dlen = if routes[i].family == AF_INET6 { 16 } else { 4 };
                    off = write_rta(resp, off, RTA_DST, &routes[i].dst[..dlen]);
                    off = write_rta(resp, off, RTA_GATEWAY, &routes[i].gateway[..dlen]);
                    off = write_rta(resp, off, RTA_OIF, &routes[i].oif.to_le_bytes());
                    let msg_len = (off - hdr_off) as u32;
                    write_nlmsghdr(resp, hdr_off, msg_len, RTM_NEWROUTE, seq);
                }
                i = i.saturating_add(1);
            }
            if off.saturating_add(16) <= 4096 {
                off = write_nlmsghdr(resp, off, 16, NLMSG_DONE, seq);
            }
            off
        }
        _ => 0,
    }
}

pub fn init() {
    // Pre-register loopback interface
    rtnetlink_newlink(b"lo", 65536, 0x09); // IFF_UP | IFF_LOOPBACK
    rtnetlink_newaddr(1, AF_INET, 8, &[127, 0, 0, 1]);
    serial_println!("[rtnetlink] rtnetlink routing interface initialized");
}
