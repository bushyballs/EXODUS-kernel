/// Datagram Congestion Control Protocol — RFC 4340 (no-heap implementation)
///
/// DCCP is a connection-oriented transport with unreliable datagrams and
/// built-in congestion control.  Unlike TCP it does not retransmit lost
/// packets; unlike UDP it manages connection state and CCID negotiation.
///
/// This implementation follows the no-heap rules:
///   - No Vec, Box, String, alloc::*
///   - No f32 / f64 casts
///   - No unwrap() / expect() / panic!()
///   - Saturating arithmetic for counters
///   - wrapping_add for sequence numbers
///   - All statics hold Copy types with const fn empty()
///
/// Inspired by: RFC 4340, RFC 4341 (CCID 2). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// IP protocol number for DCCP (RFC 4340 §5.1)
pub const IPPROTO_DCCP: u8 = 33;

/// Maximum number of concurrent DCCP sockets
pub const MAX_DCCP_SOCKS: usize = 16;

/// File-descriptor base for DCCP sockets (avoids collision with TCP/UDP)
pub const DCCP_FD_BASE: i32 = 6100;

// DCCP packet types (4-bit field, RFC 4340 §5.1)
pub const DCCP_PKT_REQUEST: u8 = 0;
pub const DCCP_PKT_RESPONSE: u8 = 1;
pub const DCCP_PKT_DATA: u8 = 2;
pub const DCCP_PKT_ACK: u8 = 3;
pub const DCCP_PKT_DATAACK: u8 = 4;
pub const DCCP_PKT_CLOSEREQ: u8 = 5;
pub const DCCP_PKT_CLOSE: u8 = 6;
pub const DCCP_PKT_RESET: u8 = 7;

// DCCP feature numbers (RFC 4340 §6)
pub const DCCPF_CCID: u8 = 1;
pub const DCCPF_SEQUENCE_WINDOW: u8 = 3;

// Internal DCCP header size (bytes) used by this stub
const DCCP_HDR_LEN: usize = 16;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// DCCP connection-state machine states (RFC 4340 §7.1)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DccpState {
    Closed,
    Listen,
    Requesting,
    Responding,
    Open,
    CloseReq,
    Closing,
    TimeWait,
}

/// A single DCCP socket entry stored in the static socket table.
#[derive(Clone, Copy)]
pub struct DccpSock {
    /// File descriptor (DCCP_FD_BASE + index)
    pub fd: i32,
    pub state: DccpState,
    pub local_port: u16,
    pub remote_port: u16,
    pub remote_ip: [u8; 4],
    /// Congestion-Control ID negotiated for this connection
    pub ccid: u8,
    /// Greatest Sequence number Received (RFC 4340 §7.1)
    pub gsr: u64,
    /// Greatest Sequence number Sent
    pub gss: u64,
    /// Receive ring buffer (fixed 4096 bytes)
    pub rx_buf: [u8; 4096],
    /// Number of valid bytes currently in rx_buf
    pub rx_len: u16,
    /// Next transmit sequence number
    pub tx_seq: u64,
    pub active: bool,
}

