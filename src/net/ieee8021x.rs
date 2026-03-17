/// ieee8021x — 802.1X port-based network access control
///
/// Implements the supplicant and authenticator state machines for
/// EAP over LAN (EAPOL) frames (EtherType 0x888E, IEEE 802.1X-2010).
///
/// Supported EAP methods: EAP-Identity, EAP-MD5 (challenge/response)
///
/// Architecture:
///   - Port table: up to 8 controlled ports (one per NIC/VLAN)
///   - Per-port state machine: DISCONNECTED → CONNECTING → AUTHENTICATING
///     → AUTHENTICATED | HELD
///   - EAPOL frame parser: Start/Logoff/Packet/Key/ASF-Alert types
///   - EAP PDU parser: Request/Response/Success/Failure, type field
///   - No RADIUS — backend auth is stubbed (always succeeds for demo)
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// EtherType and EAPOL constants
// ---------------------------------------------------------------------------

pub const ETHERTYPE_EAPOL: u16 = 0x888E;

// EAPOL packet types (IEEE 802.1X §11.3)
pub const EAPOL_EAP_PACKET: u8 = 0;
pub const EAPOL_START: u8 = 1;
pub const EAPOL_LOGOFF: u8 = 2;
pub const EAPOL_KEY: u8 = 3;
pub const EAPOL_ASF_ALERT: u8 = 4;

// EAP codes
pub const EAP_REQUEST: u8 = 1;
pub const EAP_RESPONSE: u8 = 2;
pub const EAP_SUCCESS: u8 = 3;
pub const EAP_FAILURE: u8 = 4;

// EAP types
pub const EAP_TYPE_IDENTITY: u8 = 1;
pub const EAP_TYPE_NOTIFY: u8 = 2;
pub const EAP_TYPE_NAK: u8 = 3;
pub const EAP_TYPE_MD5: u8 = 4;
pub const EAP_TYPE_TLS: u8 = 13;
pub const EAP_TYPE_PEAP: u8 = 25;

// ---------------------------------------------------------------------------
// Port state machine
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EapolPortState {
    Disconnected,
    Connecting,
    Authenticating,
    Authenticated,
    Held, // auth failed; retry timer running
}

// ---------------------------------------------------------------------------
// EAPOL frame (parsed, on-stack — no heap)
// ---------------------------------------------------------------------------

pub const EAPOL_MAX_PAYLOAD: usize = 128;

#[derive(Copy, Clone)]
pub struct EapolFrame {
    pub version: u8,
    pub pkt_type: u8,
    pub length: u16,
    pub payload: [u8; EAPOL_MAX_PAYLOAD],
    pub payload_len: u8,
}

