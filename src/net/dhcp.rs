use crate::net::Ipv4Addr;
/// DHCP client for Genesis — Dynamic Host Configuration Protocol
///
/// Implements: DHCP discover, offer, request, acknowledge flow.
/// Handles: IP address, subnet mask, gateway, DNS servers, lease time.
///
/// Features:
///   - Full DHCP state machine: INIT -> SELECTING -> REQUESTING -> BOUND ->
///     RENEWING -> REBINDING
///   - DHCP discover/offer/request/ack packet building
///   - Option parsing: subnet mask, router, DNS, lease time, server ID,
///     domain name, broadcast address
///   - Lease tracking with renewal timer (T1=50%, T2=87.5% using integer fractions)
///   - DHCP release on shutdown
///   - Auto-configure interface IP/mask/gateway from DHCP response
///
/// Inspired by: RFC 2131. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// BOOTP opcodes
const BOOTREQUEST: u8 = 1;
const BOOTREPLY: u8 = 2;

/// Hardware type: Ethernet
const HTYPE_ETHERNET: u8 = 1;

/// DHCP magic cookie: 99.130.83.99
const MAGIC_COOKIE: [u8; 4] = [99, 130, 83, 99];

/// DHCP ports
const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

/// Maximum DHCP packet size
const MAX_DHCP_SIZE: usize = 576;

/// Minimum BOOTP message size (excluding options)
const BOOTP_MIN_SIZE: usize = 236;

/// Default lease time if server doesn't provide one (1 day)
const DEFAULT_LEASE_SECS: u32 = 86400;

/// Maximum retransmit attempts per state
const MAX_RETRANSMITS: u32 = 5;

/// Initial retransmit interval in ticks (ms) — 4 seconds
const INITIAL_RETRANSMIT_MS: u64 = 4_000;

/// Maximum retransmit interval in ticks (ms) — 64 seconds
const MAX_RETRANSMIT_MS: u64 = 64_000;

// ---------------------------------------------------------------------------
// DHCP message types (option 53)
// ---------------------------------------------------------------------------

/// DHCP message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl DhcpMessageType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(DhcpMessageType::Discover),
            2 => Some(DhcpMessageType::Offer),
            3 => Some(DhcpMessageType::Request),
            4 => Some(DhcpMessageType::Decline),
            5 => Some(DhcpMessageType::Ack),
            6 => Some(DhcpMessageType::Nak),
            7 => Some(DhcpMessageType::Release),
            8 => Some(DhcpMessageType::Inform),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// DHCP option codes
// ---------------------------------------------------------------------------

const OPT_PAD: u8 = 0;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_HOSTNAME: u8 = 12;
const OPT_DOMAIN_NAME: u8 = 15;
const OPT_BROADCAST: u8 = 28;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MSG_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAM_REQ_LIST: u8 = 55;
const OPT_RENEWAL_TIME: u8 = 58;
const OPT_REBINDING_TIME: u8 = 59;
const OPT_END: u8 = 255;

// ---------------------------------------------------------------------------
// DHCP client state
// ---------------------------------------------------------------------------

/// DHCP client state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    /// Not started, no lease
    Init,
    /// DISCOVER sent, waiting for OFFER
    Selecting,
    /// REQUEST sent, waiting for ACK
    Requesting,
    /// Have a valid lease
    Bound,
    /// T1 expired, unicast REQUEST to renew
    Renewing,
    /// T2 expired, broadcast REQUEST to renew
    Rebinding,
    /// Lease released
    Released,
}

// ---------------------------------------------------------------------------
// DHCP lease
// ---------------------------------------------------------------------------

/// DHCP lease information
#[derive(Clone)]
pub struct DhcpLease {
    /// Assigned IP address
    pub ip: [u8; 4],
    /// Subnet mask
    pub subnet_mask: [u8; 4],
    /// Default gateway
    pub gateway: [u8; 4],
    /// DNS server addresses
    pub dns_servers: Vec<[u8; 4]>,
    /// Domain name
    pub domain: String,
    /// DHCP server IP (for renewals)
    pub server_ip: [u8; 4],
    /// Broadcast address
    pub broadcast: [u8; 4],
    /// Lease duration in seconds
    pub lease_time: u32,
    /// T1: renewal time in seconds (default: 50% of lease)
    pub renewal_time: u32,
    /// T2: rebinding time in seconds (default: 87.5% of lease)
    pub rebind_time: u32,
    /// Uptime (seconds) when lease was obtained
    pub obtained_at: u64,
}

