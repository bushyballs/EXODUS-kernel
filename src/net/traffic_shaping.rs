use super::Ipv4Addr;
use crate::sync::Mutex;
/// Traffic shaping and QoS for Genesis — bandwidth management
///
/// Implements:
///   - Token bucket rate limiter (per-flow and global)
///   - Priority queuing with strict priority and weighted fair queuing
///   - Per-application bandwidth quotas with enforcement
///   - Traffic classification (by port, protocol, IP, app label)
///   - Bandwidth monitoring and statistics (Q16 fixed-point math)
///   - Burst allowance and shaping delay calculation
///
/// Inspired by: Linux tc/netem, FreeBSD dummynet, Cisco QoS. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// Q16 fixed-point helpers (i32 with 16 fractional bits)
// ============================================================================

/// Q16 shift amount
const Q16_SHIFT: i32 = 16;
/// Q16 representation of 1.0
const Q16_ONE: i32 = 1 << Q16_SHIFT; // 65536

/// Convert an integer to Q16
fn q16_from_int(val: i32) -> i32 {
    val << Q16_SHIFT
}

/// Convert Q16 to integer (truncate fractional part)
fn q16_to_int(val: i32) -> i32 {
    val >> Q16_SHIFT
}

/// Multiply two Q16 values
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

/// Divide two Q16 values
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

// ============================================================================
// Traffic priority levels
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Network control (routing, ARP, DHCP)
    Control = 7,
    /// Real-time voice/video
    RealTime = 6,
    /// Interactive (SSH, gaming)
    Interactive = 5,
    /// Streaming media
    Streaming = 4,
    /// Standard (HTTP, email)
    Standard = 3,
    /// Bulk transfer (FTP, backups)
    Bulk = 2,
    /// Best effort (default)
    BestEffort = 1,
    /// Scavenger (lowest priority, P2P)
    Scavenger = 0,
}

impl Priority {
    pub fn from_u8(v: u8) -> Self {
        match v {
            7 => Priority::Control,
            6 => Priority::RealTime,
            5 => Priority::Interactive,
            4 => Priority::Streaming,
            3 => Priority::Standard,
            2 => Priority::Bulk,
            1 => Priority::BestEffort,
            _ => Priority::Scavenger,
        }
    }
}

// ============================================================================
// Token bucket rate limiter
// ============================================================================

pub struct TokenBucket {
    /// Maximum tokens (bucket capacity) in bytes
    pub capacity: u32,
    /// Current available tokens in bytes
    pub tokens: u32,
    /// Refill rate: bytes per tick (Q16 fixed-point)
    pub rate_q16: i32,
    /// Accumulated fractional tokens (Q16)
    fractional_q16: i32,
    /// Burst allowance: extra tokens above rate for short bursts
    pub burst_allowance: u32,
    /// Total bytes that passed through
    pub bytes_passed: u64,
    /// Total bytes dropped due to rate limit
    pub bytes_dropped: u64,
    /// Total packets passed
    pub packets_passed: u64,
    /// Total packets dropped
    pub packets_dropped: u64,
}

impl TokenBucket {
    pub fn new(rate_bytes_per_sec: u32, burst_bytes: u32) -> Self {
        // Assume ~100 ticks per second for refill calculation
        let rate_per_tick = q16_div(q16_from_int(rate_bytes_per_sec as i32), q16_from_int(100));

        TokenBucket {
            capacity: burst_bytes,
            tokens: burst_bytes, // start full
            rate_q16: rate_per_tick,
            fractional_q16: 0,
            burst_allowance: burst_bytes.saturating_sub(rate_bytes_per_sec / 100),
            bytes_passed: 0,
            bytes_dropped: 0,
            packets_passed: 0,
            packets_dropped: 0,
        }
    }

