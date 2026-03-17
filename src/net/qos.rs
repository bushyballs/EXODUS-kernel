use crate::sync::Mutex;
/// Quality of Service — traffic control with qdiscs
///
/// Provides classful and classless queuing disciplines (qdiscs),
/// token bucket rate limiting, priority queues, DiffServ marking,
/// and ECN (Explicit Congestion Notification) support.
///
/// Inspired by: Linux tc (traffic control), DiffServ (RFC 2474),
/// ECN (RFC 3168). All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum queues per qdisc
const MAX_QUEUES: usize = 16;

/// Maximum qdiscs
const MAX_QDISCS: usize = 64;

/// Default queue length
const DEFAULT_QUEUE_LEN: usize = 1000;

/// Token bucket refill interval (ticks)
const TOKEN_REFILL_INTERVAL: u64 = 1;

// ---------------------------------------------------------------------------
// DiffServ Code Points (DSCP)
// ---------------------------------------------------------------------------

/// DSCP values (6-bit field in IP TOS byte)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dscp {
    /// Default / Best Effort (0)
    BestEffort,
    /// Expedited Forwarding (46) — low-latency
    Expedited,
    /// Assured Forwarding classes (AFxy)
    Af11,
    Af12,
    Af13,
    Af21,
    Af22,
    Af23,
    Af31,
    Af32,
    Af33,
    Af41,
    Af42,
    Af43,
    /// Class Selector (CSx)
    ClassSelector(u8),
}

impl Dscp {
    pub fn to_value(&self) -> u8 {
        match self {
            Dscp::BestEffort => 0,
            Dscp::Expedited => 46,
            Dscp::Af11 => 10,
            Dscp::Af12 => 12,
            Dscp::Af13 => 14,
            Dscp::Af21 => 18,
            Dscp::Af22 => 20,
            Dscp::Af23 => 22,
            Dscp::Af31 => 26,
            Dscp::Af32 => 28,
            Dscp::Af33 => 30,
            Dscp::Af41 => 34,
            Dscp::Af42 => 36,
            Dscp::Af43 => 38,
            Dscp::ClassSelector(cs) => (cs & 0x07) << 3,
        }
    }

    pub fn from_tos(tos: u8) -> Self {
        let dscp = (tos >> 2) & 0x3F;
        match dscp {
            0 => Dscp::BestEffort,
            46 => Dscp::Expedited,
            10 => Dscp::Af11,
            12 => Dscp::Af12,
            14 => Dscp::Af13,
            18 => Dscp::Af21,
            20 => Dscp::Af22,
            22 => Dscp::Af23,
            26 => Dscp::Af31,
            28 => Dscp::Af32,
            30 => Dscp::Af33,
            34 => Dscp::Af41,
            36 => Dscp::Af42,
            38 => Dscp::Af43,
            _ => {
                if dscp & 0x07 == 0 {
                    Dscp::ClassSelector((dscp >> 3) & 0x07)
                } else {
                    Dscp::BestEffort
                }
            }
        }
    }

    /// Map DSCP to priority band (0-3, where 0 is highest)
    pub fn to_priority_band(&self) -> u8 {
        match self {
            Dscp::Expedited => 0,
            Dscp::Af41 | Dscp::Af42 | Dscp::Af43 => 0,
            Dscp::Af31 | Dscp::Af32 | Dscp::Af33 => 1,
            Dscp::Af21 | Dscp::Af22 | Dscp::Af23 => 1,
            Dscp::Af11 | Dscp::Af12 | Dscp::Af13 => 2,
            Dscp::ClassSelector(cs) if *cs >= 6 => 0,
            Dscp::ClassSelector(cs) if *cs >= 4 => 1,
            Dscp::ClassSelector(cs) if *cs >= 2 => 2,
            _ => 3,
        }
    }
}

// ---------------------------------------------------------------------------
// ECN (Explicit Congestion Notification)
// ---------------------------------------------------------------------------

