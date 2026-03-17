use super::ipv4;
use super::{Ipv4Addr, NetworkDriver};
use crate::sync::Mutex;
/// ICMP (Internet Control Message Protocol) for Genesis
///
/// Handles ping (echo request/reply), error messages, and diagnostics.
///
/// Features:
///   - ICMP echo request/reply (ping) with sequence tracking
///   - ICMP destination unreachable (all codes)
///   - ICMP time exceeded (for traceroute)
///   - ICMP redirect handling
///   - Ping statistics: sent, received, min/max/avg RTT (integer microseconds)
///   - Rate limiting on ICMP error messages
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// ICMP types
// ---------------------------------------------------------------------------

pub const ECHO_REPLY: u8 = 0;
pub const DEST_UNREACHABLE: u8 = 3;
pub const SOURCE_QUENCH: u8 = 4;
pub const REDIRECT: u8 = 5;
pub const ECHO_REQUEST: u8 = 8;
pub const TIME_EXCEEDED: u8 = 11;
pub const PARAMETER_PROBLEM: u8 = 12;
pub const TIMESTAMP_REQUEST: u8 = 13;
pub const TIMESTAMP_REPLY: u8 = 14;

// ---------------------------------------------------------------------------
// Destination Unreachable codes
// ---------------------------------------------------------------------------

pub const DEST_NET_UNREACHABLE: u8 = 0;
pub const DEST_HOST_UNREACHABLE: u8 = 1;
pub const DEST_PROTO_UNREACHABLE: u8 = 2;
pub const DEST_PORT_UNREACHABLE: u8 = 3;
pub const DEST_FRAG_NEEDED: u8 = 4;
pub const DEST_SOURCE_ROUTE_FAILED: u8 = 5;
pub const DEST_NET_UNKNOWN: u8 = 6;
pub const DEST_HOST_UNKNOWN: u8 = 7;
pub const DEST_HOST_ISOLATED: u8 = 8;
pub const DEST_NET_PROHIBITED: u8 = 9;
pub const DEST_HOST_PROHIBITED: u8 = 10;
pub const DEST_NET_TOS_UNREACHABLE: u8 = 11;
pub const DEST_HOST_TOS_UNREACHABLE: u8 = 12;
pub const DEST_ADMIN_PROHIBITED: u8 = 13;

// ---------------------------------------------------------------------------
// Time Exceeded codes
// ---------------------------------------------------------------------------

pub const TTL_EXCEEDED: u8 = 0;
pub const FRAG_REASSEMBLY_EXCEEDED: u8 = 1;

// ---------------------------------------------------------------------------
// Redirect codes
// ---------------------------------------------------------------------------

pub const REDIRECT_NET: u8 = 0;
pub const REDIRECT_HOST: u8 = 1;
pub const REDIRECT_TOS_NET: u8 = 2;
pub const REDIRECT_TOS_HOST: u8 = 3;

// ---------------------------------------------------------------------------
// ICMP header
// ---------------------------------------------------------------------------

/// ICMP header (8 bytes minimum)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct IcmpHeader {
    pub icmp_type: u8,
    pub code: u8,
    pub checksum: [u8; 2],
    pub rest: [u8; 4], // depends on type (e.g., ID + sequence for echo)
}

impl IcmpHeader {
    /// Parse an ICMP packet
    pub fn parse(data: &[u8]) -> Option<(&IcmpHeader, &[u8])> {
        if data.len() < 8 {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const IcmpHeader) };
        let payload = &data[8..];
        Some((header, payload))
    }

    /// Get the echo identifier (for echo request/reply)
    pub fn echo_id(&self) -> u16 {
        u16::from_be_bytes([self.rest[0], self.rest[1]])
    }

    /// Get the echo sequence number
    pub fn echo_seq(&self) -> u16 {
        u16::from_be_bytes([self.rest[2], self.rest[3]])
    }

    /// Get the gateway IP (for redirect messages)
    pub fn gateway_ip(&self) -> Ipv4Addr {
        Ipv4Addr(self.rest)
    }

    /// Get the next-hop MTU (for Frag Needed, code 4)
    pub fn next_hop_mtu(&self) -> u16 {
        u16::from_be_bytes([self.rest[2], self.rest[3]])
    }
}