    /// Attempt to consume tokens for a packet of given size
    /// Returns true if the packet is allowed, false if it should be dropped/delayed
    pub fn consume(&mut self, bytes: u32) -> bool {
        if self.tokens >= bytes {
            self.tokens = self.tokens.saturating_sub(bytes);
            self.bytes_passed = self.bytes_passed.saturating_add(bytes as u64);
            self.packets_passed = self.packets_passed.saturating_add(1);
            true
        } else {
            self.bytes_dropped = self.bytes_dropped.saturating_add(bytes as u64);
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            false
        }
    }

    /// Refill tokens (call once per tick)
    pub fn refill(&mut self) {
        let new_tokens_q16 = self.fractional_q16 + self.rate_q16;
        let whole_tokens = q16_to_int(new_tokens_q16);
        self.fractional_q16 = new_tokens_q16 - q16_from_int(whole_tokens);

        if whole_tokens > 0 {
            self.tokens = self.tokens.saturating_add(whole_tokens as u32);
            if self.tokens > self.capacity {
                self.tokens = self.capacity;
            }
        }
    }

    /// Get utilization as Q16 percentage (0 to Q16_ONE = 0% to 100%)
    pub fn utilization_q16(&self) -> i32 {
        let total = self.bytes_passed + self.bytes_dropped;
        if total == 0 {
            return 0;
        }
        q16_div(
            q16_from_int(self.bytes_passed as i32),
            q16_from_int(total as i32),
        )
    }
}

// ============================================================================
// Traffic classifier
// ============================================================================

#[derive(Debug, Clone)]
pub struct ClassifierRule {
    pub id: u32,
    pub label: String,
    pub match_criteria: MatchCriteria,
    pub priority: Priority,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct MatchCriteria {
    pub src_ip: Option<Ipv4Addr>,
    pub dst_ip: Option<Ipv4Addr>,
    pub src_port: Option<u16>,
    pub dst_port: Option<u16>,
    pub protocol: Option<u8>, // IP protocol number
}

impl MatchCriteria {
    pub fn matches(
        &self,
        src_ip: &Ipv4Addr,
        dst_ip: &Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        proto: u8,
    ) -> bool {
        if let Some(ref sip) = self.src_ip {
            if sip != src_ip {
                return false;
            }
        }
        if let Some(ref dip) = self.dst_ip {
            if dip != dst_ip {
                return false;
            }
        }
        if let Some(sp) = self.src_port {
            if sp != src_port {
                return false;
            }
        }
        if let Some(dp) = self.dst_port {
            if dp != dst_port {
                return false;
            }
        }
        if let Some(p) = self.protocol {
            if p != proto {
                return false;
            }
        }
        true
    }
}

/// Default classification based on well-known ports
fn classify_by_port(dst_port: u16) -> Priority {
    match dst_port {
        // Control plane
        53 | 67 | 68 => Priority::Control, // DNS, DHCP
        // Real-time
        5060 | 5061 => Priority::RealTime, // SIP
        // Interactive
        22 | 23 | 3389 => Priority::Interactive, // SSH, Telnet, RDP
        // Streaming
        554 | 1935 => Priority::Streaming, // RTSP, RTMP
        // Standard web
        80 | 443 | 8080 => Priority::Standard, // HTTP/HTTPS
        // Bulk transfer
        20 | 21 => Priority::Bulk, // FTP
        // Mail (low priority)
        25 | 110 | 143 | 587 | 993 => Priority::BestEffort, // SMTP, POP3, IMAP
        _ => Priority::BestEffort,
    }
}

// ============================================================================
// Priority queue
// ============================================================================

#[derive(Clone)]
pub struct QueuedPacket {
    pub data: Vec<u8>,
    pub priority: Priority,
    pub src_ip: Ipv4Addr,
    pub dst_ip: Ipv4Addr,
    pub src_port: u16,
    pub dst_port: u16,
    pub enqueue_time: u64,
}

pub struct PriorityQueue {
    /// One queue per priority level (index = priority value)
    queues: [Vec<QueuedPacket>; 8],
    /// Maximum queue depth per priority
    max_depth: usize,
    /// Total packets enqueued
    pub total_enqueued: u64,
    /// Total packets dequeued
    pub total_dequeued: u64,
    /// Total packets dropped (queue full)
    pub total_dropped: u64,
}

impl PriorityQueue {
    pub fn new(max_depth: usize) -> Self {
        PriorityQueue {
            queues: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            max_depth,
            total_enqueued: 0,
            total_dequeued: 0,
            total_dropped: 0,
        }
    }