/// ECN field values (2-bit, in low bits of TOS byte)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecn {
    NotEct = 0b00, // Not ECN-Capable Transport
    Ect0 = 0b10,   // ECN-Capable Transport(0)
    Ect1 = 0b01,   // ECN-Capable Transport(1)
    Ce = 0b11,     // Congestion Experienced
}

impl Ecn {
    pub fn from_tos(tos: u8) -> Self {
        match tos & 0x03 {
            0b00 => Ecn::NotEct,
            0b10 => Ecn::Ect0,
            0b01 => Ecn::Ect1,
            0b11 => Ecn::Ce,
            _ => Ecn::NotEct,
        }
    }
}

/// Mark ECN congestion on an IP packet (set CE bits in TOS field)
pub fn mark_ecn_congestion(packet: &mut [u8], ip_offset: usize) -> bool {
    if packet.len() < ip_offset + 2 {
        return false;
    }
    let tos = packet[ip_offset + 1];
    let ecn = Ecn::from_tos(tos);
    match ecn {
        Ecn::Ect0 | Ecn::Ect1 => {
            // Mark as Congestion Experienced
            packet[ip_offset + 1] = (tos & 0xFC) | 0x03;
            true
        }
        _ => false, // Cannot mark non-ECT or already-CE packets
    }
}

/// Set DSCP value in an IP packet's TOS field
pub fn set_dscp(packet: &mut [u8], ip_offset: usize, dscp: Dscp) {
    if packet.len() < ip_offset + 2 {
        return;
    }
    let ecn_bits = packet[ip_offset + 1] & 0x03;
    packet[ip_offset + 1] = (dscp.to_value() << 2) | ecn_bits;
}

// ---------------------------------------------------------------------------
// Queued packet
// ---------------------------------------------------------------------------

/// A packet waiting in a qdisc queue
#[derive(Clone)]
struct QueuedPacket {
    data: Vec<u8>,
    priority: u8,
    enqueue_tick: u64,
    byte_len: usize,
}

// ---------------------------------------------------------------------------
// Token bucket filter
// ---------------------------------------------------------------------------

/// Token bucket rate limiter
#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// Rate in bytes per tick
    pub rate: u64,
    /// Maximum burst size in bytes
    pub burst: u64,
    /// Current tokens
    tokens: u64,
    /// Last refill tick
    last_refill: u64,
    /// Packets conforming
    pub conform_packets: u64,
    pub conform_bytes: u64,
    /// Packets exceeding
    pub exceed_packets: u64,
    pub exceed_bytes: u64,
}

impl TokenBucket {
    pub fn new(rate_bytes_per_tick: u64, burst_bytes: u64) -> Self {
        TokenBucket {
            rate: rate_bytes_per_tick,
            burst: burst_bytes,
            tokens: burst_bytes,
            last_refill: 0,
            conform_packets: 0,
            conform_bytes: 0,
            exceed_packets: 0,
            exceed_bytes: 0,
        }
    }

    /// Refill tokens based on elapsed ticks
    fn refill(&mut self, current_tick: u64) {
        let elapsed = current_tick.wrapping_sub(self.last_refill);
        if elapsed > 0 {
            let new_tokens = elapsed * self.rate;
            self.tokens = self.tokens.saturating_add(new_tokens).min(self.burst);
            self.last_refill = current_tick;
        }
    }

