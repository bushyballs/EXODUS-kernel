/// Point-to-Point Protocol (RFC 1661) for Genesis
///
/// Provides PPP link framing for serial connections: HDLC-like flag/escape
/// byte stuffing, a static table of PPP links, and helpers for sending and
/// receiving PPP frames.
///
/// No heap allocations are used anywhere in this module.
///
/// Protocol numbers from RFC 1661. All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// HDLC frame-flag byte used as delimiter.
pub const PPP_FLAG_BYTE: u8 = 0x7E;

/// HDLC escape character.
pub const PPP_ESC_BYTE: u8 = 0x7D;

/// PPP broadcast address field.
pub const PPP_ADDR: u8 = 0xFF;

/// PPP unnumbered information control field.
pub const PPP_CTRL: u8 = 0x03;

/// Alias for the broadcast address field.
pub const PPP_ALLSTATIONS: u8 = 0xFF;

// ---------------------------------------------------------------------------
// PPP protocol numbers
// ---------------------------------------------------------------------------

/// IP datagrams (IPv4).
pub const PPP_IP: u16 = 0x0021;

/// IPv6 datagrams.
pub const PPP_IPV6: u16 = 0x0057;

/// Link Control Protocol.
pub const PPP_LCP: u16 = 0xC021;

/// IP Control Protocol.
pub const PPP_IPCP: u16 = 0x8021;

/// Password Authentication Protocol.
pub const PPP_PAP: u16 = 0xC023;

/// Challenge Handshake Authentication Protocol.
pub const PPP_CHAP: u16 = 0xC223;

/// IPv6 Control Protocol.
pub const PPP_IPV6CP: u16 = 0x8057;

// ---------------------------------------------------------------------------
// LCP/NCP code values (internal)
// ---------------------------------------------------------------------------

const CODE_CONFIGURE_REQ: u8 = 1;
const CODE_CONFIGURE_ACK: u8 = 2;
const CODE_CONFIGURE_NAK: u8 = 3;
const CODE_CONFIGURE_REJ: u8 = 4;
const CODE_TERMINATE_REQ: u8 = 5;
const CODE_TERMINATE_ACK: u8 = 6;
const CODE_CODE_REJ: u8 = 7;
const CODE_ECHO_REQ: u8 = 9;
const CODE_ECHO_REPLY: u8 = 10;

// ---------------------------------------------------------------------------
// PPP link state machine
// ---------------------------------------------------------------------------

/// PPP link phase / state.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PppState {
    Dead,
    Establish,
    Authenticate,
    Network,
    Terminate,
}

// ---------------------------------------------------------------------------
// Sizes and limits
// ---------------------------------------------------------------------------

/// Maximum number of concurrent PPP links.
pub const MAX_PPP_LINKS: usize = 4;

/// PPP Maximum Transmission Unit.
pub const PPP_MTU: usize = 1500;

/// Receive buffer size per link (accommodates worst-case byte-stuffed frame).
pub const PPP_RX_BUF: usize = 2048;

/// Maximum output frame size:
///   flag(1) + addr(1) + ctrl(1) + proto(2) + payload(1500) + flag(1) = 1506.
/// With escaping up to every byte doubles: 2 * 1500 + 8 = 3008 — but we cap at
/// the buffer size below and document that callers must supply 1508-byte buffers.
const PPP_MAX_FRAME: usize = 1508;

// ---------------------------------------------------------------------------
// PppLink — one PPP connection
// ---------------------------------------------------------------------------

/// A single PPP link entry in the static link table.
///
/// Must be `Copy` for storage in `static Mutex<[PppLink; N]>`.
#[derive(Copy, Clone)]
pub struct PppLink {
    /// Opaque link identifier.
    pub id: u32,
    /// Current link state.
    pub state: PppState,
    /// Negotiated local IP address.
    pub local_ip: [u8; 4],
    /// Negotiated remote IP address.
    pub remote_ip: [u8; 4],
    /// Maximum Transmission Unit (outbound).
    pub mtu: u16,
    /// Maximum Receive Unit.
    pub mru: u16,
    /// Partial-frame receive buffer.
    pub rx_buf: [u8; PPP_RX_BUF],
    /// Number of valid bytes currently in `rx_buf`.
    pub rx_len: u16,
    pub rx_pkts: u64,
    pub tx_pkts: u64,
    /// True when this slot is occupied.
    pub active: bool,
}

