use super::ipv4;
use super::{Ipv4Addr, NetError};
use crate::sync::Mutex;
/// UDP (User Datagram Protocol) for Genesis
///
/// Simple, connectionless datagram protocol.
/// Used for DNS, DHCP, game networking, etc.
///
/// Features:
///   - UDP header parse/build (8-byte header)
///   - UDP checksum with pseudo-header (RFC 768)
///   - Socket binding table
///   - Multicast group join/leave tracking
///   - Broadcast support
///   - Statistics counters
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum UDP payload size (65535 - 8-byte header - 20-byte IP header)
pub const MAX_UDP_PAYLOAD: usize = 65507;

/// UDP header size
pub const UDP_HEADER_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// UDP header
// ---------------------------------------------------------------------------

/// UDP header (8 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct UdpHeader {
    pub src_port: [u8; 2],
    pub dst_port: [u8; 2],
    pub length: [u8; 2],
    pub checksum: [u8; 2],
}

impl UdpHeader {
    pub fn parse(data: &[u8]) -> Option<(&UdpHeader, &[u8])> {
        if data.len() < UDP_HEADER_SIZE {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const UdpHeader) };
        let total_len = header.length_u16() as usize;
        if total_len < UDP_HEADER_SIZE || data.len() < total_len {
            return None;
        }
        let payload = &data[UDP_HEADER_SIZE..total_len];
        Some((header, payload))
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes(self.src_port)
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes(self.dst_port)
    }

    pub fn length_u16(&self) -> u16 {
        u16::from_be_bytes(self.length)
    }

    pub fn checksum_u16(&self) -> u16 {
        u16::from_be_bytes(self.checksum)
    }
}

// ---------------------------------------------------------------------------
// UDP checksum (RFC 768 + RFC 1071)
// ---------------------------------------------------------------------------

/// Compute UDP checksum with IPv4 pseudo-header.
/// If the computed checksum is 0, returns 0xFFFF (per RFC 768).
pub fn udp_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, udp_packet: &[u8]) -> u16 {
    let udp_len = udp_packet.len() as u16;

    // Build pseudo-header + UDP packet for checksum
    let total = 12 + udp_packet.len() + (udp_packet.len() % 2);
    let mut buf = Vec::with_capacity(total);

    // IPv4 pseudo-header (12 bytes)
    buf.extend_from_slice(&src_ip.0); // 4 bytes
    buf.extend_from_slice(&dst_ip.0); // 4 bytes
    buf.push(0); // zero
    buf.push(ipv4::PROTO_UDP); // protocol
    buf.extend_from_slice(&udp_len.to_be_bytes()); // UDP length

    // UDP packet (with checksum field set to 0)
    buf.extend_from_slice(udp_packet);

    // Pad to even length
    if buf.len() % 2 != 0 {
        buf.push(0);
    }

    let cksum = ipv4::internet_checksum(&buf);
    // Per RFC 768: if computed checksum is 0, transmit 0xFFFF
    if cksum == 0 {
        0xFFFF
    } else {
        cksum
    }
}

/// Verify the checksum of a received UDP packet.
/// A checksum of 0 in the header means "no checksum" (valid).
pub fn verify_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, udp_packet: &[u8]) -> bool {
    if udp_packet.len() < UDP_HEADER_SIZE {
        return false;
    }
    // If checksum field is 0, checksum was not computed (valid per RFC 768)
    let stored_cksum = u16::from_be_bytes([udp_packet[6], udp_packet[7]]);
    if stored_cksum == 0 {
        return true;
    }
    // Compute over the whole packet including stored checksum — should be 0
    let udp_len = udp_packet.len() as u16;
    let total = 12 + udp_packet.len() + (udp_packet.len() % 2);
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(&src_ip.0);
    buf.extend_from_slice(&dst_ip.0);
    buf.push(0);
    buf.push(ipv4::PROTO_UDP);
    buf.extend_from_slice(&udp_len.to_be_bytes());
    buf.extend_from_slice(udp_packet);
    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    ipv4::internet_checksum(&buf) == 0
}

// ---------------------------------------------------------------------------
// Build UDP packet
// ---------------------------------------------------------------------------