    /// Enqueue a packet into the appropriate priority queue
    pub fn enqueue(&mut self, packet: QueuedPacket) -> bool {
        let idx = packet.priority as usize;
        if idx >= 8 {
            return false;
        }

        if self.queues[idx].len() >= self.max_depth {
            // Queue full — drop lowest priority first (tail drop)
            self.total_dropped = self.total_dropped.saturating_add(1);
            return false;
        }

        self.queues[idx].push(packet);
        self.total_enqueued = self.total_enqueued.saturating_add(1);
        true
    }

    /// Dequeue the highest-priority packet (strict priority scheduling)
    pub fn dequeue_strict(&mut self) -> Option<QueuedPacket> {
        // Check from highest priority (7) to lowest (0)
        for idx in (0..8).rev() {
            if !self.queues[idx].is_empty() {
                self.total_dequeued = self.total_dequeued.saturating_add(1);
                return Some(self.queues[idx].remove(0));
            }
        }
        None
    }

    /// Dequeue using weighted fair queuing
    /// Weights are proportional to priority level
    pub fn dequeue_wfq(&mut self, round: u64) -> Option<QueuedPacket> {
        // Weight = priority_level + 1 (so scavenger gets weight 1, control gets weight 8)
        // In each round, higher priority queues get more service opportunities
        for idx in (0..8).rev() {
            let weight = (idx as u64) + 1;
            // Serve this queue if (round % total_weight) falls in its slice
            if (round % 36) < weight * (weight + 1) / 2 {
                if !self.queues[idx].is_empty() {
                    self.total_dequeued = self.total_dequeued.saturating_add(1);
                    return Some(self.queues[idx].remove(0));
                }
            }
        }

        // Fallback: dequeue anything available
        self.dequeue_strict()
    }

    /// Get total packet count across all queues
    pub fn total_queued(&self) -> usize {
        self.queues.iter().map(|q| q.len()).sum()
    }

    /// Get per-priority queue depths
    pub fn queue_depths(&self) -> [usize; 8] {
        let mut depths = [0usize; 8];
        for i in 0..8 {
            depths[i] = self.queues[i].len();
        }
        depths
    }
}

// ============================================================================
// Per-application bandwidth quota
// ============================================================================

#[derive(Debug, Clone)]
pub struct AppQuota {
    pub label: String,
    pub max_bytes_per_sec: u32,
    pub current_bytes: u32,
    pub total_bytes: u64,
    pub bucket: u32, // index into token buckets
    pub enforced: bool,
}

// ============================================================================
// Traffic shaper (main structure)
// ============================================================================

pub struct TrafficShaper {
    /// Global rate limiter (outbound)
    pub global_limiter: TokenBucket,
    /// Per-flow rate limiters (keyed by flow ID)
    pub flow_limiters: BTreeMap<u32, TokenBucket>,
    /// Classifier rules
    pub classifier_rules: Vec<ClassifierRule>,
    /// Priority output queue
    pub priority_queue: PriorityQueue,
    /// Per-app quotas
    pub app_quotas: BTreeMap<String, AppQuota>,
    /// Next flow ID
    next_flow_id: u32,
    /// Next classifier rule ID
    next_rule_id: u32,
    /// Global tick counter (for refill timing)
    tick_counter: u64,
    /// Total bytes shaped
    pub total_bytes: u64,
    /// Total packets shaped
    pub total_packets: u64,
    /// Scheduling mode
    pub scheduling: SchedulingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingMode {
    StrictPriority,
    WeightedFairQueue,
}

impl TrafficShaper {
    pub fn new(global_rate_bps: u32, global_burst: u32) -> Self {
        TrafficShaper {
            global_limiter: TokenBucket::new(global_rate_bps, global_burst),
            flow_limiters: BTreeMap::new(),
            classifier_rules: Vec::new(),
            priority_queue: PriorityQueue::new(256),
            app_quotas: BTreeMap::new(),
            next_flow_id: 1,
            next_rule_id: 1,
            tick_counter: 0,
            total_bytes: 0,
            total_packets: 0,
            scheduling: SchedulingMode::StrictPriority,
        }
    }