/// Verify the ICMP checksum of a packet.
pub fn verify_checksum(icmp_packet: &[u8]) -> bool {
    ipv4::internet_checksum(icmp_packet) == 0
}

// ---------------------------------------------------------------------------
// Echo request / reply (Ping)
// ---------------------------------------------------------------------------

/// Build an ICMP echo reply in response to an echo request
pub fn build_echo_reply(request: &[u8]) -> Option<Vec<u8>> {
    let (header, _payload) = IcmpHeader::parse(request)?;

    if header.icmp_type != ECHO_REQUEST {
        return None;
    }

    let mut reply = alloc::vec![0u8; request.len()];

    // Copy the request, change type to reply
    reply.copy_from_slice(request);
    reply[0] = ECHO_REPLY; // type = 0

    // Recompute checksum
    reply[2] = 0;
    reply[3] = 0;
    let cksum = ipv4::internet_checksum(&reply);
    reply[2] = (cksum >> 8) as u8;
    reply[3] = cksum as u8;

    Some(reply)
}

/// Build an ICMP echo request (ping).
/// `id` is the identifier, `seq` is the sequence number.
/// `payload` is optional additional data to include.
pub fn build_echo_request(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let total = 8 + payload.len();
    let mut pkt = alloc::vec![0u8; total];

    pkt[0] = ECHO_REQUEST;
    pkt[1] = 0; // code
                // checksum placeholder [2..4]
    pkt[4] = (id >> 8) as u8;
    pkt[5] = id as u8;
    pkt[6] = (seq >> 8) as u8;
    pkt[7] = seq as u8;

    if !payload.is_empty() {
        pkt[8..8 + payload.len()].copy_from_slice(payload);
    }

    // Compute checksum
    let cksum = ipv4::internet_checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    // Record statistics
    {
        let mut s = PING_STATS.lock();
        s.sent = s.sent.saturating_add(1);
    }

    pkt
}

// ---------------------------------------------------------------------------
// Error message building
// ---------------------------------------------------------------------------

/// Build an ICMP Destination Unreachable message.
/// `code` is one of the DEST_* constants.
/// `original_packet` should be the first 8 bytes of the original IP datagram
/// (IP header + first 8 bytes of payload).
pub fn build_dest_unreachable(code: u8, original_packet: &[u8]) -> Vec<u8> {
    if !rate_limit_check() {
        return Vec::new(); // Rate limited
    }

    // Include IP header + 8 bytes of original datagram
    let include_len = original_packet.len().min(28); // 20-byte IP header + 8 bytes
    let total = 8 + include_len;
    let mut pkt = alloc::vec![0u8; total];

    pkt[0] = DEST_UNREACHABLE;
    pkt[1] = code;
    // checksum [2..4] = 0
    // rest [4..8] = 0 (unused, except code 4 which has next-hop MTU in bytes 6-7)

    pkt[8..8 + include_len].copy_from_slice(&original_packet[..include_len]);

    // Compute checksum
    let cksum = ipv4::internet_checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    pkt
}

/// Build a Destination Unreachable with Fragmentation Needed (code 4).
/// `mtu` is the next-hop MTU to report to the sender.
pub fn build_frag_needed(mtu: u16, original_packet: &[u8]) -> Vec<u8> {
    if !rate_limit_check() {
        return Vec::new();
    }

    let include_len = original_packet.len().min(28);
    let total = 8 + include_len;
    let mut pkt = alloc::vec![0u8; total];

    pkt[0] = DEST_UNREACHABLE;
    pkt[1] = DEST_FRAG_NEEDED;
    // rest[0..2] = 0 (unused)
    // rest[2..4] = next-hop MTU
    pkt[6] = (mtu >> 8) as u8;
    pkt[7] = mtu as u8;

    pkt[8..8 + include_len].copy_from_slice(&original_packet[..include_len]);

    let cksum = ipv4::internet_checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    pkt
}