impl DhcpLease {
    /// Check if the lease has expired.
    fn is_expired(&self, now_secs: u64) -> bool {
        now_secs.saturating_sub(self.obtained_at) >= self.lease_time as u64
    }

    /// Check if T1 (renewal time) has elapsed.
    fn needs_renewal(&self, now_secs: u64) -> bool {
        now_secs.saturating_sub(self.obtained_at) >= self.renewal_time as u64
    }

    /// Check if T2 (rebinding time) has elapsed.
    fn needs_rebinding(&self, now_secs: u64) -> bool {
        now_secs.saturating_sub(self.obtained_at) >= self.rebind_time as u64
    }

    /// Time remaining on the lease in seconds.
    fn remaining_secs(&self, now_secs: u64) -> u64 {
        let elapsed = now_secs.saturating_sub(self.obtained_at);
        (self.lease_time as u64).saturating_sub(elapsed)
    }

    /// Format the IP as a string.
    pub fn ip_str(&self) -> String {
        format!(
            "{}.{}.{}.{}",
            self.ip[0], self.ip[1], self.ip[2], self.ip[3]
        )
    }
}

// ---------------------------------------------------------------------------
// Parsed DHCP offer/ack
// ---------------------------------------------------------------------------

/// Parsed data from a DHCP OFFER or ACK message
struct ParsedDhcpResponse {
    msg_type: DhcpMessageType,
    your_ip: [u8; 4],
    server_ip: [u8; 4],
    subnet_mask: [u8; 4],
    gateway: [u8; 4],
    dns_servers: Vec<[u8; 4]>,
    domain: String,
    broadcast: [u8; 4],
    lease_time: u32,
    renewal_time: Option<u32>,
    rebind_time: Option<u32>,
    server_id: [u8; 4],
}

// ---------------------------------------------------------------------------
// DHCP client
// ---------------------------------------------------------------------------

/// DHCP client
pub struct DhcpClient {
    /// Current state
    pub state: DhcpState,
    /// Our MAC address
    pub mac_addr: [u8; 6],
    /// Current lease
    pub lease: Option<DhcpLease>,
    /// Transaction ID (XID)
    pub transaction_id: u32,
    /// Interface name
    pub interface: String,
    /// Number of retransmissions in current state
    retransmit_count: u32,
    /// Tick when we last sent a message
    last_send_tick: u64,
    /// Current retransmit interval (doubles each time)
    retransmit_interval: u64,
    /// Offered IP from the most recent OFFER (before we REQUEST it)
    offered_ip: [u8; 4],
    /// Server IP from the most recent OFFER
    offered_server_ip: [u8; 4],
}

impl DhcpClient {
    const fn new() -> Self {
        DhcpClient {
            state: DhcpState::Init,
            mac_addr: [0; 6],
            lease: None,
            transaction_id: 0,
            interface: String::new(),
            retransmit_count: 0,
            last_send_tick: 0,
            retransmit_interval: INITIAL_RETRANSMIT_MS,
            offered_ip: [0; 4],
            offered_server_ip: [0; 4],
        }
    }