/// Build a UDP packet (header + payload) into the provided buffer.
/// Checksum is set to 0 (disabled). Use `build_packet_with_checksum`
/// for proper checksumming.
/// Returns the total length written.
pub fn build_packet(src_port: u16, dst_port: u16, payload: &[u8], buf: &mut [u8]) -> usize {
    let total_len = UDP_HEADER_SIZE + payload.len();
    assert!(buf.len() >= total_len);

    // Source port
    buf[0..2].copy_from_slice(&src_port.to_be_bytes());
    // Destination port
    buf[2..4].copy_from_slice(&dst_port.to_be_bytes());
    // Length
    buf[4..6].copy_from_slice(&(total_len as u16).to_be_bytes());
    // Checksum (0 = disabled)
    buf[6..8].copy_from_slice(&[0, 0]);
    // Payload
    buf[8..8 + payload.len()].copy_from_slice(payload);

    total_len
}

/// Build a UDP packet with proper checksum.
/// Returns the packet as a Vec.
pub fn build_packet_with_checksum(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let total_len = UDP_HEADER_SIZE + payload.len();
    let mut pkt = Vec::with_capacity(total_len);

    // Header with checksum = 0 initially
    pkt.extend_from_slice(&src_port.to_be_bytes());
    pkt.extend_from_slice(&dst_port.to_be_bytes());
    pkt.extend_from_slice(&(total_len as u16).to_be_bytes());
    pkt.extend_from_slice(&[0, 0]); // checksum placeholder
    pkt.extend_from_slice(payload);

    // Compute and fill checksum
    let cksum = udp_checksum(src_ip, dst_ip, &pkt);
    pkt[6] = (cksum >> 8) as u8;
    pkt[7] = cksum as u8;

    pkt
}

// ---------------------------------------------------------------------------
// UDP binding table
// ---------------------------------------------------------------------------

/// UDP socket binding entry
struct UdpBinding {
    /// Local port
    port: u16,
    /// Receive queue: (src_ip, src_port, data)
    recv_queue: Vec<(Ipv4Addr, u16, Vec<u8>)>,
    /// Maximum queue depth
    max_queue: usize,
    /// Multicast groups this binding has joined (as u32 for BTreeMap key)
    multicast_groups: Vec<u32>,
    /// Whether broadcast reception is enabled
    broadcast_enabled: bool,
}

impl UdpBinding {
    fn new(port: u16) -> Self {
        UdpBinding {
            port,
            recv_queue: Vec::new(),
            max_queue: 128,
            multicast_groups: Vec::new(),
            broadcast_enabled: false,
        }
    }
}

/// Global UDP binding table: port -> binding
static UDP_BINDINGS: Mutex<BTreeMap<u16, UdpBinding>> = Mutex::new(BTreeMap::new());

/// Bind a UDP port for receiving.
pub fn bind(port: u16) -> Result<(), NetError> {
    let mut bindings = UDP_BINDINGS.lock();
    if bindings.contains_key(&port) {
        return Err(NetError::AddrInUse);
    }
    bindings.insert(port, UdpBinding::new(port));
    Ok(())
}

/// Unbind a UDP port.
pub fn unbind(port: u16) {
    UDP_BINDINGS.lock().remove(&port);
}

/// Outcome of a deliver operation (used to release the lock before calling stats)
enum DeliverResult {
    Delivered,
    QueueFull,
    NoPort,
    NotSubscribed,
}

/// Deliver a received datagram to the correct binding.
/// Called by the network stack when a UDP packet arrives for a bound port.
pub fn deliver(dst_port: u16, src_ip: Ipv4Addr, src_port: u16, data: &[u8]) {
    stat_rx(UDP_HEADER_SIZE + data.len());
    let result = {
        let mut bindings = UDP_BINDINGS.lock();
        if let Some(binding) = bindings.get_mut(&dst_port) {
            if binding.recv_queue.len() < binding.max_queue {
                binding.recv_queue.push((src_ip, src_port, Vec::from(data)));
                DeliverResult::Delivered
            } else {
                DeliverResult::QueueFull
            }
        } else {
            DeliverResult::NoPort
        }
    };
    match result {
        DeliverResult::QueueFull => stat_rx_drop(),
        DeliverResult::NoPort => stat_no_port(),
        _ => {}
    }
}

/// Deliver a multicast datagram to all bindings that joined the group.
pub fn deliver_multicast(
    group_ip: Ipv4Addr,
    dst_port: u16,
    src_ip: Ipv4Addr,
    src_port: u16,
    data: &[u8],
) {
    stat_rx(UDP_HEADER_SIZE + data.len());
    let group_key = group_ip.to_u32();
    let result = {
        let mut bindings = UDP_BINDINGS.lock();
        if let Some(binding) = bindings.get_mut(&dst_port) {
            if binding.multicast_groups.contains(&group_key) {
                if binding.recv_queue.len() < binding.max_queue {
                    binding.recv_queue.push((src_ip, src_port, Vec::from(data)));
                    DeliverResult::Delivered
                } else {
                    DeliverResult::QueueFull
                }
            } else {
                DeliverResult::NotSubscribed
            }
        } else {
            DeliverResult::NoPort
        }
    };
    match result {
        DeliverResult::QueueFull => stat_rx_drop(),
        DeliverResult::NoPort | DeliverResult::NotSubscribed => stat_no_port(),
        _ => {}
    }
}

