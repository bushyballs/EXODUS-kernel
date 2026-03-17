/// pppoe — PPPoE client (RFC 2516)
///
/// Implements the discovery and session phases for PPPoE:
///   Discovery: PADI → PADO → PADR → PADS → PADT
///   Session:   encapsulate/decapsulate PPP frames in PPPoE session frames
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// PPPoE constants (RFC 2516)
// ---------------------------------------------------------------------------

pub const PPPOE_VER_TYPE: u8 = 0x11; // version=1, type=1
pub const PPPOE_CODE_PADI: u8 = 0x09;
pub const PPPOE_CODE_PADO: u8 = 0x07;
pub const PPPOE_CODE_PADR: u8 = 0x19;
pub const PPPOE_CODE_PADS: u8 = 0x65;
pub const PPPOE_CODE_PADT: u8 = 0xA7;
pub const PPPOE_CODE_SESSION: u8 = 0x00; // in-session data

pub const ETHERTYPE_DISCOVERY: u16 = 0x8863;
pub const ETHERTYPE_SESSION: u16 = 0x8864;

// PPPoE tag types
pub const TAG_SERVICE_NAME: u16 = 0x0101;
pub const TAG_AC_NAME: u16 = 0x0102;
pub const TAG_AC_COOKIE: u16 = 0x0104;
pub const TAG_RELAY_ID: u16 = 0x0110;
pub const TAG_HOST_UNIQ: u16 = 0x0103;
pub const TAG_END_OF_LIST: u16 = 0x0000;

const MAX_SESSIONS: usize = 8;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum PppoeState {
    Idle,
    SendingPadi,
    WaitingPado,
    SendingPadr,
    WaitingPads,
    Connected,
    Terminating,
}

#[derive(Copy, Clone)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const fn zero() -> Self {
        MacAddr([0u8; 6])
    }
    pub const fn broadcast() -> Self {
        MacAddr([0xFF; 6])
    }
}