    /// Generate a new transaction ID using TSC.
    fn new_xid(&mut self) {
        let low: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") low, out("edx") _);
        }
        self.transaction_id = low;
    }

    /// Reset retransmit state.
    fn reset_retransmit(&mut self) {
        self.retransmit_count = 0;
        self.retransmit_interval = INITIAL_RETRANSMIT_MS;
        self.last_send_tick = crate::time::clock::uptime_ms();
    }

    // ----- Packet building -----

    /// Build the common BOOTP header portion.
    fn build_bootp_header(&self, ciaddr: [u8; 4]) -> Vec<u8> {
        let mut pkt = Vec::with_capacity(MAX_DHCP_SIZE);

        pkt.push(BOOTREQUEST); // op
        pkt.push(HTYPE_ETHERNET); // htype
        pkt.push(6); // hlen
        pkt.push(0); // hops

        // xid (4 bytes)
        pkt.extend_from_slice(&self.transaction_id.to_be_bytes());

        // secs (2 bytes)
        pkt.extend_from_slice(&[0, 0]);

        // flags: broadcast (2 bytes)
        pkt.extend_from_slice(&[0x80, 0x00]);

        // ciaddr (4 bytes) — client IP (used in RENEWING)
        pkt.extend_from_slice(&ciaddr);

        // yiaddr (4 bytes)
        pkt.extend_from_slice(&[0, 0, 0, 0]);

        // siaddr (4 bytes)
        pkt.extend_from_slice(&[0, 0, 0, 0]);

        // giaddr (4 bytes)
        pkt.extend_from_slice(&[0, 0, 0, 0]);

        // chaddr (16 bytes, MAC + padding)
        pkt.extend_from_slice(&self.mac_addr);
        pkt.extend_from_slice(&[0; 10]);

        // sname (64 bytes) + file (128 bytes) = 192 bytes of zeros
        pkt.extend_from_slice(&[0; 192]);

        // Magic cookie
        pkt.extend_from_slice(&MAGIC_COOKIE);

        pkt
    }

    /// Build a DHCP discover packet.
    pub fn build_discover(&self) -> Vec<u8> {
        let mut pkt = self.build_bootp_header([0, 0, 0, 0]);

        // Option 53: DHCP Message Type = DISCOVER
        pkt.extend_from_slice(&[OPT_MSG_TYPE, 1, DhcpMessageType::Discover as u8]);

        // Option 55: Parameter Request List
        pkt.extend_from_slice(&[
            OPT_PARAM_REQ_LIST,
            7,
            OPT_SUBNET_MASK,
            OPT_ROUTER,
            OPT_DNS,
            OPT_DOMAIN_NAME,
            OPT_BROADCAST,
            OPT_LEASE_TIME,
            OPT_RENEWAL_TIME,
        ]);

        // Option 12: Hostname
        let hostname = b"genesis";
        pkt.push(OPT_HOSTNAME);
        pkt.push(hostname.len() as u8);
        pkt.extend_from_slice(hostname);

        // End
        pkt.push(OPT_END);

        // Pad to minimum BOOTP size
        while pkt.len() < 300 {
            pkt.push(0);
        }

        pkt
    }

    /// Build a DHCP request packet.
    fn build_request(&self, server_ip: [u8; 4], requested_ip: [u8; 4]) -> Vec<u8> {
        let mut pkt = self.build_bootp_header([0, 0, 0, 0]);

        // Option 53: DHCP Request
        pkt.extend_from_slice(&[OPT_MSG_TYPE, 1, DhcpMessageType::Request as u8]);

        // Option 50: Requested IP Address
        pkt.push(OPT_REQUESTED_IP);
        pkt.push(4);
        pkt.extend_from_slice(&requested_ip);

        // Option 54: Server Identifier
        pkt.push(OPT_SERVER_ID);
        pkt.push(4);
        pkt.extend_from_slice(&server_ip);

        // Option 55: Parameter Request List
        pkt.extend_from_slice(&[
            OPT_PARAM_REQ_LIST,
            7,
            OPT_SUBNET_MASK,
            OPT_ROUTER,
            OPT_DNS,
            OPT_DOMAIN_NAME,
            OPT_BROADCAST,
            OPT_LEASE_TIME,
            OPT_RENEWAL_TIME,
        ]);

        // End
        pkt.push(OPT_END);

        while pkt.len() < 300 {
            pkt.push(0);
        }

        pkt
    }

    /// Build a DHCP request for renewal (unicast, with ciaddr set).
    fn build_renewal_request(&self) -> Option<Vec<u8>> {
        let lease = self.lease.as_ref()?;

        let mut pkt = self.build_bootp_header(lease.ip);

        // Option 53: DHCP Request
        pkt.extend_from_slice(&[OPT_MSG_TYPE, 1, DhcpMessageType::Request as u8]);

        // Option 55: Parameter Request List
        pkt.extend_from_slice(&[
            OPT_PARAM_REQ_LIST,
            4,
            OPT_SUBNET_MASK,
            OPT_ROUTER,
            OPT_DNS,
            OPT_LEASE_TIME,
        ]);

        // End
        pkt.push(OPT_END);

        while pkt.len() < 300 {
            pkt.push(0);
        }

        Some(pkt)
    }

    /// Build a DHCP release packet.
    fn build_release(&self) -> Option<Vec<u8>> {
        let lease = self.lease.as_ref()?;

        let mut pkt = self.build_bootp_header(lease.ip);

        // Option 53: DHCP Release
        pkt.extend_from_slice(&[OPT_MSG_TYPE, 1, DhcpMessageType::Release as u8]);

        // Option 54: Server Identifier
        pkt.push(OPT_SERVER_ID);
        pkt.push(4);
        pkt.extend_from_slice(&lease.server_ip);

        // End
        pkt.push(OPT_END);

        Some(pkt)
    }

    /// Build a DHCP decline packet (IP conflict detected).
    fn build_decline(&self, ip: [u8; 4], server_ip: [u8; 4]) -> Vec<u8> {
        let mut pkt = self.build_bootp_header([0, 0, 0, 0]);

        // Option 53: DHCP Decline
        pkt.extend_from_slice(&[OPT_MSG_TYPE, 1, DhcpMessageType::Decline as u8]);

        // Option 50: Requested IP
        pkt.push(OPT_REQUESTED_IP);
        pkt.push(4);
        pkt.extend_from_slice(&ip);

        // Option 54: Server Identifier
        pkt.push(OPT_SERVER_ID);
        pkt.push(4);
        pkt.extend_from_slice(&server_ip);

        // End
        pkt.push(OPT_END);

        pkt
    }

    // ----- Response parsing -----

    /// Parse a DHCP response packet.
    fn parse_response_inner(&self, data: &[u8]) -> Option<ParsedDhcpResponse> {
        if data.len() < BOOTP_MIN_SIZE + 4 {
            return None; // Too short for BOOTP + magic cookie
        }

        // Check opcode is BOOTREPLY
        if data[0] != BOOTREPLY {
            return None;
        }

        // Check transaction ID
        let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if xid != self.transaction_id {
            return None;
        }

        // Check magic cookie
        if data[236..240] != MAGIC_COOKIE {
            return None;
        }

        let your_ip = [data[16], data[17], data[18], data[19]];
        let server_ip_bootp = [data[20], data[21], data[22], data[23]];

        // Parse options
        let mut msg_type: Option<DhcpMessageType> = None;
        let mut subnet_mask = [255, 255, 255, 0];
        let mut gateway = [0u8; 4];
        let mut dns_servers = Vec::new();
        let mut domain = String::new();
        let mut broadcast = [255, 255, 255, 255];
        let mut lease_time = DEFAULT_LEASE_SECS;
        let mut renewal_time: Option<u32> = None;
        let mut rebind_time: Option<u32> = None;
        let mut server_id = server_ip_bootp;

        let mut i = 240;
        while i < data.len() {
            let opt = data[i];
            if opt == OPT_END {
                break;
            }
            if opt == OPT_PAD {
                i += 1;
                continue;
            }
            if i + 1 >= data.len() {
                break;
            }
            let len = data[i + 1] as usize;
            if i + 2 + len > data.len() {
                break;
            }
            let val = &data[i + 2..i + 2 + len];

            match opt {
                OPT_MSG_TYPE => {
                    if !val.is_empty() {
                        msg_type = DhcpMessageType::from_u8(val[0]);
                    }
                }
                OPT_SUBNET_MASK => {
                    if val.len() >= 4 {
                        subnet_mask = [val[0], val[1], val[2], val[3]];
                    }
                }
                OPT_ROUTER => {
                    if val.len() >= 4 {
                        gateway = [val[0], val[1], val[2], val[3]];
                    }
                }
                OPT_DNS => {
                    let mut j = 0;
                    while j + 3 < val.len() {
                        dns_servers.push([val[j], val[j + 1], val[j + 2], val[j + 3]]);
                        j = j.saturating_add(4);
                    }
                }
                OPT_DOMAIN_NAME => {
                    if let Ok(s) = core::str::from_utf8(val) {
                        domain = String::from(s);
                    }
                }
                OPT_BROADCAST => {
                    if val.len() >= 4 {
                        broadcast = [val[0], val[1], val[2], val[3]];
                    }
                }
                OPT_LEASE_TIME => {
                    if val.len() >= 4 {
                        lease_time = u32::from_be_bytes([val[0], val[1], val[2], val[3]]);
                    }
                }
                OPT_RENEWAL_TIME => {
                    if val.len() >= 4 {
                        renewal_time = Some(u32::from_be_bytes([val[0], val[1], val[2], val[3]]));
                    }
                }
                OPT_REBINDING_TIME => {
                    if val.len() >= 4 {
                        rebind_time = Some(u32::from_be_bytes([val[0], val[1], val[2], val[3]]));
                    }
                }
                OPT_SERVER_ID => {
                    if val.len() >= 4 {
                        server_id = [val[0], val[1], val[2], val[3]];
                    }
                }
                _ => {} // Ignore unknown options
            }

            i = i.saturating_add(2 + len);
        }

        let msg_type = msg_type?;

        Some(ParsedDhcpResponse {
            msg_type,
            your_ip,
            server_ip: server_id,
            subnet_mask,
            gateway,
            dns_servers,
            domain,
            broadcast,
            lease_time,
            renewal_time,
            rebind_time,
            server_id,
        })
    }

    /// Process a DHCP response. Returns the new state and optional packet to send.
    pub fn process_response(&mut self, data: &[u8]) -> Option<DhcpState> {
        let parsed = self.parse_response_inner(data)?;

        match (self.state, parsed.msg_type) {
            (DhcpState::Selecting, DhcpMessageType::Offer) => {
                // Got an OFFER — remember it and transition to REQUESTING
                self.offered_ip = parsed.your_ip;
                self.offered_server_ip = parsed.server_id;
                self.state = DhcpState::Requesting;
                self.reset_retransmit();

                crate::serial_println!(
                    "  DHCP: OFFER ip={}.{}.{}.{} from server {}.{}.{}.{}",
                    parsed.your_ip[0],
                    parsed.your_ip[1],
                    parsed.your_ip[2],
                    parsed.your_ip[3],
                    parsed.server_id[0],
                    parsed.server_id[1],
                    parsed.server_id[2],
                    parsed.server_id[3]
                );

                Some(DhcpState::Requesting)
            }

            (DhcpState::Requesting, DhcpMessageType::Ack)
            | (DhcpState::Renewing, DhcpMessageType::Ack)
            | (DhcpState::Rebinding, DhcpMessageType::Ack) => {
                // Got an ACK — lease is now valid
                let now_secs = crate::time::clock::uptime_ms() / 1000;

                // Compute T1 and T2 using integer fractions
                // T1 = lease_time / 2 (50%)
                // T2 = lease_time * 7 / 8 (87.5%)
                let t1 = parsed.renewal_time.unwrap_or(parsed.lease_time / 2);
                let t2 = parsed.rebind_time.unwrap_or(parsed.lease_time * 7 / 8);

                self.lease = Some(DhcpLease {
                    ip: parsed.your_ip,
                    subnet_mask: parsed.subnet_mask,
                    gateway: parsed.gateway,
                    dns_servers: parsed.dns_servers.clone(),
                    domain: parsed.domain.clone(),
                    server_ip: parsed.server_id,
                    broadcast: parsed.broadcast,
                    lease_time: parsed.lease_time,
                    renewal_time: t1,
                    rebind_time: t2,
                    obtained_at: now_secs,
                });

                self.state = DhcpState::Bound;
                self.reset_retransmit();

                crate::serial_println!(
                    "  DHCP: BOUND ip={}.{}.{}.{} lease={}s T1={}s T2={}s",
                    parsed.your_ip[0],
                    parsed.your_ip[1],
                    parsed.your_ip[2],
                    parsed.your_ip[3],
                    parsed.lease_time,
                    t1,
                    t2
                );

                // Auto-configure the interface
                self.configure_interface(&parsed);

                Some(DhcpState::Bound)
            }

            (DhcpState::Requesting, DhcpMessageType::Nak)
            | (DhcpState::Renewing, DhcpMessageType::Nak)
            | (DhcpState::Rebinding, DhcpMessageType::Nak) => {
                // Got a NAK — go back to INIT
                crate::serial_println!("  DHCP: NAK received — restarting");
                self.lease = None;
                self.state = DhcpState::Init;
                self.reset_retransmit();
                Some(DhcpState::Init)
            }

            _ => None,
        }
    }

    /// Configure the network interface from the DHCP response.
    fn configure_interface(&self, parsed: &ParsedDhcpResponse) {
        let ip = Ipv4Addr(parsed.your_ip);
        let mask = Ipv4Addr(parsed.subnet_mask);
        let gw = Ipv4Addr(parsed.gateway);

        // Configure the interface via the net module
        // Drop any existing config for this interface first
        let mac = crate::net::MacAddr(self.mac_addr);
        crate::net::configure_interface("eth0", mac, ip, mask, gw);

        // Add connected route
        crate::net::routing::add_connected(ip, mask, "eth0");

        // Set default gateway
        if gw != Ipv4Addr::ANY {
            crate::net::routing::set_default_gateway(gw, "eth0");
        }

        crate::serial_println!(
            "  DHCP: interface configured ip={} mask={} gw={}",
            ip,
            mask,
            gw
        );
    }

    // ----- Timer / state machine -----

    /// Periodic timer check. Should be called every ~1 second.
    /// Returns an action the caller should take.
    pub fn timer_tick(&mut self) -> DhcpAction {
        let now_ms = crate::time::clock::uptime_ms();
        let now_secs = now_ms / 1000;

        match self.state {
            DhcpState::Init => {
                // Nothing to do until start() is called
                DhcpAction::None
            }

            DhcpState::Selecting => {
                // Retransmit DISCOVER if timeout elapsed
                let elapsed = now_ms.saturating_sub(self.last_send_tick);
                if elapsed >= self.retransmit_interval {
                    if self.retransmit_count >= MAX_RETRANSMITS {
                        crate::serial_println!("  DHCP: DISCOVER max retries — giving up");
                        self.state = DhcpState::Init;
                        return DhcpAction::None;
                    }
                    self.retransmit_count = self.retransmit_count.saturating_add(1);
                    // Exponential backoff (cap at MAX_RETRANSMIT_MS)
                    self.retransmit_interval =
                        (self.retransmit_interval * 2).min(MAX_RETRANSMIT_MS);
                    self.last_send_tick = now_ms;
                    return DhcpAction::SendDiscover(self.build_discover());
                }
                DhcpAction::None
            }

            DhcpState::Requesting => {
                // Retransmit REQUEST if timeout elapsed
                let elapsed = now_ms.saturating_sub(self.last_send_tick);
                if elapsed >= self.retransmit_interval {
                    if self.retransmit_count >= MAX_RETRANSMITS {
                        crate::serial_println!("  DHCP: REQUEST max retries — restarting");
                        self.state = DhcpState::Init;
                        return DhcpAction::None;
                    }
                    self.retransmit_count = self.retransmit_count.saturating_add(1);
                    self.retransmit_interval =
                        (self.retransmit_interval * 2).min(MAX_RETRANSMIT_MS);
                    self.last_send_tick = now_ms;
                    let pkt = self.build_request(self.offered_server_ip, self.offered_ip);
                    return DhcpAction::SendRequest(pkt);
                }
                DhcpAction::None
            }

            DhcpState::Bound => {
                // Extract lease status before mutable borrows
                let lease_status = self.lease.as_ref().map(|lease| {
                    (
                        lease.is_expired(now_secs),
                        lease.needs_rebinding(now_secs),
                        lease.needs_renewal(now_secs),
                        lease.server_ip,
                    )
                });
                if let Some((expired, needs_rebind, needs_renew, server_ip)) = lease_status {
                    if expired {
                        // Lease expired — go back to INIT
                        crate::serial_println!("  DHCP: lease EXPIRED");
                        self.state = DhcpState::Init;
                        self.lease = None;
                        return DhcpAction::None;
                    }
                    if needs_rebind {
                        // T2 expired — enter REBINDING (broadcast)
                        crate::serial_println!("  DHCP: T2 expired — REBINDING");
                        self.state = DhcpState::Rebinding;
                        self.reset_retransmit();
                        if let Some(pkt) = self.build_renewal_request() {
                            self.last_send_tick = now_ms;
                            return DhcpAction::SendBroadcastRequest(pkt);
                        }
                    } else if needs_renew {
                        // T1 expired — enter RENEWING (unicast)
                        crate::serial_println!("  DHCP: T1 expired — RENEWING");
                        self.state = DhcpState::Renewing;
                        self.reset_retransmit();
                        if let Some(pkt) = self.build_renewal_request() {
                            self.last_send_tick = now_ms;
                            return DhcpAction::SendUnicastRequest(pkt, server_ip);
                        }
                    }
                }
                DhcpAction::None
            }

            DhcpState::Renewing => {
                let elapsed = now_ms.saturating_sub(self.last_send_tick);
                if elapsed >= self.retransmit_interval {
                    if let Some(ref lease) = self.lease {
                        if lease.needs_rebinding(now_secs) {
                            // Escalate to REBINDING
                            crate::serial_println!("  DHCP: escalating to REBINDING");
                            self.state = DhcpState::Rebinding;
                            self.reset_retransmit();
                        } else if lease.is_expired(now_secs) {
                            self.state = DhcpState::Init;
                            self.lease = None;
                            return DhcpAction::None;
                        }
                    }
                    self.retransmit_count = self.retransmit_count.saturating_add(1);
                    self.retransmit_interval =
                        (self.retransmit_interval * 2).min(MAX_RETRANSMIT_MS);
                    self.last_send_tick = now_ms;
                    if let Some(pkt) = self.build_renewal_request() {
                        if let Some(ref lease) = self.lease {
                            return DhcpAction::SendUnicastRequest(pkt, lease.server_ip);
                        }
                    }
                }
                DhcpAction::None
            }

            DhcpState::Rebinding => {
                let elapsed = now_ms.saturating_sub(self.last_send_tick);
                if elapsed >= self.retransmit_interval {
                    if let Some(ref lease) = self.lease {
                        if lease.is_expired(now_secs) {
                            crate::serial_println!("  DHCP: lease expired during rebind");
                            self.state = DhcpState::Init;
                            self.lease = None;
                            return DhcpAction::None;
                        }
                    }
                    self.retransmit_count = self.retransmit_count.saturating_add(1);
                    self.retransmit_interval =
                        (self.retransmit_interval * 2).min(MAX_RETRANSMIT_MS);
                    self.last_send_tick = now_ms;
                    if let Some(pkt) = self.build_renewal_request() {
                        return DhcpAction::SendBroadcastRequest(pkt);
                    }
                }
                DhcpAction::None
            }

            DhcpState::Released => DhcpAction::None,
        }
    }

    // ----- Public API -----

    /// Start the DHCP process: send a DISCOVER.
    pub fn start(&mut self, mac: [u8; 6], interface: &str) -> Vec<u8> {
        self.mac_addr = mac;
        self.interface = String::from(interface);
        self.new_xid();
        self.state = DhcpState::Selecting;
        self.reset_retransmit();
        self.last_send_tick = crate::time::clock::uptime_ms();

        crate::serial_println!("  DHCP: DISCOVER sent (xid={:#010x})", self.transaction_id);
        self.build_discover()
    }

    /// Release the current lease.
    pub fn release(&mut self) -> Option<Vec<u8>> {
        if self.state != DhcpState::Bound
            && self.state != DhcpState::Renewing
            && self.state != DhcpState::Rebinding
        {
            return None;
        }
        let pkt = self.build_release();
        self.state = DhcpState::Released;
        crate::serial_println!("  DHCP: RELEASE sent");
        pkt
    }

    /// Decline an offered IP (e.g., ARP detected conflict).
    pub fn decline(&mut self) -> Option<Vec<u8>> {
        if self.offered_ip == [0, 0, 0, 0] {
            return None;
        }
        let pkt = self.build_decline(self.offered_ip, self.offered_server_ip);
        self.state = DhcpState::Init;
        crate::serial_println!("  DHCP: DECLINE sent (IP conflict)");
        Some(pkt)
    }

    /// Get current IP address.
    pub fn ip(&self) -> Option<[u8; 4]> {
        self.lease.as_ref().map(|l| l.ip)
    }

    /// Format lease info.
    pub fn lease_info(&self) -> String {
        if let Some(ref l) = self.lease {
            let now_secs = crate::time::clock::uptime_ms() / 1000;
            let remaining = l.remaining_secs(now_secs);
            format!(
                "IP: {}.{}.{}.{}\nMask: {}.{}.{}.{}\nGW: {}.{}.{}.{}\nDNS: {}\nDomain: {}\nLease: {}s ({}s remaining)\nState: {:?}",
                l.ip[0], l.ip[1], l.ip[2], l.ip[3],
                l.subnet_mask[0], l.subnet_mask[1], l.subnet_mask[2], l.subnet_mask[3],
                l.gateway[0], l.gateway[1], l.gateway[2], l.gateway[3],
                format_dns_servers(&l.dns_servers),
                l.domain,
                l.lease_time, remaining,
                self.state,
            )
        } else {
            format!("No lease (state: {:?})", self.state)
        }
    }
}