/// Deliver a broadcast datagram to all bindings on that port with broadcast enabled.
pub fn deliver_broadcast(dst_port: u16, src_ip: Ipv4Addr, src_port: u16, data: &[u8]) {
    stat_rx(UDP_HEADER_SIZE + data.len());
    let result = {
        let mut bindings = UDP_BINDINGS.lock();
        if let Some(binding) = bindings.get_mut(&dst_port) {
            if binding.broadcast_enabled {
                if binding.recv_queue.len() < binding.max_queue {
                    binding.recv_queue.push((src_ip, src_port, Vec::from(data)));
                    DeliverResult::Delivered
                } else {
                    DeliverResult::QueueFull
                }
            } else {
                // Port exists but broadcast not enabled — silently ignore
                DeliverResult::Delivered
            }
        } else {
            DeliverResult::NoPort
        }
    };
    match result {
        DeliverResult::QueueFull => stat_rx_drop(),
        DeliverResult::NoPort => stat_no_port(),
        _ => {}
    }
}

/// Receive a datagram from a bound port. Non-blocking: returns None if empty.
pub fn recv(port: u16) -> Option<(Ipv4Addr, u16, Vec<u8>)> {
    let mut bindings = UDP_BINDINGS.lock();
    if let Some(binding) = bindings.get_mut(&port) {
        if binding.recv_queue.is_empty() {
            None
        } else {
            Some(binding.recv_queue.remove(0))
        }
    } else {
        None
    }
}

/// Peek at the next datagram without removing it.
pub fn peek(port: u16) -> Option<(Ipv4Addr, u16, usize)> {
    let bindings = UDP_BINDINGS.lock();
    if let Some(binding) = bindings.get(&port) {
        binding
            .recv_queue
            .first()
            .map(|(ip, p, data)| (*ip, *p, data.len()))
    } else {
        None
    }
}

/// Get the number of queued datagrams for a port.
pub fn queue_len(port: u16) -> usize {
    let bindings = UDP_BINDINGS.lock();
    bindings.get(&port).map_or(0, |b| b.recv_queue.len())
}

// ---------------------------------------------------------------------------
// Multicast
// ---------------------------------------------------------------------------

/// Join a multicast group on a bound port.
/// Multicast addresses are in the range 224.0.0.0 - 239.255.255.255.
pub fn multicast_join(port: u16, group: Ipv4Addr) -> Result<(), NetError> {
    if !is_multicast(group) {
        return Err(NetError::InvalidPacket);
    }
    let group_key = group.to_u32();
    let mut bindings = UDP_BINDINGS.lock();
    let binding = bindings.get_mut(&port).ok_or(NetError::AddrNotAvailable)?;
    if !binding.multicast_groups.contains(&group_key) {
        binding.multicast_groups.push(group_key);
    }
    Ok(())
}

/// Leave a multicast group on a bound port.
pub fn multicast_leave(port: u16, group: Ipv4Addr) -> Result<(), NetError> {
    let group_key = group.to_u32();
    let mut bindings = UDP_BINDINGS.lock();
    let binding = bindings.get_mut(&port).ok_or(NetError::AddrNotAvailable)?;
    binding.multicast_groups.retain(|&g| g != group_key);
    Ok(())
}

/// List multicast groups for a port.
pub fn multicast_list(port: u16) -> Vec<Ipv4Addr> {
    let bindings = UDP_BINDINGS.lock();
    if let Some(binding) = bindings.get(&port) {
        binding
            .multicast_groups
            .iter()
            .map(|&g| Ipv4Addr::from_u32(g))
            .collect()
    } else {
        Vec::new()
    }
}

/// Check if an IP address is in the multicast range (224.0.0.0/4).
pub fn is_multicast(addr: Ipv4Addr) -> bool {
    addr.0[0] >= 224 && addr.0[0] <= 239
}

// ---------------------------------------------------------------------------
// Broadcast
// ---------------------------------------------------------------------------

/// Enable broadcast reception on a bound port.
pub fn enable_broadcast(port: u16) -> Result<(), NetError> {
    let mut bindings = UDP_BINDINGS.lock();
    let binding = bindings.get_mut(&port).ok_or(NetError::AddrNotAvailable)?;
    binding.broadcast_enabled = true;
    Ok(())
}