/// Build an ICMP Time Exceeded message.
/// `code`: 0 = TTL exceeded in transit, 1 = fragment reassembly time exceeded.
pub fn build_time_exceeded(code: u8, original_packet: &[u8]) -> Vec<u8> {
    if !rate_limit_check() {
        return Vec::new();
    }

    let include_len = original_packet.len().min(28);
    let total = 8 + include_len;
    let mut pkt = alloc::vec![0u8; total];

    pkt[0] = TIME_EXCEEDED;
    pkt[1] = code;
    // rest [4..8] = 0 (unused)

    pkt[8..8 + include_len].copy_from_slice(&original_packet[..include_len]);

    let cksum = ipv4::internet_checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    pkt
}

/// Build an ICMP Redirect message.
/// `code`: one of the REDIRECT_* constants.
/// `gateway`: the IP of the better gateway.
pub fn build_redirect(code: u8, gateway: Ipv4Addr, original_packet: &[u8]) -> Vec<u8> {
    if !rate_limit_check() {
        return Vec::new();
    }

    let include_len = original_packet.len().min(28);
    let total = 8 + include_len;
    let mut pkt = alloc::vec![0u8; total];

    pkt[0] = REDIRECT;
    pkt[1] = code;
    // Gateway IP in rest field
    pkt[4..8].copy_from_slice(&gateway.0);

    pkt[8..8 + include_len].copy_from_slice(&original_packet[..include_len]);

    let cksum = ipv4::internet_checksum(&pkt);
    pkt[2] = (cksum >> 8) as u8;
    pkt[3] = cksum as u8;

    pkt
}

// ---------------------------------------------------------------------------
// ICMP message processing
// ---------------------------------------------------------------------------

/// Process a received ICMP message. Returns an action to take.
pub fn process_icmp(data: &[u8]) -> IcmpAction {
    let (header, _payload) = match IcmpHeader::parse(data) {
        Some(h) => h,
        None => return IcmpAction::Drop,
    };

    if !verify_checksum(data) {
        return IcmpAction::Drop;
    }

    match header.icmp_type {
        ECHO_REQUEST => {
            if let Some(reply) = build_echo_reply(data) {
                IcmpAction::SendReply(reply)
            } else {
                IcmpAction::Drop
            }
        }
        ECHO_REPLY => {
            let id = header.echo_id();
            let seq = header.echo_seq();
            record_echo_reply(id, seq);
            IcmpAction::EchoReply(id, seq)
        }
        DEST_UNREACHABLE => IcmpAction::DestUnreachable(header.code),
        TIME_EXCEEDED => IcmpAction::TimeExceeded(header.code),
        REDIRECT => {
            let gateway = header.gateway_ip();
            IcmpAction::Redirect(header.code, gateway)
        }
        _ => IcmpAction::Drop,
    }
}

/// Action to take after processing an ICMP message
pub enum IcmpAction {
    /// Send a reply packet
    SendReply(Vec<u8>),
    /// Received an echo reply (id, seq)
    EchoReply(u16, u16),
    /// Destination unreachable (code)
    DestUnreachable(u8),
    /// Time exceeded (code)
    TimeExceeded(u8),
    /// Redirect to a better gateway (code, gateway IP)
    Redirect(u8, Ipv4Addr),
    /// Drop the packet (invalid or unhandled)
    Drop,
}

// ---------------------------------------------------------------------------
// Ping statistics
// ---------------------------------------------------------------------------

