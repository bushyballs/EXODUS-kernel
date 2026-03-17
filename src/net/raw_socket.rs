use crate::sync::Mutex;
/// Raw socket — send/receive raw IP packets
///
/// Bypasses transport layer processing. Binds to an IP protocol number
/// and receives all packets matching that protocol. Supports multiple
/// sockets per protocol with demux to all matching sockets.
///
/// Inspired by: Linux AF_PACKET/SOCK_RAW, BSD raw sockets.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum raw sockets
const MAX_RAW_SOCKETS: usize = 64;

/// Maximum receive queue per socket
const MAX_RECV_QUEUE: usize = 256;

/// Maximum packet size
const MAX_PACKET_SIZE: usize = 65535;

/// Receive buffer high watermark (bytes)
const RECV_BUF_LIMIT: usize = 262144; // 256KB

// ---------------------------------------------------------------------------
// Socket options
// ---------------------------------------------------------------------------

/// Raw socket options
#[derive(Debug, Clone)]
pub struct RawSocketOptions {
    /// Include IP header in received packets
    pub ip_hdr_incl: bool,
    /// Bind to specific source address
    pub bind_addr: Option<[u8; 4]>,
    /// Bind to specific interface
    pub bind_iface: Option<u32>,
    /// Enable promiscuous mode
    pub promiscuous: bool,
    /// BPF filter (simplified: protocol match only)
    pub filter_proto: Option<u8>,
    /// Receive buffer size limit
    pub recv_buf_size: usize,
}

impl Default for RawSocketOptions {
    fn default() -> Self {
        RawSocketOptions {
            ip_hdr_incl: false,
            bind_addr: None,
            bind_iface: None,
            promiscuous: false,
            filter_proto: None,
            recv_buf_size: RECV_BUF_LIMIT,
        }
    }
}

// ---------------------------------------------------------------------------
// Received packet info
// ---------------------------------------------------------------------------

/// Information about a received packet
#[derive(Debug, Clone)]
pub struct RecvPacketInfo {
    /// Source IP address
    pub src_ip: [u8; 4],
    /// Destination IP address
    pub dst_ip: [u8; 4],
    /// IP protocol number
    pub protocol: u8,
    /// Interface index it arrived on
    pub iface_index: u32,
    /// TTL from IP header
    pub ttl: u8,
    /// Packet data (with or without IP header depending on IP_HDRINCL)
    pub data: Vec<u8>,
    /// Timestamp (tick)
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Raw socket
// ---------------------------------------------------------------------------

/// Internal socket state
struct RawSocketInner {
    /// Socket ID
    id: u32,
    /// Bound IP protocol number (0 = all)
    protocol: u8,
    /// Options
    options: RawSocketOptions,
    /// Receive queue
    recv_queue: Vec<RecvPacketInfo>,
    /// Total bytes in recv queue
    recv_bytes: usize,
    /// Statistics
    rx_packets: u64,
    rx_bytes: u64,
    tx_packets: u64,
    tx_bytes: u64,
    rx_dropped: u64,
    /// Whether socket is open
    open: bool,
}

/// Public raw socket handle
pub struct RawSocket {
    id: u32,
}

impl RawSocket {
    /// Create a new raw socket bound to an IP protocol number
    /// protocol = 0 means receive all protocols
    pub fn new(protocol: u8) -> Option<Self> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut()?;
        if sys.sockets.len() >= MAX_RAW_SOCKETS {
            serial_println!("  Raw: max sockets reached");
            return None;
        }

        let id = sys.next_id;
        sys.next_id = sys.next_id.saturating_add(1);

        sys.sockets.push(RawSocketInner {
            id,
            protocol,
            options: RawSocketOptions::default(),
            recv_queue: Vec::new(),
            recv_bytes: 0,
            rx_packets: 0,
            rx_bytes: 0,
            tx_packets: 0,
            tx_bytes: 0,
            rx_dropped: 0,
            open: true,
        });

        serial_println!("  Raw: socket {} created (proto={})", id, protocol);
        Some(RawSocket { id })
    }