/// Disable broadcast reception on a bound port.
pub fn disable_broadcast(port: u16) -> Result<(), NetError> {
    let mut bindings = UDP_BINDINGS.lock();
    let binding = bindings.get_mut(&port).ok_or(NetError::AddrNotAvailable)?;
    binding.broadcast_enabled = false;
    Ok(())
}

/// Check if an address is a broadcast address for a given subnet.
pub fn is_broadcast(addr: Ipv4Addr, netmask: Ipv4Addr) -> bool {
    if addr == Ipv4Addr::BROADCAST {
        return true;
    }
    // Directed broadcast: host part is all 1s
    let addr_u32 = addr.to_u32();
    let mask_u32 = netmask.to_u32();
    let host_part = addr_u32 & !mask_u32;
    let max_host = !mask_u32;
    host_part == max_host && max_host != 0
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// UDP statistics
struct UdpStats {
    rx_packets: u64,
    tx_packets: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_errors: u64,
    tx_errors: u64,
    rx_dropped: u64,
    checksum_errors: u64,
    no_port: u64,
}

static UDP_STATS: Mutex<UdpStats> = Mutex::new(UdpStats {
    rx_packets: 0,
    tx_packets: 0,
    rx_bytes: 0,
    tx_bytes: 0,
    rx_errors: 0,
    tx_errors: 0,
    rx_dropped: 0,
    checksum_errors: 0,
    no_port: 0,
});

/// Record a received packet
pub fn stat_rx(bytes: usize) {
    let mut stats = UDP_STATS.lock();
    stats.rx_packets = stats.rx_packets.saturating_add(1);
    stats.rx_bytes = stats.rx_bytes.saturating_add(bytes as u64);
}

/// Record a transmitted packet
pub fn stat_tx(bytes: usize) {
    let mut stats = UDP_STATS.lock();
    stats.tx_packets = stats.tx_packets.saturating_add(1);
    stats.tx_bytes = stats.tx_bytes.saturating_add(bytes as u64);
}

/// Record a receive error
pub fn stat_rx_error() {
    let mut s = UDP_STATS.lock();
    s.rx_errors = s.rx_errors.saturating_add(1);
}

/// Record a checksum error
pub fn stat_checksum_error() {
    let mut s = UDP_STATS.lock();
    s.checksum_errors = s.checksum_errors.saturating_add(1);
}

/// Record a "no port" drop (no binding for destination port)
pub fn stat_no_port() {
    let mut s = UDP_STATS.lock();
    s.no_port = s.no_port.saturating_add(1);
}

/// Record a dropped packet (queue full)
pub fn stat_rx_drop() {
    let mut s = UDP_STATS.lock();
    s.rx_dropped = s.rx_dropped.saturating_add(1);
}

/// Get a snapshot of UDP statistics.
pub fn get_stats() -> (u64, u64, u64, u64, u64, u64, u64, u64, u64) {
    let s = UDP_STATS.lock();
    (
        s.rx_packets,
        s.tx_packets,
        s.rx_bytes,
        s.tx_bytes,
        s.rx_errors,
        s.tx_errors,
        s.rx_dropped,
        s.checksum_errors,
        s.no_port,
    )
}

/// Reset all statistics.
pub fn reset_stats() {
    let mut s = UDP_STATS.lock();
    s.rx_packets = 0;
    s.tx_packets = 0;
    s.rx_bytes = 0;
    s.tx_bytes = 0;
    s.rx_errors = 0;
    s.tx_errors = 0;
    s.rx_dropped = 0;
    s.checksum_errors = 0;
    s.no_port = 0;
}

// ---------------------------------------------------------------------------
// Send helper
// ---------------------------------------------------------------------------

/// High-level UDP send: builds checksummed UDP datagram and hands it to the
/// network stack for IP encapsulation and transmission.
pub fn send_to(
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
    data: &[u8],
) -> Result<(), NetError> {
    if data.len() > MAX_UDP_PAYLOAD {
        return Err(NetError::BufferTooSmall);
    }
    stat_tx(UDP_HEADER_SIZE + data.len());
    // Delegate to the net module which holds the NIC driver reference.
    // super::send_udp builds the IPv4 + UDP packet (with checksum) and sends.
    super::send_udp(src_port, dst_ip, dst_port, data)
}

/// List all bound UDP ports.
pub fn list_bindings() -> Vec<u16> {
    UDP_BINDINGS.lock().keys().copied().collect()
}