/// Ping statistics tracker
struct PingStatsInner {
    /// Echo requests sent
    sent: u64,
    /// Echo replies received
    received: u64,
    /// Minimum RTT in microseconds (u64::MAX if no samples)
    min_rtt_us: u64,
    /// Maximum RTT in microseconds
    max_rtt_us: u64,
    /// Sum of all RTTs in microseconds (for computing average)
    sum_rtt_us: u64,
    /// Active pings: maps (id, seq) key -> sent_tick_ms
    /// Key is (id << 16) | seq as a u32
    active_pings: [u64; 64], // Ring buffer of sent timestamps
    active_ids: [u32; 64], // Corresponding (id << 16 | seq) keys
    active_head: usize,
    active_count: usize,
}

static PING_STATS: Mutex<PingStatsInner> = Mutex::new(PingStatsInner {
    sent: 0,
    received: 0,
    min_rtt_us: u64::MAX,
    max_rtt_us: 0,
    sum_rtt_us: 0,
    active_pings: [0; 64],
    active_ids: [0; 64],
    active_head: 0,
    active_count: 0,
});

/// Record that we sent an echo request (for RTT tracking).
pub fn record_echo_sent(id: u16, seq: u16) {
    let now = crate::time::clock::uptime_ms();
    let key = ((id as u32) << 16) | (seq as u32);
    let mut stats = PING_STATS.lock();

    // Store in ring buffer
    let idx = (stats.active_head + stats.active_count) % 64;
    if stats.active_count < 64 {
        stats.active_count = stats.active_count.saturating_add(1);
    } else {
        stats.active_head = (stats.active_head + 1) % 64;
    }
    stats.active_pings[idx] = now;
    stats.active_ids[idx] = key;
}

/// Record that we received an echo reply.
fn record_echo_reply(id: u16, seq: u16) {
    let now = crate::time::clock::uptime_ms();
    let key = ((id as u32) << 16) | (seq as u32);
    let mut stats = PING_STATS.lock();

    stats.received = stats.received.saturating_add(1);

    // Find the matching sent timestamp
    for i in 0..stats.active_count {
        let idx = (stats.active_head + i) % 64;
        if stats.active_ids[idx] == key {
            let sent_tick = stats.active_pings[idx];
            let rtt_ms = now.saturating_sub(sent_tick);
            // Convert to microseconds (our ticks are in ms)
            let rtt_us = rtt_ms * 1000;

            if rtt_us < stats.min_rtt_us {
                stats.min_rtt_us = rtt_us;
            }
            if rtt_us > stats.max_rtt_us {
                stats.max_rtt_us = rtt_us;
            }
            stats.sum_rtt_us += rtt_us;

            // Remove this entry by marking it as 0
            stats.active_ids[idx] = 0;
            break;
        }
    }
}

/// Ping statistics snapshot
pub struct PingStats {
    pub sent: u64,
    pub received: u64,
    pub lost: u64,
    pub min_rtt_us: u64,
    pub max_rtt_us: u64,
    pub avg_rtt_us: u64,
    /// Loss percentage (0-100)
    pub loss_pct: u32,
}

/// Get current ping statistics.
pub fn get_ping_stats() -> PingStats {
    let stats = PING_STATS.lock();
    let lost = stats.sent.saturating_sub(stats.received);
    let avg = if stats.received > 0 {
        stats.sum_rtt_us / stats.received
    } else {
        0
    };
    let loss_pct = if stats.sent > 0 {
        ((lost * 100) / stats.sent) as u32
    } else {
        0
    };
    PingStats {
        sent: stats.sent,
        received: stats.received,
        lost,
        min_rtt_us: if stats.min_rtt_us == u64::MAX {
            0
        } else {
            stats.min_rtt_us
        },
        max_rtt_us: stats.max_rtt_us,
        avg_rtt_us: avg,
        loss_pct,
    }
}