/// Action to take after a DHCP timer tick
pub enum DhcpAction {
    /// No action needed
    None,
    /// Send a DISCOVER (broadcast)
    SendDiscover(Vec<u8>),
    /// Send a REQUEST (broadcast, during SELECTING/REQUESTING)
    SendRequest(Vec<u8>),
    /// Send a REQUEST (unicast to server, during RENEWING)
    SendUnicastRequest(Vec<u8>, [u8; 4]),
    /// Send a REQUEST (broadcast, during REBINDING)
    SendBroadcastRequest(Vec<u8>),
}

/// Format DNS server list as a string
fn format_dns_servers(servers: &[[u8; 4]]) -> String {
    let mut s = String::new();
    for (i, srv) in servers.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{}.{}.{}.{}", srv[0], srv[1], srv[2], srv[3]));
    }
    if s.is_empty() {
        s.push_str("none");
    }
    s
}

// ---------------------------------------------------------------------------
// Global DHCP client
// ---------------------------------------------------------------------------

static DHCP: Mutex<DhcpClient> = Mutex::new(DhcpClient::new());

/// Initialize DHCP subsystem.
pub fn init() {
    crate::serial_println!("  [dhcp] DHCP client initialized");
}

/// Start DHCP discovery on the given interface.
/// Returns the DISCOVER packet to send via UDP broadcast.
pub fn start(mac: [u8; 6], interface: &str) -> Vec<u8> {
    DHCP.lock().start(mac, interface)
}