    /// Set socket options
    pub fn set_options(&self, options: RawSocketOptions) -> Result<(), RawError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(RawError::NotInitialized)?;
        let sock = find_socket_mut(sys, self.id)?;
        sock.options = options;
        Ok(())
    }

    /// Set IP_HDRINCL option
    pub fn set_ip_hdr_incl(&self, enabled: bool) -> Result<(), RawError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(RawError::NotInitialized)?;
        let sock = find_socket_mut(sys, self.id)?;
        sock.options.ip_hdr_incl = enabled;
        Ok(())
    }

    /// Bind to a specific source address
    pub fn bind_addr(&self, addr: [u8; 4]) -> Result<(), RawError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(RawError::NotInitialized)?;
        let sock = find_socket_mut(sys, self.id)?;
        sock.options.bind_addr = Some(addr);
        Ok(())
    }

    /// Bind to a specific interface
    pub fn bind_iface(&self, iface: u32) -> Result<(), RawError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(RawError::NotInitialized)?;
        let sock = find_socket_mut(sys, self.id)?;
        sock.options.bind_iface = Some(iface);
        Ok(())
    }

    /// Send a raw packet
    ///
    /// If IP_HDRINCL is set, `data` must include the complete IP header.
    /// Otherwise, the kernel will prepend an IP header.
    pub fn send_raw(&self, _dst_ip: [u8; 4], data: &[u8]) -> Result<usize, RawError> {
        if data.len() > MAX_PACKET_SIZE {
            return Err(RawError::PacketTooLarge);
        }

        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(RawError::NotInitialized)?;
        let sock = find_socket_mut(sys, self.id)?;
        if !sock.open {
            return Err(RawError::SocketClosed);
        }

        sock.tx_packets = sock.tx_packets.saturating_add(1);
        sock.tx_bytes = sock.tx_bytes.saturating_add(data.len() as u64);

        // In a real implementation, we would construct or forward the packet
        // through the IP layer. For now, we just count it.
        // The actual sending would be done via crate::net::send_ip_frame()
        // after building the appropriate IP header.

        Ok(data.len())
    }

    /// Receive a raw packet (non-blocking)
    pub fn recv_raw(&self) -> Option<RecvPacketInfo> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut()?;
        let sock = find_socket_mut(sys, self.id).ok()?;
        if sock.recv_queue.is_empty() {
            return None;
        }
        let pkt = sock.recv_queue.remove(0);
        sock.recv_bytes = sock.recv_bytes.saturating_sub(pkt.data.len());
        Some(pkt)
    }

    /// Check if there are packets waiting
    pub fn has_data(&self) -> bool {
        let guard = SUBSYSTEM.lock();
        match guard.as_ref() {
            Some(sys) => sys
                .sockets
                .iter()
                .find(|s| s.id == self.id)
                .map(|s| !s.recv_queue.is_empty())
                .unwrap_or(false),
            None => false,
        }
    }

    /// Get receive queue depth
    pub fn recv_queue_len(&self) -> usize {
        let guard = SUBSYSTEM.lock();
        match guard.as_ref() {
            Some(sys) => sys
                .sockets
                .iter()
                .find(|s| s.id == self.id)
                .map(|s| s.recv_queue.len())
                .unwrap_or(0),
            None => 0,
        }
    }

    /// Get socket statistics
    pub fn stats(&self) -> Option<RawSocketStats> {
        let guard = SUBSYSTEM.lock();
        let sys = guard.as_ref()?;
        let sock = sys.sockets.iter().find(|s| s.id == self.id)?;
        Some(RawSocketStats {
            rx_packets: sock.rx_packets,
            rx_bytes: sock.rx_bytes,
            tx_packets: sock.tx_packets,
            tx_bytes: sock.tx_bytes,
            rx_dropped: sock.rx_dropped,
            recv_queue_len: sock.recv_queue.len(),
            recv_queue_bytes: sock.recv_bytes,
        })
    }

    /// Close the socket
    pub fn close(self) {
        let mut guard = SUBSYSTEM.lock();
        if let Some(sys) = guard.as_mut() {
            if let Some(sock) = sys.sockets.iter_mut().find(|s| s.id == self.id) {
                sock.open = false;
                sock.recv_queue.clear();
                sock.recv_bytes = 0;
            }
            sys.sockets.retain(|s| s.id != self.id);
            serial_println!("  Raw: socket {} closed", self.id);
        }
    }
}