    /// Try to consume tokens for a packet. Returns true if allowed.
    pub fn consume(&mut self, bytes: u64, current_tick: u64) -> bool {
        self.refill(current_tick);
        if self.tokens >= bytes {
            self.tokens = self.tokens.saturating_sub(bytes);
            self.conform_packets = self.conform_packets.saturating_add(1);
            self.conform_bytes = self.conform_bytes.saturating_add(bytes);
            true
        } else {
            self.exceed_packets = self.exceed_packets.saturating_add(1);
            self.exceed_bytes = self.exceed_bytes.saturating_add(bytes);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Qdisc types
// ---------------------------------------------------------------------------

/// Qdisc type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QdiscType {
    /// FIFO (simple queue)
    Fifo,
    /// Priority queue (prio) — multiple bands served in order
    Prio,
    /// Token bucket filter
    Tbf,
    /// Stochastic Fair Queuing
    Sfq,
    /// Hierarchical Token Bucket
    Htb,
}

/// Qdisc instance
pub struct Qdisc {
    pub qdisc_type: QdiscType,
    pub iface_id: u32,
    pub handle: u32,
    /// Parent handle (0 = root)
    pub parent: u32,
    /// Priority bands (for Prio qdisc)
    bands: Vec<Vec<QueuedPacket>>,
    /// Number of bands
    num_bands: usize,
    /// Token bucket (for TBF/HTB)
    pub token_bucket: Option<TokenBucket>,
    /// Maximum queue length per band
    pub queue_limit: usize,
    /// Tick counter
    tick: u64,
    /// Statistics
    pub enqueued: u64,
    pub dequeued: u64,
    pub dropped: u64,
    pub bytes_enqueued: u64,
    pub bytes_dequeued: u64,
    /// ECN marking enabled
    pub ecn_enabled: bool,
    /// ECN threshold (queue occupancy fraction, 0-100)
    pub ecn_threshold: u8,
}

impl Qdisc {
    pub fn new_fifo(iface_id: u32, handle: u32, queue_limit: usize) -> Self {
        Qdisc {
            qdisc_type: QdiscType::Fifo,
            iface_id,
            handle,
            parent: 0,
            bands: alloc::vec![Vec::new()],
            num_bands: 1,
            token_bucket: None,
            queue_limit,
            tick: 0,
            enqueued: 0,
            dequeued: 0,
            dropped: 0,
            bytes_enqueued: 0,
            bytes_dequeued: 0,
            ecn_enabled: false,
            ecn_threshold: 80,
        }
    }

    pub fn new_prio(iface_id: u32, handle: u32, num_bands: usize, queue_limit: usize) -> Self {
        let mut bands = Vec::new();
        for _ in 0..num_bands {
            bands.push(Vec::new());
        }
        Qdisc {
            qdisc_type: QdiscType::Prio,
            iface_id,
            handle,
            parent: 0,
            bands,
            num_bands,
            token_bucket: None,
            queue_limit,
            tick: 0,
            enqueued: 0,
            dequeued: 0,
            dropped: 0,
            bytes_enqueued: 0,
            bytes_dequeued: 0,
            ecn_enabled: false,
            ecn_threshold: 80,
        }
    }

    pub fn new_tbf(iface_id: u32, handle: u32, rate: u64, burst: u64, queue_limit: usize) -> Self {
        Qdisc {
            qdisc_type: QdiscType::Tbf,
            iface_id,
            handle,
            parent: 0,
            bands: alloc::vec![Vec::new()],
            num_bands: 1,
            token_bucket: Some(TokenBucket::new(rate, burst)),
            queue_limit,
            tick: 0,
            enqueued: 0,
            dequeued: 0,
            dropped: 0,
            bytes_enqueued: 0,
            bytes_dequeued: 0,
            ecn_enabled: false,
            ecn_threshold: 80,
        }
    }

    /// Enqueue a packet
    pub fn enqueue(&mut self, data: Vec<u8>, priority: u8) -> Result<(), QosError> {
        self.tick = self.tick.saturating_add(1);
        let byte_len = data.len();

        let band = match self.qdisc_type {
            QdiscType::Prio => {
                let b = (priority as usize).min(self.num_bands - 1);
                b
            }
            _ => 0,
        };

        if band >= self.bands.len() {
            return Err(QosError::InvalidBand);
        }

        if self.bands[band].len() >= self.queue_limit {
            self.dropped = self.dropped.saturating_add(1);
            return Err(QosError::QueueFull);
        }

        // ECN marking check
        if self.ecn_enabled {
            let occupancy = (self.bands[band].len() * 100) / self.queue_limit;
            if occupancy >= self.ecn_threshold as usize {
                // Would mark ECN here if we had mutable access to the packet
            }
        }

        self.bands[band].push(QueuedPacket {
            data,
            priority,
            enqueue_tick: self.tick,
            byte_len,
        });

        self.enqueued = self.enqueued.saturating_add(1);
        self.bytes_enqueued = self.bytes_enqueued.saturating_add(byte_len as u64);
        Ok(())
    }

    /// Dequeue a packet (returns highest-priority available)
    pub fn dequeue(&mut self) -> Option<Vec<u8>> {
        self.tick = self.tick.saturating_add(1);

        // Rate limiting for TBF
        if let Some(ref mut tb) = self.token_bucket {
            // Find the first packet and check if tokens available
            for band in &self.bands {
                if let Some(pkt) = band.first() {
                    if !tb.consume(pkt.byte_len as u64, self.tick) {
                        return None; // rate limited
                    }
                    break;
                }
            }
        }

        // Dequeue from highest-priority non-empty band
        for band in &mut self.bands {
            if !band.is_empty() {
                let pkt = band.remove(0);
                self.dequeued = self.dequeued.saturating_add(1);
                self.bytes_dequeued = self.bytes_dequeued.saturating_add(pkt.byte_len as u64);
                return Some(pkt.data);
            }
        }

        None
    }

    /// Get queue occupancy
    pub fn queue_len(&self) -> usize {
        self.bands.iter().map(|b| b.len()).sum()
    }

    /// Get per-band occupancy
    pub fn band_lengths(&self) -> Vec<usize> {
        self.bands.iter().map(|b| b.len()).collect()
    }
}

// ---------------------------------------------------------------------------
// QoS policy
// ---------------------------------------------------------------------------

/// QoS classification rule
#[derive(Debug, Clone)]
pub struct QosPolicy {
    pub dscp: Dscp,
    pub rate_limit_kbps: u32,
    pub priority: u8,
    pub mark: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct QosInner {
    qdiscs: Vec<Qdisc>,
    policies: Vec<QosPolicy>,
    ecn_enabled: bool,
}

static QOS: Mutex<Option<QosInner>> = Mutex::new(None);

/// Initialize the QoS subsystem
pub fn init() {
    *QOS.lock() = Some(QosInner {
        qdiscs: Vec::new(),
        policies: Vec::new(),
        ecn_enabled: false,
    });
    serial_println!("  Net: QoS/traffic control subsystem initialized");
}

/// Add a qdisc
pub fn add_qdisc(qdisc: Qdisc) -> Result<u32, QosError> {
    let mut guard = QOS.lock();
    let inner = guard.as_mut().ok_or(QosError::NotInitialized)?;
    if inner.qdiscs.len() >= MAX_QDISCS {
        return Err(QosError::TooManyQdiscs);
    }
    let handle = qdisc.handle;
    inner.qdiscs.push(qdisc);
    Ok(handle)
}

/// Remove a qdisc by handle
pub fn remove_qdisc(handle: u32) -> Result<(), QosError> {
    let mut guard = QOS.lock();
    let inner = guard.as_mut().ok_or(QosError::NotInitialized)?;
    inner.qdiscs.retain(|q| q.handle != handle);
    Ok(())
}

/// Classify a packet based on its TOS byte
pub fn classify_packet(tos: u8) -> (Dscp, u8) {
    let dscp = Dscp::from_tos(tos);
    let band = dscp.to_priority_band();
    (dscp, band)
}

/// Add a QoS policy
pub fn add_policy(policy: QosPolicy) {
    let mut guard = QOS.lock();
    if let Some(inner) = guard.as_mut() {
        inner.policies.push(policy);
    }
}

/// Enable/disable ECN globally
pub fn set_ecn_enabled(enabled: bool) {
    let mut guard = QOS.lock();
    if let Some(inner) = guard.as_mut() {
        inner.ecn_enabled = enabled;
        serial_println!(
            "  QoS: ECN {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QosError {
    NotInitialized,
    QueueFull,
    InvalidBand,
    TooManyQdiscs,
    QdiscNotFound,
}