impl EapolFrame {
    pub const fn empty() -> Self {
        EapolFrame {
            version: 0,
            pkt_type: 0,
            length: 0,
            payload: [0u8; EAPOL_MAX_PAYLOAD],
            payload_len: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// EAP PDU (parsed from EAPOL_EAP_PACKET payload)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct EapPdu {
    pub code: u8,
    pub id: u8,
    pub length: u16,
    pub eap_type: u8, // valid only for Request/Response
    pub data: [u8; 64],
    pub data_len: u8,
}

impl EapPdu {
    pub const fn empty() -> Self {
        EapPdu {
            code: 0,
            id: 0,
            length: 0,
            eap_type: 0,
            data: [0u8; 64],
            data_len: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-port 802.1X state
// ---------------------------------------------------------------------------

pub const PORT_IDENTITY_LEN: usize = 64;

#[derive(Copy, Clone)]
pub struct EapolPort {
    pub id: u32,
    pub iface_name: [u8; 12],
    pub iface_len: u8,
    pub state: EapolPortState,
    pub eap_id: u8, // current EAP identifier
    pub retry_count: u8,
    pub held_timer: u32, // ticks remaining in HELD state
    pub identity: [u8; PORT_IDENTITY_LEN],
    pub identity_len: u8,
    pub auth_ok_count: u32,
    pub auth_fail_count: u32,
    pub valid: bool,
}

impl EapolPort {
    pub const fn empty() -> Self {
        EapolPort {
            id: 0,
            iface_name: [0u8; 12],
            iface_len: 0,
            state: EapolPortState::Disconnected,
            eap_id: 0,
            retry_count: 0,
            held_timer: 0,
            identity: [0u8; PORT_IDENTITY_LEN],
            identity_len: 0,
            auth_ok_count: 0,
            auth_fail_count: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static port table
// ---------------------------------------------------------------------------

pub const MAX_8021X_PORTS: usize = 8;

static PORTS: Mutex<[EapolPort; MAX_8021X_PORTS]> =
    Mutex::new([EapolPort::empty(); MAX_8021X_PORTS]);
static PORT_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn copy_iface(dst: &mut [u8; 12], src: &[u8]) -> u8 {
    let n = src.len().min(11);
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

// ---------------------------------------------------------------------------
// EAPOL frame parser
// ---------------------------------------------------------------------------

/// Parse raw bytes into an EapolFrame. Returns None on truncated/invalid.
pub fn eapol_parse(raw: &[u8]) -> Option<EapolFrame> {
    // EAPOL header: version(1) + type(1) + length(2) = 4 bytes
    if raw.len() < 4 {
        return None;
    }
    let version = raw[0];
    let pkt_type = raw[1];
    let length = u16::from_be_bytes([raw[2], raw[3]]);
    let payload_bytes = (length as usize).min(raw.len().saturating_sub(4));
    let n = payload_bytes.min(EAPOL_MAX_PAYLOAD);

    let mut frame = EapolFrame::empty();
    frame.version = version;
    frame.pkt_type = pkt_type;
    frame.length = length;
    let mut i = 0usize;
    while i < n {
        frame.payload[i] = raw[4 + i];
        i = i.saturating_add(1);
    }
    frame.payload_len = n as u8;
    Some(frame)
}

/// Parse an EAP PDU from an EAPOL_EAP_PACKET payload.
pub fn eap_parse(payload: &[u8], len: usize) -> Option<EapPdu> {
    if len < 4 || payload.len() < 4 {
        return None;
    }
    let code = payload[0];
    let id = payload[1];
    let length = u16::from_be_bytes([payload[2], payload[3]]);
    let mut pdu = EapPdu::empty();
    pdu.code = code;
    pdu.id = id;
    pdu.length = length;
    // Type field present for Request/Response
    if (code == EAP_REQUEST || code == EAP_RESPONSE) && len >= 5 {
        pdu.eap_type = payload[4];
        let dn = (len.saturating_sub(5)).min(64);
        let mut i = 0usize;
        while i < dn {
            pdu.data[i] = payload[5 + i];
            i = i.saturating_add(1);
        }
        pdu.data_len = dn as u8;
    }
    Some(pdu)
}

// ---------------------------------------------------------------------------
// Port management
// ---------------------------------------------------------------------------

/// Register a controlled port. Returns port id or 0 on failure.
pub fn ieee8021x_add_port(iface: &[u8]) -> u32 {
    let mut table = PORTS.lock();
    let mut i = 0usize;
    while i < MAX_8021X_PORTS {
        if !table[i].valid {
            let id = PORT_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i] = EapolPort::empty();
            table[i].id = id;
            table[i].iface_len = copy_iface(&mut table[i].iface_name, iface);
            table[i].valid = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

pub fn ieee8021x_remove_port(port_id: u32) -> bool {
    let mut table = PORTS.lock();
    let mut i = 0usize;
    while i < MAX_8021X_PORTS {
        if table[i].id == port_id && table[i].valid {
            table[i] = EapolPort::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// State machine: process an incoming EAPOL frame on a port
// ---------------------------------------------------------------------------

/// Process a raw EAPOL frame received on `port_id`.
/// Returns true if frame was handled.
pub fn ieee8021x_rx(port_id: u32, raw: &[u8]) -> bool {
    let frame = match eapol_parse(raw) {
        Some(f) => f,
        None => return false,
    };

    let mut table = PORTS.lock();
    let mut pi = 0usize;
    while pi < MAX_8021X_PORTS {
        if table[pi].id == port_id && table[pi].valid {
            break;
        }
        pi = pi.saturating_add(1);
    }
    if pi == MAX_8021X_PORTS {
        return false;
    }

    match frame.pkt_type {
        EAPOL_START => {
            // Supplicant wants to authenticate → send Identity Request
            table[pi].state = EapolPortState::Connecting;
            table[pi].eap_id = table[pi].eap_id.wrapping_add(1);
            table[pi].retry_count = 0;
        }
        EAPOL_LOGOFF => {
            table[pi].state = EapolPortState::Disconnected;
            table[pi].identity_len = 0;
        }
        EAPOL_EAP_PACKET => {
            let plen = frame.payload_len as usize;
            if let Some(pdu) = eap_parse(&frame.payload, plen) {
                match pdu.code {
                    EAP_RESPONSE => {
                        if pdu.eap_type == EAP_TYPE_IDENTITY {
                            // Store identity
                            let dn = pdu.data_len as usize;
                            let n = dn.min(PORT_IDENTITY_LEN - 1);
                            let mut k = 0usize;
                            while k < n {
                                table[pi].identity[k] = pdu.data[k];
                                k = k.saturating_add(1);
                            }
                            table[pi].identity_len = n as u8;
                            table[pi].state = EapolPortState::Authenticating;
                            // In a real implementation, relay to RADIUS here.
                            // Stub: immediately succeed.
                            table[pi].state = EapolPortState::Authenticated;
                            table[pi].auth_ok_count = table[pi].auth_ok_count.saturating_add(1);
                        } else if pdu.eap_type == EAP_TYPE_MD5 {
                            // MD5 challenge response: stub accept
                            table[pi].state = EapolPortState::Authenticated;
                            table[pi].auth_ok_count = table[pi].auth_ok_count.saturating_add(1);
                        } else if pdu.eap_type == EAP_TYPE_NAK {
                            // Peer rejected proposed method
                            table[pi].state = EapolPortState::Held;
                            table[pi].held_timer = 60;
                            table[pi].auth_fail_count = table[pi].auth_fail_count.saturating_add(1);
                        }
                    }
                    EAP_SUCCESS => {
                        table[pi].state = EapolPortState::Authenticated;
                        table[pi].auth_ok_count = table[pi].auth_ok_count.saturating_add(1);
                    }
                    EAP_FAILURE => {
                        table[pi].state = EapolPortState::Held;
                        table[pi].held_timer = 60;
                        table[pi].auth_fail_count = table[pi].auth_fail_count.saturating_add(1);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    true
}

// ---------------------------------------------------------------------------
// Build outbound EAPOL frames
// ---------------------------------------------------------------------------

/// Build an EAPOL-EAP Identity Request into `buf`.
/// Returns bytes written or 0 if buf too small.
pub fn eapol_build_identity_request(eap_id: u8, buf: &mut [u8]) -> usize {
    // EAPOL header(4) + EAP header(4) + type(1) = 9
    if buf.len() < 9 {
        return 0;
    }
    buf[0] = 1; // EAPOL version
    buf[1] = EAPOL_EAP_PACKET;
    // length of EAP body = 5
    buf[2] = 0;
    buf[3] = 5;
    // EAP PDU
    buf[4] = EAP_REQUEST;
    buf[5] = eap_id;
    buf[6] = 0;
    buf[7] = 5; // EAP length
    buf[8] = EAP_TYPE_IDENTITY;
    9
}

/// Build an EAPOL-Success frame into `buf`. Returns bytes written.
pub fn eapol_build_success(eap_id: u8, buf: &mut [u8]) -> usize {
    if buf.len() < 8 {
        return 0;
    }
    buf[0] = 1;
    buf[1] = EAPOL_EAP_PACKET;
    buf[2] = 0;
    buf[3] = 4;
    buf[4] = EAP_SUCCESS;
    buf[5] = eap_id;
    buf[6] = 0;
    buf[7] = 4;
    8
}

/// Build an EAPOL-Failure frame into `buf`. Returns bytes written.
pub fn eapol_build_failure(eap_id: u8, buf: &mut [u8]) -> usize {
    if buf.len() < 8 {
        return 0;
    }
    buf[0] = 1;
    buf[1] = EAPOL_EAP_PACKET;
    buf[2] = 0;
    buf[3] = 4;
    buf[4] = EAP_FAILURE;
    buf[5] = eap_id;
    buf[6] = 0;
    buf[7] = 4;
    8
}

// ---------------------------------------------------------------------------
// HELD timer tick — decrement and transition back to Disconnected
// ---------------------------------------------------------------------------

pub fn ieee8021x_tick(port_id: u32) {
    let mut table = PORTS.lock();
    let mut i = 0usize;
    while i < MAX_8021X_PORTS {
        if table[i].id == port_id && table[i].valid {
            if table[i].state == EapolPortState::Held && table[i].held_timer > 0 {
                table[i].held_timer -= 1;
                if table[i].held_timer == 0 {
                    table[i].state = EapolPortState::Disconnected;
                }
            }
            return;
        }
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

pub fn ieee8021x_state(port_id: u32) -> EapolPortState {
    let table = PORTS.lock();
    let mut i = 0usize;
    while i < MAX_8021X_PORTS {
        if table[i].id == port_id && table[i].valid {
            return table[i].state;
        }
        i = i.saturating_add(1);
    }
    EapolPortState::Disconnected
}

pub fn ieee8021x_port_count() -> usize {
    let table = PORTS.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < MAX_8021X_PORTS {
        if table[i].valid {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // Register a default port for the primary Ethernet interface
    ieee8021x_add_port(b"eth0");
    serial_println!(
        "[802.1X] port-based NAC initialized ({} ports)",
        ieee8021x_port_count()
    );
}