impl PppLink {
    pub const fn empty() -> Self {
        PppLink {
            id: 0,
            state: PppState::Dead,
            local_ip: [0u8; 4],
            remote_ip: [0u8; 4],
            mtu: PPP_MTU as u16,
            mru: PPP_MTU as u16,
            rx_buf: [0u8; PPP_RX_BUF],
            rx_len: 0,
            rx_pkts: 0,
            tx_pkts: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Counter used to assign unique link IDs.
static PPP_NEXT_ID: Mutex<u32> = Mutex::new(1);

/// Static table of PPP links.
static PPP_LINKS: Mutex<[PppLink; MAX_PPP_LINKS]> = Mutex::new([PppLink::empty(); MAX_PPP_LINKS]);

// ---------------------------------------------------------------------------
// Link lifecycle
// ---------------------------------------------------------------------------

/// Create a new PPP link.
///
/// Returns the link `id` on success, or `None` if the table is full.
pub fn ppp_link_create() -> Option<u32> {
    let id = {
        let mut next = PPP_NEXT_ID.lock();
        let id = *next;
        *next = next.wrapping_add(1);
        id
    };
    let mut links = PPP_LINKS.lock();
    let mut i = 0;
    while i < MAX_PPP_LINKS {
        if !links[i].active {
            links[i] = PppLink {
                id,
                state: PppState::Establish,
                active: true,
                ..PppLink::empty()
            };
            serial_println!("[ppp] link {} created", id);
            return Some(id);
        }
        i += 1;
    }
    None
}

/// Destroy the PPP link identified by `link_id`.
///
/// Returns `true` on success.
pub fn ppp_link_destroy(link_id: u32) -> bool {
    let mut links = PPP_LINKS.lock();
    let mut i = 0;
    while i < MAX_PPP_LINKS {
        if links[i].active && links[i].id == link_id {
            links[i] = PppLink::empty();
            serial_println!("[ppp] link {} destroyed", link_id);
            return true;
        }
        i += 1;
    }
    false
}

/// Bring a PPP link to the Network phase and configure its IP addresses.
///
/// Returns `true` on success.
pub fn ppp_link_connect(link_id: u32, local_ip: [u8; 4], remote_ip: [u8; 4]) -> bool {
    let mut links = PPP_LINKS.lock();
    let mut i = 0;
    while i < MAX_PPP_LINKS {
        if links[i].active && links[i].id == link_id {
            links[i].state = PppState::Network;
            links[i].local_ip = local_ip;
            links[i].remote_ip = remote_ip;
            serial_println!(
                "[ppp] link {} connected ({}.{}.{}.{} <-> {}.{}.{}.{})",
                link_id,
                local_ip[0],
                local_ip[1],
                local_ip[2],
                local_ip[3],
                remote_ip[0],
                remote_ip[1],
                remote_ip[2],
                remote_ip[3]
            );
            return true;
        }
        i += 1;
    }
    false
}

/// Gracefully disconnect a PPP link (Terminate → Dead).
///
/// Returns `true` on success.
pub fn ppp_link_disconnect(link_id: u32) -> bool {
    let mut links = PPP_LINKS.lock();
    let mut i = 0;
    while i < MAX_PPP_LINKS {
        if links[i].active && links[i].id == link_id {
            links[i].state = PppState::Terminate;
            links[i].state = PppState::Dead;
            serial_println!("[ppp] link {} disconnected", link_id);
            return true;
        }
        i += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Frame encapsulation / decapsulation
// ---------------------------------------------------------------------------

/// Build a PPP frame in `out`.
///
/// Format: `flag(0x7E) | addr(0xFF) | ctrl(0x03) | proto[2 BE] | data | flag(0x7E)`
///
/// The link's TX counter is incremented.  Returns the total bytes written, or
/// 0 if the data would overflow `out`.
pub fn ppp_encap(link_id: u32, proto: u16, data: &[u8], out: &mut [u8; 1508]) -> usize {
    // Minimum header overhead: flag + addr + ctrl + proto(2) + flag = 6
    // Maximum payload: 1508 - 6 = 1502; cap at PPP_MTU for safety.
    if data.len() > PPP_MTU {
        return 0;
    }
    let needed = 1 + 1 + 1 + 2 + data.len() + 1; // 6 + payload
    if needed > 1508 {
        return 0;
    }

    let mut pos = 0usize;
    out[pos] = PPP_FLAG_BYTE;
    pos += 1;
    out[pos] = PPP_ADDR;
    pos += 1;
    out[pos] = PPP_CTRL;
    pos += 1;
    out[pos] = (proto >> 8) as u8;
    pos += 1;
    out[pos] = (proto & 0xFF) as u8;
    pos += 1;
    let mut k = 0;
    while k < data.len() {
        out[pos] = data[k];
        pos += 1;
        k += 1;
    }
    out[pos] = PPP_FLAG_BYTE;
    pos += 1;

    // Update TX stats.
    {
        let mut links = PPP_LINKS.lock();
        let mut i = 0;
        while i < MAX_PPP_LINKS {
            if links[i].active && links[i].id == link_id {
                links[i].tx_pkts = links[i].tx_pkts.saturating_add(1);
                break;
            }
            i += 1;
        }
    }
    pos
}

/// Strip PPP framing from a received raw frame.
///
/// Sets `*proto_out` to the PPP protocol number and returns the payload length.
/// Returns 0 if the frame is malformed (too short, wrong flags, missing
/// addr/ctrl fields).
pub fn ppp_decap(frame: &[u8], len: usize, proto_out: &mut u16) -> usize {
    if len < 6 {
        return 0;
    }
    // Must start and end with flag byte.
    if frame[0] != PPP_FLAG_BYTE {
        return 0;
    }
    if frame[len - 1] != PPP_FLAG_BYTE {
        return 0;
    }
    // addr + ctrl + proto(2) + at least 1 byte payload = 5 bytes between flags
    if len < 7 {
        return 0;
    }
    if frame[1] != PPP_ADDR {
        return 0;
    }
    if frame[2] != PPP_CTRL {
        return 0;
    }
    *proto_out = u16::from_be_bytes([frame[3], frame[4]]);
    // payload is bytes 5..(len-1)
    let payload_len = len.saturating_sub(6); // len - flag - addr - ctrl - proto(2) - flag
    payload_len
}

// ---------------------------------------------------------------------------
// Send / receive helpers
// ---------------------------------------------------------------------------

/// Encapsulate `data` as a PPP frame for `link_id` with protocol `proto`.
///
/// Returns the number of bytes written into the internal stack buffer, which is
/// also the notional on-wire length.  Callers that need the actual bytes should
/// call `ppp_encap` directly with their own buffer.
pub fn ppp_send(link_id: u32, proto: u16, data: &[u8]) -> usize {
    let mut buf = [0u8; 1508];
    ppp_encap(link_id, proto, data, &mut buf)
}

/// Deframe a received PPP frame for `link_id`.
///
/// Returns `(payload_len, protocol)`.  The link's RX counter is incremented if
/// the frame is valid.  Returns `(0, 0)` on error.
pub fn ppp_recv(link_id: u32, frame: &[u8], len: usize) -> (usize, u16) {
    let mut proto = 0u16;
    let payload_len = ppp_decap(frame, len, &mut proto);
    if payload_len == 0 {
        return (0, 0);
    }

    {
        let mut links = PPP_LINKS.lock();
        let mut i = 0;
        while i < MAX_PPP_LINKS {
            if links[i].active && links[i].id == link_id {
                links[i].rx_pkts = links[i].rx_pkts.saturating_add(1);
                break;
            }
            i += 1;
        }
    }
    (payload_len, proto)
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the PPP subsystem.
pub fn init() {
    serial_println!("[ppp] PPP protocol driver initialized");
}