/// Reset ping statistics.
pub fn reset_ping_stats() {
    let mut stats = PING_STATS.lock();
    stats.sent = 0;
    stats.received = 0;
    stats.min_rtt_us = u64::MAX;
    stats.max_rtt_us = 0;
    stats.sum_rtt_us = 0;
    stats.active_head = 0;
    stats.active_count = 0;
}

// ---------------------------------------------------------------------------
// Rate limiting
// ---------------------------------------------------------------------------

/// Rate limiter state for ICMP error messages.
/// Limits to at most `RATE_LIMIT_PER_SEC` error messages per second.
const RATE_LIMIT_PER_SEC: u64 = 10;

struct RateLimiter {
    /// Tokens available (allows burst)
    tokens: u64,
    /// Last tick when tokens were replenished
    last_tick: u64,
    /// Maximum tokens (bucket size)
    max_tokens: u64,
}

static RATE_LIMITER: Mutex<RateLimiter> = Mutex::new(RateLimiter {
    tokens: 10,
    last_tick: 0,
    max_tokens: 10,
});

/// Check if we can send an ICMP error message (token bucket rate limiter).
/// Returns true if allowed, false if rate-limited.
fn rate_limit_check() -> bool {
    let now = crate::time::clock::uptime_ms();
    let mut rl = RATE_LIMITER.lock();

    // Replenish tokens based on elapsed time
    let elapsed_ms = now.saturating_sub(rl.last_tick);
    if elapsed_ms > 0 {
        // Add tokens: RATE_LIMIT_PER_SEC tokens per 1000ms
        let new_tokens = (elapsed_ms * RATE_LIMIT_PER_SEC) / 1000;
        rl.tokens = (rl.tokens + new_tokens).min(rl.max_tokens);
        rl.last_tick = now;
    }

    if rl.tokens > 0 {
        rl.tokens = rl.tokens.saturating_sub(1);
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Traceroute helper
// ---------------------------------------------------------------------------

/// Build an ICMP echo request with a specific TTL for traceroute.
/// The caller must set the IP TTL when sending.
/// Returns (icmp_packet, ttl).
pub fn build_traceroute_probe(id: u16, seq: u16, ttl: u8) -> (Vec<u8>, u8) {
    // Use the sequence number to encode the TTL for identification
    let pkt = build_echo_request(id, seq, &[]);
    record_echo_sent(id, seq);
    (pkt, ttl)
}

// ---------------------------------------------------------------------------
// handle_icmp — full inbound dispatch with IPv4 reply path
// ---------------------------------------------------------------------------

/// Handle a complete incoming ICMP packet (from an IPv4 frame).
///
/// `icmp_data`  — the raw ICMP bytes (type + code + checksum + rest + payload).
/// `src_ip`     — IPv4 source address of the enclosing IP datagram (big-endian octets).
///
/// For Echo Request (type 8) this function builds and returns an Echo Reply
/// wrapped in a complete IPv4 packet ready to hand to the NIC driver.
/// For other types it updates internal state and returns `None`.
pub fn handle_icmp(icmp_data: &[u8], src_ip: [u8; 4]) -> Option<Vec<u8>> {
    if !verify_checksum(icmp_data) {
        return None;
    }
    let (header, _payload) = IcmpHeader::parse(icmp_data)?;

    match header.icmp_type {
        ECHO_REQUEST => {
            // Build the ICMP echo reply
            let icmp_reply = build_echo_reply(icmp_data)?;

            // Determine our source IP from the first configured interface.
            // We build the IPv4 header here so the caller only needs to send
            // the returned bytes directly.
            let our_ip = get_our_ipv4();

            // IPv4 header: 20 bytes
            let total_len = (20u16).saturating_add(icmp_reply.len() as u16);
            let mut ip_hdr = [0u8; 20];
            ip_hdr[0] = 0x45; // version=4, IHL=5
            ip_hdr[1] = 0x00; // DSCP/ECN
            let tl = total_len.to_be_bytes();
            ip_hdr[2] = tl[0];
            ip_hdr[3] = tl[1];
            // ID, flags, fragment offset = 0
            ip_hdr[8] = 64; // TTL
            ip_hdr[9] = super::ipv4::PROTO_ICMP;
            // Source = our IP, Dst = original sender
            ip_hdr[12..16].copy_from_slice(&our_ip);
            ip_hdr[16..20].copy_from_slice(&src_ip);
            // Compute IPv4 header checksum
            let cksum = super::ipv4::internet_checksum(&ip_hdr);
            ip_hdr[10] = (cksum >> 8) as u8;
            ip_hdr[11] = cksum as u8;

            let mut packet = Vec::with_capacity(20 + icmp_reply.len());
            packet.extend_from_slice(&ip_hdr);
            packet.extend_from_slice(&icmp_reply);
            Some(packet)
        }
        ECHO_REPLY => {
            let id = header.echo_id();
            let seq = header.echo_seq();
            record_echo_reply(id, seq);
            None
        }
        TIME_EXCEEDED => {
            // Record for traceroute — the ICMP data includes the original
            // IP header + 8 bytes, so we can extract the original dest.
            None
        }
        _ => None,
    }
}

/// Return our IPv4 source address (first configured interface, or 0.0.0.0).
fn get_our_ipv4() -> [u8; 4] {
    let ifaces = crate::net::INTERFACES.lock();
    ifaces
        .first()
        .and_then(|i| i.ipv4)
        .map(|ip| ip.0)
        .unwrap_or([0, 0, 0, 0])
}

// ---------------------------------------------------------------------------
// ping — send ICMP echo request and wait for reply
// ---------------------------------------------------------------------------

/// Send an ICMP Echo Request to `dst_ip` and spin-wait for the Echo Reply.
///
/// Returns `true` if a reply was received within ~100 000 spin iterations,
/// `false` on timeout or send failure.
///
/// This is a bare-metal busy-wait ping suitable for kernel diagnostics.
/// It uses a hardcoded identifier of `0xAB01` and sequence number `1`.
pub fn ping(dst_ip: [u8; 4]) -> bool {
    const PING_ID: u16 = 0xAB01;
    const PING_SEQ: u16 = 1;

    let icmp_pkt = build_echo_request(PING_ID, PING_SEQ, b"genesis");
    record_echo_sent(PING_ID, PING_SEQ);

    // Wrap in IPv4 and send via the stack
    let our_ip = get_our_ipv4();
    let total_len = (20u16).saturating_add(icmp_pkt.len() as u16);
    let mut ip_hdr = [0u8; 20];
    ip_hdr[0] = 0x45;
    ip_hdr[2] = (total_len >> 8) as u8;
    ip_hdr[3] = total_len as u8;
    ip_hdr[8] = 64; // TTL
    ip_hdr[9] = super::ipv4::PROTO_ICMP;
    ip_hdr[12..16].copy_from_slice(&our_ip);
    ip_hdr[16..20].copy_from_slice(&dst_ip);
    let cksum = super::ipv4::internet_checksum(&ip_hdr);
    ip_hdr[10] = (cksum >> 8) as u8;
    ip_hdr[11] = cksum as u8;

    let mut packet = Vec::with_capacity(20 + icmp_pkt.len());
    packet.extend_from_slice(&ip_hdr);
    packet.extend_from_slice(&icmp_pkt);

    // Send via the NIC driver (mirrors crate::net::send_ip_frame logic)
    {
        let our_mac = {
            let ifaces = crate::net::INTERFACES.lock();
            ifaces
                .first()
                .map(|i| i.mac)
                .unwrap_or(crate::net::MacAddr::ZERO)
        };
        let dst_mac_raw = super::arp::lookup(crate::net::Ipv4Addr(dst_ip))
            .unwrap_or(crate::net::MacAddr::BROADCAST);
        crate::net::send_ip_frame_pub(our_mac, dst_mac_raw, &packet);
    }

    // Spin-wait for the echo reply to be processed.
    // The NIC poll path calls handle_icmp / record_echo_reply which updates
    // PING_STATS.received.
    let sent_before = {
        let s = PING_STATS.lock();
        s.received
    };

    for _ in 0u32..100_000 {
        crate::net::poll();
        let received_now = {
            let s = PING_STATS.lock();
            s.received
        };
        if received_now > sent_before {
            return true;
        }
        core::hint::spin_loop();
    }

    false
}

// ---------------------------------------------------------------------------
// traceroute_hop — probe a single TTL hop
// ---------------------------------------------------------------------------

/// Send a UDP probe with a given TTL and wait for an ICMP Time Exceeded reply.
///
/// Returns the IP address of the router that generated the TTL-exceeded
/// message, or `None` on timeout.
///
/// Implementation notes (bare-metal):
/// - Uses UDP port 33434 (traditional traceroute destination port).
/// - Spins up to 200 000 iterations waiting for the ICMP reply.
/// - The ICMP Time Exceeded message contains the original IP header and first
///   8 bytes of the UDP datagram; we extract the gateway address from the
///   outer IP source.
pub fn traceroute_hop(dst: [u8; 4], ttl: u8) -> Option<[u8; 4]> {
    const TRACEROUTE_PORT: u16 = 33434;
    let our_ip = get_our_ipv4();
    let our_mac = {
        let ifaces = crate::net::INTERFACES.lock();
        ifaces
            .first()
            .map(|i| i.mac)
            .unwrap_or(crate::net::MacAddr::ZERO)
    };

    // Build a tiny UDP datagram (8-byte header, no payload)
    let udp_src_port: u16 = 54321;
    let udp_len: u16 = 8;
    let mut udp_hdr = [0u8; 8];
    udp_hdr[0..2].copy_from_slice(&udp_src_port.to_be_bytes());
    udp_hdr[2..4].copy_from_slice(&TRACEROUTE_PORT.to_be_bytes());
    udp_hdr[4..6].copy_from_slice(&udp_len.to_be_bytes());
    // UDP checksum = 0 (optional for IPv4)

    // Build IPv4 header with the requested TTL
    let total_len = (20u16).saturating_add(8);
    let mut ip_hdr = [0u8; 20];
    ip_hdr[0] = 0x45;
    ip_hdr[2] = (total_len >> 8) as u8;
    ip_hdr[3] = total_len as u8;
    ip_hdr[8] = ttl;
    ip_hdr[9] = 17; // UDP
    ip_hdr[12..16].copy_from_slice(&our_ip);
    ip_hdr[16..20].copy_from_slice(&dst);
    let cksum = super::ipv4::internet_checksum(&ip_hdr);
    ip_hdr[10] = (cksum >> 8) as u8;
    ip_hdr[11] = cksum as u8;

    let mut packet = Vec::with_capacity(28);
    packet.extend_from_slice(&ip_hdr);
    packet.extend_from_slice(&udp_hdr);

    let dst_mac =
        super::arp::lookup(crate::net::Ipv4Addr(dst)).unwrap_or(crate::net::MacAddr::BROADCAST);
    crate::net::send_ip_frame_pub(our_mac, dst_mac, &packet);

    // Spin-wait for an ICMP Time Exceeded that references our probe.
    // We watch the raw NIC receive path by polling and scanning for ICMP
    // type=11 packets in a small scratch buffer.
    let mut rx_buf = [0u8; 2048];
    for _ in 0u32..200_000 {
        let len = {
            let driver = crate::drivers::e1000::driver().lock();
            match driver.as_ref() {
                Some(nic) => NetworkDriver::recv(nic, &mut rx_buf).unwrap_or(0),
                None => 0,
            }
        };
        if len >= 14 + 20 + 8 {
            // Skip Ethernet header (14) and outer IPv4 header (20)
            let eth_payload = &rx_buf[14..len];
            let ihl = ((eth_payload[0] & 0x0F) as usize) * 4;
            if ihl < 20 || eth_payload.len() < ihl + 8 {
                core::hint::spin_loop();
                continue;
            }
            let proto = eth_payload[9];
            let gateway_ip = [
                eth_payload[12],
                eth_payload[13],
                eth_payload[14],
                eth_payload[15],
            ];
            if proto == super::ipv4::PROTO_ICMP {
                let icmp = &eth_payload[ihl..];
                if icmp.len() >= 8 && icmp[0] == TIME_EXCEEDED {
                    return Some(gateway_ip);
                }
            }
            // Also feed the frame to the rest of the stack
            crate::net::process_frame(&rx_buf[..len]);
        }
        core::hint::spin_loop();
    }
    None
}

// ---------------------------------------------------------------------------
// ICMPv6 — Neighbor Solicitation helper
// ---------------------------------------------------------------------------

/// Send an ICMPv6 Neighbor Solicitation for `target` using the IPv6/NDP
/// subsystem.
///
/// This is a convenience wrapper that delegates to
/// `crate::net::ipv6::ndp_neighbor_solicitation` for packet construction,
/// then transmits over the first NIC using the solicited-node multicast
/// destination MAC (33:33:ff:XX:XX:XX per RFC 2464).
pub fn icmpv6_neighbor_solicitation(target: [u8; 16]) {
    use crate::net::ipv6;

    // Get our link-local source address and MAC (ifindex 0 = primary interface)
    let our_ll = match ipv6::get_link_local(0) {
        Some(a) => a,
        None => return, // IPv6 not configured
    };
    let our_mac = {
        let ifaces = crate::net::INTERFACES.lock();
        match ifaces.first() {
            Some(i) => i.mac.0,
            None => return,
        }
    };

    // Build the ICMPv6 NS packet into a 32-byte stack buffer
    let mut icmpv6_buf = [0u8; 32];
    let ns_len = ipv6::ndp_neighbor_solicitation(&mut icmpv6_buf, our_ll, target, our_mac);

    // Solicited-node multicast destination
    let dst_v6 = ipv6::solicited_node_multicast(target);

    // Build IPv6 header (returns Ipv6Header struct, write to stack buffer)
    let ip6hdr_struct = ipv6::ipv6_build_header(
        ipv6::Ipv6Addr(our_ll),
        ipv6::Ipv6Addr(dst_v6),
        58, // Next Header = ICMPv6
        ns_len as u16,
    );
    let mut ip6_hdr = [0u8; 40];
    ip6hdr_struct.write_to(&mut ip6_hdr);

    // Ethernet destination for solicited-node multicast: 33:33:ff:XX:XX:XX
    let eth_dst = crate::net::MacAddr([0x33, 0x33, dst_v6[12], dst_v6[13], dst_v6[14], dst_v6[15]]);
    let our_eth_mac = crate::net::MacAddr(our_mac);

    // Assemble full frame on stack (14 eth + 40 ip6 + 32 icmpv6 = 86 bytes, pad to 60)
    let mut frame = [0u8; 86];
    frame[0..6].copy_from_slice(&eth_dst.0);
    frame[6..12].copy_from_slice(&our_eth_mac.0);
    frame[12..14].copy_from_slice(&(0x86DDu16).to_be_bytes());
    frame[14..54].copy_from_slice(&ip6_hdr);
    frame[54..54 + ns_len].copy_from_slice(&icmpv6_buf[..ns_len]);

    let driver = crate::drivers::e1000::driver().lock();
    if let Some(ref nic) = *driver {
        let _ = nic.send(&frame[..54 + ns_len]);
    }
    crate::serial_println!("  ICMPv6: NS sent for target {:02x?}", &target[13..]);
}