/// Get a DISCOVER packet (legacy API for backward compatibility).
pub fn discover() -> Vec<u8> {
    let dhcp = DHCP.lock();
    dhcp.build_discover()
}

/// Process a DHCP response received on port 68.
pub fn process_response(data: &[u8]) -> Option<DhcpState> {
    DHCP.lock().process_response(data)
}

/// Periodic timer tick. Returns an action the caller should take.
pub fn timer_tick() -> DhcpAction {
    DHCP.lock().timer_tick()
}

/// Release the current lease.
pub fn release() -> Option<Vec<u8>> {
    DHCP.lock().release()
}

/// Decline the offered IP (conflict detected).
pub fn decline() -> Option<Vec<u8>> {
    DHCP.lock().decline()
}

/// Get the current DHCP state.
pub fn state() -> DhcpState {
    DHCP.lock().state
}

/// Get the current IP from the lease.
pub fn current_ip() -> Option<[u8; 4]> {
    DHCP.lock().ip()
}

/// Get lease info as a formatted string.
pub fn lease_info() -> String {
    DHCP.lock().lease_info()
}

/// Get the current lease (cloned).
pub fn current_lease() -> Option<DhcpLease> {
    DHCP.lock().lease.clone()
}

/// Set the MAC address for the DHCP client.
pub fn set_mac(mac: [u8; 6]) {
    DHCP.lock().mac_addr = mac;
}