/// Socket statistics
#[derive(Debug, Clone)]
pub struct RawSocketStats {
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
    pub rx_dropped: u64,
    pub recv_queue_len: usize,
    pub recv_queue_bytes: usize,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawError {
    NotInitialized,
    SocketNotFound,
    SocketClosed,
    PacketTooLarge,
    QueueFull,
    InvalidPacket,
    PermissionDenied,
}

// ---------------------------------------------------------------------------
// Global subsystem
// ---------------------------------------------------------------------------

struct RawSubsystem {
    sockets: Vec<RawSocketInner>,
    next_id: u32,
    tick: u64,
}

static SUBSYSTEM: Mutex<Option<RawSubsystem>> = Mutex::new(None);

fn find_socket_mut(sys: &mut RawSubsystem, id: u32) -> Result<&mut RawSocketInner, RawError> {
    sys.sockets
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(RawError::SocketNotFound)
}

/// Initialize the raw socket subsystem
pub fn init() {
    *SUBSYSTEM.lock() = Some(RawSubsystem {
        sockets: Vec::new(),
        next_id: 1,
        tick: 0,
    });
    serial_println!("  Net: raw socket subsystem initialized");
}

/// Deliver an incoming IP packet to all matching raw sockets
///
/// Called from the IP layer for every received packet.
pub fn deliver_packet(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    ttl: u8,
    iface_index: u32,
    ip_header: &[u8],
    payload: &[u8],
) {
    let mut guard = SUBSYSTEM.lock();
    let sys = match guard.as_mut() {
        Some(s) => s,
        None => return,
    };
    sys.tick = sys.tick.saturating_add(1);
    let tick = sys.tick;

    for sock in &mut sys.sockets {
        if !sock.open {
            continue;
        }

        // Protocol filter
        if sock.protocol != 0 && sock.protocol != protocol {
            continue;
        }

        // Additional filter
        if let Some(filter) = sock.options.filter_proto {
            if filter != protocol {
                continue;
            }
        }

        // Address filter
        if let Some(ref addr) = sock.options.bind_addr {
            if *addr != dst_ip {
                continue;
            }
        }

        // Interface filter
        if let Some(iface) = sock.options.bind_iface {
            if iface != iface_index {
                continue;
            }
        }

        // Build packet data
        let data = if sock.options.ip_hdr_incl {
            let mut full = Vec::with_capacity(ip_header.len() + payload.len());
            full.extend_from_slice(ip_header);
            full.extend_from_slice(payload);
            full
        } else {
            payload.to_vec()
        };

        // Check recv buffer limit
        if sock.recv_bytes + data.len() > sock.options.recv_buf_size {
            sock.rx_dropped = sock.rx_dropped.saturating_add(1);
            continue;
        }

        if sock.recv_queue.len() >= MAX_RECV_QUEUE {
            sock.rx_dropped = sock.rx_dropped.saturating_add(1);
            continue;
        }

        sock.rx_packets = sock.rx_packets.saturating_add(1);
        sock.rx_bytes = sock.rx_bytes.saturating_add(data.len() as u64);
        sock.recv_bytes = sock.recv_bytes.saturating_add(data.len());

        sock.recv_queue.push(RecvPacketInfo {
            src_ip,
            dst_ip,
            protocol,
            iface_index,
            ttl,
            data,
            timestamp: tick,
        });
    }
}

/// Get the number of active raw sockets
pub fn socket_count() -> usize {
    let guard = SUBSYSTEM.lock();
    match guard.as_ref() {
        Some(sys) => sys.sockets.len(),
        None => 0,
    }
}