    /// Classify a packet and determine its priority
    pub fn classify(
        &self,
        src_ip: &Ipv4Addr,
        dst_ip: &Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> Priority {
        // Check custom classifier rules first
        for rule in &self.classifier_rules {
            if rule.enabled
                && rule
                    .match_criteria
                    .matches(src_ip, dst_ip, src_port, dst_port, protocol)
            {
                return rule.priority;
            }
        }

        // Fall back to port-based classification
        classify_by_port(dst_port)
    }

    /// Submit a packet for shaping
    /// Returns true if the packet should be sent immediately, false if queued/dropped
    pub fn shape_packet(
        &mut self,
        data: &[u8],
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        protocol: u8,
    ) -> bool {
        let pkt_size = data.len() as u32;
        self.total_bytes = self.total_bytes.saturating_add(pkt_size as u64);
        self.total_packets = self.total_packets.saturating_add(1);

        // Classify
        let priority = self.classify(&src_ip, &dst_ip, src_port, dst_port, protocol);

        // Check global rate limit
        if !self.global_limiter.consume(pkt_size) {
            // Queue the packet for later transmission
            let pkt = QueuedPacket {
                data: Vec::from(data),
                priority,
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                enqueue_time: self.tick_counter,
            };
            self.priority_queue.enqueue(pkt);
            return false;
        }

        // Check per-app quota
        let app_label = format!("port_{}", dst_port);
        if let Some(quota) = self.app_quotas.get_mut(&app_label) {
            if quota.enforced && quota.current_bytes + pkt_size > quota.max_bytes_per_sec {
                return false; // quota exceeded
            }
            quota.current_bytes = quota.current_bytes.saturating_add(pkt_size);
            quota.total_bytes = quota.total_bytes.saturating_add(pkt_size as u64);
        }

        true
    }

    /// Dequeue the next packet that should be transmitted
    pub fn dequeue_next(&mut self) -> Option<QueuedPacket> {
        match self.scheduling {
            SchedulingMode::StrictPriority => self.priority_queue.dequeue_strict(),
            SchedulingMode::WeightedFairQueue => self.priority_queue.dequeue_wfq(self.tick_counter),
        }
    }

    /// Add a classifier rule
    pub fn add_classifier_rule(
        &mut self,
        label: &str,
        criteria: MatchCriteria,
        priority: Priority,
    ) -> u32 {
        let id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);
        self.classifier_rules.push(ClassifierRule {
            id,
            label: String::from(label),
            match_criteria: criteria,
            priority,
            enabled: true,
        });
        id
    }

    /// Add a per-flow rate limiter
    pub fn add_flow_limiter(&mut self, rate_bps: u32, burst: u32) -> u32 {
        let id = self.next_flow_id;
        self.next_flow_id = self.next_flow_id.saturating_add(1);
        self.flow_limiters
            .insert(id, TokenBucket::new(rate_bps, burst));
        id
    }

    /// Set a per-application bandwidth quota
    pub fn set_app_quota(&mut self, label: &str, max_bytes_per_sec: u32) {
        self.app_quotas.insert(
            String::from(label),
            AppQuota {
                label: String::from(label),
                max_bytes_per_sec,
                current_bytes: 0,
                total_bytes: 0,
                bucket: 0,
                enforced: true,
            },
        );
    }