#[derive(Copy, Clone)]
pub struct PppoeSession {
    pub id: u32,         // local handle
    pub session_id: u16, // PPPoE session ID from PADS
    pub state: PppoeState,
    pub local_mac: MacAddr,
    pub server_mac: MacAddr,
    pub cookie: [u8; 32], // AC-Cookie tag
    pub cookie_len: u8,
    pub host_uniq: u32, // our nonce for matching replies
    pub active: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl PppoeSession {
    pub const fn empty() -> Self {
        PppoeSession {
            id: 0,
            session_id: 0,
            state: PppoeState::Idle,
            local_mac: MacAddr::zero(),
            server_mac: MacAddr::zero(),
            cookie: [0u8; 32],
            cookie_len: 0,
            host_uniq: 0,
            active: false,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }
}

const EMPTY_SESSION: PppoeSession = PppoeSession::empty();
static SESSIONS: Mutex<[PppoeSession; MAX_SESSIONS]> = Mutex::new([EMPTY_SESSION; MAX_SESSIONS]);
static SESSION_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Frame builder helpers
// ---------------------------------------------------------------------------

/// Write a big-endian u16 into buf at offset. Returns new offset.
fn write_u16_be(buf: &mut [u8], offset: usize, val: u16) -> usize {
    if offset.saturating_add(2) > buf.len() {
        return offset;
    }
    buf[offset] = (val >> 8) as u8;
    buf[offset + 1] = (val & 0xFF) as u8;
    offset.saturating_add(2)
}

/// Write a PPPoE tag into buf at offset.
fn write_tag(buf: &mut [u8], offset: usize, tag_type: u16, data: &[u8]) -> usize {
    let len = data.len();
    let mut off = write_u16_be(buf, offset, tag_type);
    off = write_u16_be(buf, off, len as u16);
    let end = off.saturating_add(len);
    if end <= buf.len() {
        let mut i = 0usize;
        while i < len {
            buf[off + i] = data[i];
            i = i.saturating_add(1);
        }
    }
    off.saturating_add(len)
}

/// Build a PPPoE discovery header into buf[0..6]. Returns 6.
fn write_pppoe_hdr(buf: &mut [u8], code: u8, session_id: u16, payload_len: u16) -> usize {
    if buf.len() < 6 {
        return 0;
    }
    buf[0] = PPPOE_VER_TYPE;
    buf[1] = code;
    let off = write_u16_be(buf, 2, session_id);
    write_u16_be(buf, off, payload_len);
    6
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new PPPoE client session. Returns session handle id.
pub fn pppoe_create(local_mac: MacAddr) -> Option<u32> {
    let id = SESSION_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if !sessions[i].active {
            sessions[i] = PppoeSession::empty();
            sessions[i].id = id;
            sessions[i].local_mac = local_mac;
            sessions[i].host_uniq = id; // use id as nonce
            sessions[i].state = PppoeState::Idle;
            sessions[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Build a PADI frame into buf. Returns frame length, or 0 on error.
/// The caller is responsible for prepending the Ethernet header.
pub fn pppoe_build_padi(id: u32, buf: &mut [u8; 256]) -> usize {
    let sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            // PADI: dst=broadcast, session_id=0, tags: service-name + host-uniq
            let host_uniq_bytes = sessions[i].host_uniq.to_be_bytes();
            let mut payload = [0u8; 64];
            let mut poff = 0usize;
            poff = write_tag(&mut payload, poff, TAG_SERVICE_NAME, b""); // any service
            poff = write_tag(&mut payload, poff, TAG_HOST_UNIQ, &host_uniq_bytes);
            poff = write_tag(&mut payload, poff, TAG_END_OF_LIST, b"");
            let payload_len = poff;

            let mut off = write_pppoe_hdr(buf, PPPOE_CODE_PADI, 0, payload_len as u16);
            let mut k = 0usize;
            while k < payload_len && off < 256 {
                buf[off] = payload[k];
                off = off.saturating_add(1);
                k = k.saturating_add(1);
            }
            return off;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Process a received PADO frame (server offer). Extracts AC-Cookie.
/// buf should be the PPPoE payload (after Ethernet + PPPoE header).
pub fn pppoe_recv_pado(id: u32, server_mac: MacAddr, payload: &[u8]) -> bool {
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active
            && sessions[i].id == id
            && sessions[i].state == PppoeState::WaitingPado
        {
            sessions[i].server_mac = server_mac;
            // Parse tags to extract AC-Cookie
            let mut off = 0usize;
            while off.saturating_add(4) <= payload.len() {
                let tag_type = ((payload[off] as u16) << 8) | payload[off + 1] as u16;
                let tag_len = ((payload[off + 2] as u16) << 8) | payload[off + 3] as u16;
                off = off.saturating_add(4);
                if tag_type == TAG_AC_COOKIE {
                    let clen = (tag_len as usize).min(32);
                    let mut k = 0usize;
                    while k < clen && off.saturating_add(k) < payload.len() {
                        sessions[i].cookie[k] = payload[off + k];
                        k = k.saturating_add(1);
                    }
                    sessions[i].cookie_len = clen as u8;
                }
                if tag_type == TAG_END_OF_LIST {
                    break;
                }
                off = off.saturating_add(tag_len as usize);
            }
            sessions[i].state = PppoeState::SendingPadr;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Build a PADR frame into buf. Returns frame length, or 0 on error.
pub fn pppoe_build_padr(id: u32, buf: &mut [u8; 256]) -> usize {
    let sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            let host_uniq_bytes = sessions[i].host_uniq.to_be_bytes();
            let cookie_len = sessions[i].cookie_len as usize;
            let mut payload = [0u8; 128];
            let mut poff = 0usize;
            poff = write_tag(&mut payload, poff, TAG_SERVICE_NAME, b"");
            poff = write_tag(&mut payload, poff, TAG_HOST_UNIQ, &host_uniq_bytes);
            if cookie_len > 0 {
                let cookie_copy = sessions[i].cookie;
                poff = write_tag(
                    &mut payload,
                    poff,
                    TAG_AC_COOKIE,
                    &cookie_copy[..cookie_len],
                );
            }
            poff = write_tag(&mut payload, poff, TAG_END_OF_LIST, b"");
            let payload_len = poff;

            let mut off = write_pppoe_hdr(buf, PPPOE_CODE_PADR, 0, payload_len as u16);
            let mut k = 0usize;
            while k < payload_len && off < 256 {
                buf[off] = payload[k];
                off = off.saturating_add(1);
                k = k.saturating_add(1);
            }
            return off;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Process a received PADS frame (session confirmed). Records session_id.
pub fn pppoe_recv_pads(id: u32, session_id: u16) -> bool {
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active
            && sessions[i].id == id
            && sessions[i].state == PppoeState::WaitingPads
        {
            sessions[i].session_id = session_id;
            sessions[i].state = PppoeState::Connected;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Encapsulate a PPP frame (ppp_payload) into a PPPoE session frame.
/// Returns total bytes written, or 0 on error.
pub fn pppoe_encap(id: u32, ppp_payload: &[u8], out: &mut [u8; 1514]) -> usize {
    let sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id && sessions[i].state == PppoeState::Connected
        {
            let payload_len = ppp_payload.len().min(1500);
            let mut off = write_pppoe_hdr(
                out,
                PPPOE_CODE_SESSION,
                sessions[i].session_id,
                payload_len as u16,
            );
            let mut k = 0usize;
            while k < payload_len && off < 1514 {
                out[off] = ppp_payload[k];
                off = off.saturating_add(1);
                k = k.saturating_add(1);
            }
            return off;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Decapsulate a received PPPoE session frame. Returns (session_id, ppp_data_offset, ppp_data_len).
pub fn pppoe_decap(frame: &[u8]) -> Option<(u16, usize, usize)> {
    if frame.len() < 6 {
        return None;
    }
    if frame[0] != PPPOE_VER_TYPE || frame[1] != PPPOE_CODE_SESSION {
        return None;
    }
    let session_id = ((frame[2] as u16) << 8) | frame[3] as u16;
    let payload_len = ((frame[4] as u16) << 8) | frame[5] as u16;
    let data_end = (6usize)
        .saturating_add(payload_len as usize)
        .min(frame.len());
    Some((session_id, 6, data_end - 6))
}

/// Build a PADT (terminate session) frame into buf. Returns frame length.
pub fn pppoe_build_padt(id: u32, buf: &mut [u8; 64]) -> usize {
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            let session_id = sessions[i].session_id;
            sessions[i].state = PppoeState::Terminating;
            let mut payload = [0u8; 4];
            let poff = write_tag(&mut payload, 0, TAG_END_OF_LIST, b"");
            let mut off = write_pppoe_hdr(buf, PPPOE_CODE_PADT, session_id, poff as u16);
            let mut k = 0usize;
            while k < poff && off < 64 {
                buf[off] = payload[k];
                off = off.saturating_add(1);
                k = k.saturating_add(1);
            }
            return off;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Close and free a PPPoE session.
pub fn pppoe_close(id: u32) -> bool {
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            sessions[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Advance state machine: Idle→SendingPadi, SendingPadr→WaitingPads, etc.
pub fn pppoe_advance(id: u32) {
    let mut sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            sessions[i].state = match sessions[i].state {
                PppoeState::Idle => PppoeState::SendingPadi,
                PppoeState::SendingPadi => PppoeState::WaitingPado,
                PppoeState::SendingPadr => PppoeState::WaitingPads,
                other => other,
            };
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn pppoe_get_state(id: u32) -> Option<PppoeState> {
    let sessions = SESSIONS.lock();
    let mut i = 0usize;
    while i < MAX_SESSIONS {
        if sessions[i].active && sessions[i].id == id {
            return Some(sessions[i].state);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn init() {
    serial_println!(
        "[pppoe] PPPoE client initialized (max {} sessions)",
        MAX_SESSIONS
    );
}