impl DccpSock {
    pub const fn empty() -> Self {
        DccpSock {
            fd: -1,
            state: DccpState::Closed,
            local_port: 0,
            remote_port: 0,
            remote_ip: [0u8; 4],
            ccid: 2,
            gsr: 0,
            gss: 0,
            rx_buf: [0u8; 4096],
            rx_len: 0,
            tx_seq: 1,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static socket table
// ---------------------------------------------------------------------------

static DCCP_SOCKS: Mutex<[DccpSock; MAX_DCCP_SOCKS]> =
    Mutex::new([DccpSock::empty(); MAX_DCCP_SOCKS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build a minimal 16-byte DCCP header into `out`.
/// Layout (RFC 4340 §5.1, generic header + 48-bit seq, no ack sub-header):
///   [0..2]  source port
///   [2..4]  destination port
///   [4]     data offset (in 32-bit words; 4 = 16 bytes)
///   [5]     ccval(hi 4) | cscov(lo 4)
///   [6..8]  checksum (zeroed — stub)
///   [8]     pkt_type(hi 4) | X=1 | reserved(lo 3)
///   [9]     reserved
///   [10..16] 48-bit sequence number (big-endian, 6 bytes)
fn build_dccp_hdr(
    out: &mut [u8; DCCP_HDR_LEN],
    src_port: u16,
    dst_port: u16,
    pkt_type: u8,
    seq: u64,
) {
    out[0..2].copy_from_slice(&src_port.to_be_bytes());
    out[2..4].copy_from_slice(&dst_port.to_be_bytes());
    out[4] = 4; // data offset = 16 / 4
    out[5] = 0; // ccval=0, cscov=0
    out[6] = 0; // checksum high (stub)
    out[7] = 0; // checksum low  (stub)
    out[8] = ((pkt_type & 0x0F) << 1) | 1; // type | X=1
    out[9] = 0; // reserved
                // 48-bit sequence number in bytes 10..16 (big-endian, take low 6 bytes)
    let seq_be = seq.to_be_bytes(); // 8 bytes, take [2..8]
    out[10..16].copy_from_slice(&seq_be[2..8]);
}

/// Transmit a DCCP packet over IPv4 using the raw frame interface.
///
/// Builds an IPv4 + DCCP frame and hands it to `net::send_ip_frame_pub`.
/// This is a best-effort stub: if the ARP cache has no entry for the
/// destination, the frame is sent to the Ethernet broadcast MAC.
fn send_dccp_pkt(sock: &DccpSock, pkt_type: u8, payload: &[u8], payload_len: usize) {
    use crate::net::arp;
    use crate::net::ipv4;
    use crate::net::{Ipv4Addr, MacAddr};

    // Clamp payload length to what was actually provided
    let plen = payload_len.min(payload.len());

    // Build DCCP header
    let mut dccp_hdr = [0u8; DCCP_HDR_LEN];
    build_dccp_hdr(
        &mut dccp_hdr,
        sock.local_port,
        sock.remote_port,
        pkt_type,
        sock.gss,
    );

    // Total DCCP segment = header + payload
    let dccp_total = DCCP_HDR_LEN.saturating_add(plen);
    if dccp_total > 1480 {
        // Exceeds typical MTU payload — drop silently
        return;
    }

    // Assemble into a scratch buffer (max MTU payload)
    let mut seg = [0u8; 1480];
    seg[..DCCP_HDR_LEN].copy_from_slice(&dccp_hdr);
    if plen > 0 {
        seg[DCCP_HDR_LEN..DCCP_HDR_LEN.saturating_add(plen)].copy_from_slice(&payload[..plen]);
    }

    // Build IPv4 header
    let dst_ip = Ipv4Addr(sock.remote_ip);
    let src_ip = crate::net::primary_ip().unwrap_or(Ipv4Addr([127, 0, 0, 1]));
    let ip_hdr = ipv4::build_header(src_ip, dst_ip, IPPROTO_DCCP, dccp_total as u16, 64);
    let ip_bytes =
        unsafe { core::slice::from_raw_parts(&ip_hdr as *const ipv4::Ipv4Header as *const u8, 20) };

    // Combine IPv4 header + DCCP segment
    let total_ip = 20usize.saturating_add(dccp_total);
    if total_ip > 1500 {
        return;
    }
    let mut ip_pkt = [0u8; 1500];
    ip_pkt[..20].copy_from_slice(ip_bytes);
    ip_pkt[20..20usize.saturating_add(dccp_total)].copy_from_slice(&seg[..dccp_total]);

    // Resolve destination MAC
    let dst_mac = arp::lookup(dst_ip).unwrap_or(MacAddr::BROADCAST);
    let src_mac = crate::net::primary_mac().unwrap_or(MacAddr::ZERO);

    crate::net::send_ip_frame_pub(src_mac, dst_mac, &ip_pkt[..total_ip]);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Allocate a new DCCP socket.
///
/// Returns the file descriptor (>= DCCP_FD_BASE) on success, or -1 if the
/// socket table is full.
pub fn dccp_socket() -> i32 {
    let mut socks = DCCP_SOCKS.lock();
    let mut i = 0usize;
    while i < MAX_DCCP_SOCKS {
        if !socks[i].active {
            socks[i] = DccpSock::empty();
            socks[i].fd = DCCP_FD_BASE.saturating_add(i as i32);
            socks[i].state = DccpState::Closed;
            socks[i].active = true;
            return socks[i].fd;
        }
        i = i.saturating_add(1);
    }
    -1
}

/// Bind a DCCP socket to a local port.
///
/// Returns 0 on success, -1 if `fd` is not a valid DCCP socket.
pub fn dccp_bind(fd: i32, port: u16) -> i32 {
    let mut socks = DCCP_SOCKS.lock();
    let mut i = 0usize;
    while i < MAX_DCCP_SOCKS {
        if socks[i].active && socks[i].fd == fd {
            socks[i].local_port = port;
            return 0;
        }
        i = i.saturating_add(1);
    }
    -1
}

/// Initiate a DCCP connection to a remote endpoint.
///
/// Stub handshake:
///   1. Set state = Requesting, send a REQUEST packet.
///   2. Immediately (no real network) transition to Open (simulate RESPONSE+ACK).
///
/// Returns 0 on success, -1 if `fd` is invalid.
pub fn dccp_connect(fd: i32, dst_ip: [u8; 4], dst_port: u16) -> i32 {
    // Find socket index
    let idx = {
        let socks = DCCP_SOCKS.lock();
        let mut found: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_DCCP_SOCKS {
            if socks[i].active && socks[i].fd == fd {
                found = Some(i);
                break;
            }
            i = i.saturating_add(1);
        }
        match found {
            Some(v) => v,
            None => return -1,
        }
    };

    // Transition to Requesting and send REQUEST
    {
        let mut socks = DCCP_SOCKS.lock();
        socks[idx].remote_ip = dst_ip;
        socks[idx].remote_port = dst_port;
        socks[idx].state = DccpState::Requesting;
        socks[idx].gss = socks[idx].gss.wrapping_add(1);

        // Build a local copy of the sock for sending (avoids holding lock during send)
        let snap = socks[idx];
        drop(socks);

        send_dccp_pkt(&snap, DCCP_PKT_REQUEST, &[], 0);
    }

    // Stub: immediately simulate RESPONSE received → send ACK → Open
    {
        let mut socks = DCCP_SOCKS.lock();
        socks[idx].gsr = socks[idx].gsr.wrapping_add(1);
        socks[idx].gss = socks[idx].gss.wrapping_add(1);
        socks[idx].state = DccpState::Open;

        let snap = socks[idx];
        drop(socks);

        send_dccp_pkt(&snap, DCCP_PKT_ACK, &[], 0);
    }

    serial_println!(
        "[dccp] fd={} connected -> {}.{}.{}.{}:{}",
        fd,
        dst_ip[0],
        dst_ip[1],
        dst_ip[2],
        dst_ip[3],
        dst_port
    );

    0
}

/// Send a datagram on an open DCCP connection.
///
/// Builds a DCCP DATA packet and transmits it via IPv4.
/// Returns the number of bytes sent (>= 0) on success, or -1 on error.
pub fn dccp_send(fd: i32, data: &[u8]) -> isize {
    let idx = {
        let socks = DCCP_SOCKS.lock();
        let mut found: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_DCCP_SOCKS {
            if socks[i].active && socks[i].fd == fd {
                found = Some(i);
                break;
            }
            i = i.saturating_add(1);
        }
        match found {
            Some(v) => v,
            None => return -1,
        }
    };

    {
        let mut socks = DCCP_SOCKS.lock();
        if socks[idx].state != DccpState::Open {
            return -1;
        }
        socks[idx].gss = socks[idx].gss.wrapping_add(1);
        let snap = socks[idx];
        let send_len = data.len().min(1452); // max payload in a DCCP DATA frame
        drop(socks);

        send_dccp_pkt(&snap, DCCP_PKT_DATA, data, send_len);

        send_len as isize
    }
}

/// Receive a datagram from a DCCP socket.
///
/// Copies bytes from the internal receive ring into `buf`.
/// Returns number of bytes copied (>= 0), or -1 if `fd` is invalid / no data.
pub fn dccp_recv(fd: i32, buf: &mut [u8; 4096]) -> isize {
    let mut socks = DCCP_SOCKS.lock();
    let mut i = 0usize;
    while i < MAX_DCCP_SOCKS {
        if socks[i].active && socks[i].fd == fd {
            let avail = socks[i].rx_len as usize;
            if avail == 0 {
                return -1; // no data
            }
            let copy = avail.min(4096);
            let mut k = 0usize;
            while k < copy {
                buf[k] = socks[i].rx_buf[k];
                k = k.saturating_add(1);
            }
            // Consume the bytes (shift remaining data forward)
            let remain = avail.saturating_sub(copy);
            let mut k = 0usize;
            while k < remain {
                socks[i].rx_buf[k] = socks[i].rx_buf[k.saturating_add(copy)];
                k = k.saturating_add(1);
            }
            socks[i].rx_len = remain as u16;
            return copy as isize;
        }
        i = i.saturating_add(1);
    }
    -1
}

/// Close a DCCP connection.
///
/// Sends a CLOSE packet and transitions state to Closing.
/// Returns 0 on success, -1 if `fd` is not found.
pub fn dccp_close(fd: i32) -> i32 {
    let idx = {
        let socks = DCCP_SOCKS.lock();
        let mut found: Option<usize> = None;
        let mut i = 0usize;
        while i < MAX_DCCP_SOCKS {
            if socks[i].active && socks[i].fd == fd {
                found = Some(i);
                break;
            }
            i = i.saturating_add(1);
        }
        match found {
            Some(v) => v,
            None => return -1,
        }
    };

    let snap = {
        let mut socks = DCCP_SOCKS.lock();
        socks[idx].state = DccpState::Closing;
        socks[idx].gss = socks[idx].gss.wrapping_add(1);
        socks[idx]
    };

    send_dccp_pkt(&snap, DCCP_PKT_CLOSE, &[], 0);

    // Mark socket as closed/inactive after sending CLOSE
    {
        let mut socks = DCCP_SOCKS.lock();
        socks[idx].state = DccpState::Closed;
        socks[idx].active = false;
    }

    serial_println!("[dccp] fd={} closed", fd);
    0
}

/// Return `true` if `fd` belongs to a DCCP socket (active or not within range).
pub fn dccp_is_fd(fd: i32) -> bool {
    if fd < DCCP_FD_BASE {
        return false;
    }
    let idx = (fd - DCCP_FD_BASE) as usize;
    if idx >= MAX_DCCP_SOCKS {
        return false;
    }
    let socks = DCCP_SOCKS.lock();
    socks[idx].fd == fd
}

/// Process an incoming DCCP packet.
///
/// Called by the IPv4 receive path when `ip_hdr.protocol == IPPROTO_DCCP`.
///
/// Header layout (16 bytes):
///   [0..2]   source port
///   [2..4]   destination port
///   [4]      data offset (32-bit words)
///   [5]      ccval(hi 4) | cscov(lo 4)
///   [6..8]   checksum
///   [8]      pkt_type(hi 4) | X bit | …
///   [9]      reserved
///   [10..16] 48-bit sequence number (if X=1)
///
/// DATA and DATAACK packets enqueue their payload into the matching socket's
/// rx_buf.  Other packet types update connection state accordingly.
pub fn dccp_input(pkt: &[u8], len: usize, src_ip: [u8; 4]) {
    if len < DCCP_HDR_LEN {
        return; // packet too short to parse
    }

    let src_port = u16::from_be_bytes([pkt[0], pkt[1]]);
    let dst_port = u16::from_be_bytes([pkt[2], pkt[3]]);
    let data_off = pkt[4] as usize; // in 32-bit words
    let type_byte = pkt[8];
    let pkt_type = (type_byte >> 1) & 0x0F; // bits [4:1]
    let x_bit = type_byte & 1; // extended sequence flag

    // Extract 48-bit sequence number (bytes 10..16 when X=1, else bytes 8..12)
    let seq: u64 = if x_bit != 0 && len >= 16 {
        let hi = u16::from_be_bytes([pkt[10], pkt[11]]) as u64;
        let lo = u32::from_be_bytes([pkt[12], pkt[13], pkt[14], pkt[15]]) as u64;
        (hi << 32) | lo
    } else if len >= 12 {
        u32::from_be_bytes([pkt[8], pkt[9], pkt[10], pkt[11]]) as u64
    } else {
        0
    };

    // Compute payload start
    let hdr_bytes = data_off.saturating_mul(4);
    let payload_start = hdr_bytes.min(len);
    let payload = &pkt[payload_start..len];

    // Find the matching socket (by destination port and remote IP/port)
    let mut socks = DCCP_SOCKS.lock();
    let mut i = 0usize;
    while i < MAX_DCCP_SOCKS {
        let s = &mut socks[i];
        if s.active
            && s.local_port == dst_port
            && (s.remote_port == 0 || s.remote_port == src_port)
            && (s.remote_ip == [0u8; 4] || s.remote_ip == src_ip)
        {
            // Update GSR if this sequence is newer
            if seq > s.gsr {
                s.gsr = seq;
            }

            match pkt_type {
                DCCP_PKT_REQUEST => {
                    // Passive open: move to Responding
                    if s.state == DccpState::Listen {
                        s.state = DccpState::Responding;
                        s.remote_ip = src_ip;
                        s.remote_port = src_port;
                    }
                }
                DCCP_PKT_RESPONSE => {
                    // Completes active open
                    if s.state == DccpState::Requesting {
                        s.state = DccpState::Open;
                    }
                }
                DCCP_PKT_DATA | DCCP_PKT_DATAACK => {
                    // Enqueue payload into rx_buf (ring — newest data overwrites if full)
                    let avail = (4096u16).saturating_sub(s.rx_len) as usize;
                    let copy = payload.len().min(avail).min(4096);
                    let base = s.rx_len as usize;
                    let mut k = 0usize;
                    while k < copy {
                        if base.saturating_add(k) < 4096 {
                            s.rx_buf[base.saturating_add(k)] = payload[k];
                        }
                        k = k.saturating_add(1);
                    }
                    s.rx_len = s.rx_len.saturating_add(copy as u16);
                }
                DCCP_PKT_ACK => {
                    // Pure acknowledgment — no payload to enqueue
                }
                DCCP_PKT_CLOSEREQ => {
                    if s.state == DccpState::Open {
                        s.state = DccpState::CloseReq;
                    }
                }
                DCCP_PKT_CLOSE => {
                    s.state = DccpState::TimeWait;
                }
                DCCP_PKT_RESET => {
                    s.state = DccpState::Closed;
                    s.active = false;
                }
                _ => {}
            }
            return; // handled
        }
        i = i.saturating_add(1);
    }
    // No matching socket — silently drop (a full implementation would send RESET)
}

/// Initialise the DCCP subsystem.
pub fn init() {
    serial_println!("[dccp] DCCP protocol initialized");
}