    /// Periodic tick: refill token buckets, reset per-second counters
    pub fn tick(&mut self) {
        self.tick_counter = self.tick_counter.saturating_add(1);

        // Refill global limiter
        self.global_limiter.refill();

        // Refill per-flow limiters
        for bucket in self.flow_limiters.values_mut() {
            bucket.refill();
        }

        // Reset per-second app quota counters every 100 ticks (~1 second)
        if self.tick_counter % 100 == 0 {
            for quota in self.app_quotas.values_mut() {
                quota.current_bytes = 0;
            }
        }

        // Try to drain queued packets
        while let Some(pkt) = self.dequeue_next() {
            let pkt_size = pkt.data.len() as u32;
            if self.global_limiter.consume(pkt_size) {
                // Packet can be sent now — caller would transmit pkt.data
                // (In a full implementation, this would call send_raw)
            } else {
                // Still rate-limited, re-queue
                self.priority_queue.enqueue(pkt);
                break;
            }
        }
    }

    /// Get shaping statistics
    pub fn stats(&self) -> String {
        let util = q16_to_int(q16_mul(
            self.global_limiter.utilization_q16(),
            q16_from_int(100),
        ));
        format!(
            "QoS: total={}pkts/{}B queued={} dropped={} util={}% mode={:?}",
            self.total_packets,
            self.total_bytes,
            self.priority_queue.total_queued(),
            self.global_limiter.bytes_dropped,
            util,
            self.scheduling,
        )
    }

    /// Get per-priority queue stats
    pub fn queue_stats(&self) -> String {
        let depths = self.priority_queue.queue_depths();
        format!(
            "Queues: ctrl={} rt={} inter={} stream={} std={} bulk={} be={} scav={}",
            depths[7], depths[6], depths[5], depths[4], depths[3], depths[2], depths[1], depths[0],
        )
    }
}

// ============================================================================
// Global state
// ============================================================================

static SHAPER: Mutex<Option<TrafficShaper>> = Mutex::new(None);

pub fn init() {
    // Default: 100 Mbps global rate, 1 MB burst
    let shaper = TrafficShaper::new(12_500_000, 1_048_576);
    *SHAPER.lock() = Some(shaper);
    serial_println!("    [qos] Traffic shaping initialized (100Mbps, strict priority)");
}

/// Submit a packet through the traffic shaper
pub fn shape(
    data: &[u8],
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
) -> bool {
    SHAPER
        .lock()
        .as_mut()
        .map(|s| s.shape_packet(data, src_ip, dst_ip, src_port, dst_port, protocol))
        .unwrap_or(true) // if no shaper, allow everything
}

/// Periodic tick
pub fn tick() {
    if let Some(ref mut shaper) = *SHAPER.lock() {
        shaper.tick();
    }
}

/// Add a classifier rule
pub fn add_rule(label: &str, criteria: MatchCriteria, priority: Priority) -> u32 {
    SHAPER
        .lock()
        .as_mut()
        .map(|s| s.add_classifier_rule(label, criteria, priority))
        .unwrap_or(0)
}

/// Set per-app bandwidth quota
pub fn set_quota(label: &str, max_bytes_per_sec: u32) {
    if let Some(ref mut shaper) = *SHAPER.lock() {
        shaper.set_app_quota(label, max_bytes_per_sec);
    }
}

/// Set scheduling mode
pub fn set_scheduling(mode: SchedulingMode) {
    if let Some(ref mut shaper) = *SHAPER.lock() {
        shaper.scheduling = mode;
        serial_println!("  [qos] Scheduling mode set to {:?}", mode);
    }
}

/// Get traffic shaping stats
pub fn stats() -> String {
    SHAPER
        .lock()
        .as_ref()
        .map(|s| s.stats())
        .unwrap_or_else(|| String::from("QoS: not initialized"))
}

/// Get queue depth stats
pub fn queue_stats() -> String {
    SHAPER
        .lock()
        .as_ref()
        .map(|s| s.queue_stats())
        .unwrap_or_else(|| String::from("Queues: not initialized"))
}

/// Get global rate limiter utilization as a percentage (0-100, integer)
pub fn utilization_percent() -> i32 {
    SHAPER
        .lock()
        .as_ref()
        .map(|s| {
            q16_to_int(q16_mul(
                s.global_limiter.utilization_q16(),
                q16_from_int(100),
            ))
        })
        .unwrap_or(0)
}
