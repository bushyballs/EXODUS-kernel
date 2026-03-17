use super::ipv4;
use super::tcp_options::{
    build_data_options, build_sack_options, build_syn_options, sack_add_block, sack_coalesce,
    sack_covers, sack_prune, SackBlock, TcpOptions,
};
use super::{Ipv4Addr, NetError};
use crate::sync::Mutex;
/// TCP (Transmission Control Protocol) for Genesis
///
/// Full TCP implementation: 3-way handshake, data transfer, flow control,
/// congestion control, connection teardown.
///
/// TCP state machine (RFC 793):
///   CLOSED -> LISTEN -> SYN_RECEIVED -> ESTABLISHED -> FIN_WAIT_1 -> ...
///   CLOSED -> SYN_SENT -> ESTABLISHED -> ...
///
/// Features:
///   - Retransmission with exponential backoff (tick-based)
///   - Out-of-order segment reassembly (BTreeMap keyed by seq)
///   - Congestion control: slow start, congestion avoidance, fast retransmit/recovery
///   - Nagle's algorithm (coalesce small writes)
///   - Keep-alive probes
///   - TIME_WAIT cleanup
///   - TCP checksum with pseudo-header
///
/// Inspired by: BSD TCP (the original reference implementation),
/// Linux TCP (congestion control), lwIP (simplicity).
/// All code is original.
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
// Re-export the options parser so callers (e.g. net/mod.rs) can decode
// incoming option bytes and pass the result to apply_syn_options().
pub use super::tcp_options::parse_tcp_options;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum Segment Size (default, without options)
pub const DEFAULT_MSS: u16 = 1460;

/// Maximum number of retransmission attempts before giving up
const MAX_RETRIES: u32 = 12;

/// Initial retransmission timeout in ticks (1 tick = 1ms at 1000 Hz)
const INITIAL_RTO_TICKS: u64 = 1000; // 1 second

/// Maximum RTO in ticks (60 seconds)
const MAX_RTO_TICKS: u64 = 60_000;

/// Minimum RTO in ticks (200ms)
const MIN_RTO_TICKS: u64 = 200;

/// TIME_WAIT duration in ticks (2*MSL = 120 seconds)
const TIME_WAIT_TICKS: u64 = 120_000;

/// Keep-alive interval in ticks (75 seconds)
const KEEPALIVE_INTERVAL_TICKS: u64 = 75_000;

/// Keep-alive probe count before declaring dead
const KEEPALIVE_PROBES: u32 = 9;

/// Default receive window size
const DEFAULT_RCV_WND: u16 = 65535;

/// Default receive buffer capacity
const DEFAULT_RCV_BUF_CAP: usize = 65536;

/// Default send buffer capacity
const DEFAULT_SND_BUF_CAP: usize = 65536;

/// Duplicate ACK threshold for fast retransmit
const DUP_ACK_THRESHOLD: u32 = 3;

/// Initial slow-start threshold
const INITIAL_SSTHRESH: u32 = 65535;

/// Our advertised window scale shift (we ask for 7 → 128 × 65535 ≈ 8 MB receive buffer)
const OUR_WSCALE: u8 = 7;

/// Minimum RTO in milliseconds (RFC 6298 §2.4)
const RTO_MIN_MS: u32 = 200;

/// Maximum RTO in milliseconds (RFC 6298 §2.5 — 60 s is common; 120 s per RFC)
const RTO_MAX_MS: u32 = 120_000;

/// Delayed ACK timer: send a bare ACK after this many ms without piggybacking.
const DELAYED_ACK_MS: u32 = 200;

/// Number of unacknowledged data segments before forcing an immediate ACK.
const DELAYED_ACK_SEGS: u32 = 2;

// ---------------------------------------------------------------------------
// TCP header
// ---------------------------------------------------------------------------

/// TCP header (20 bytes minimum)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct TcpHeader {
    pub src_port: [u8; 2],
    pub dst_port: [u8; 2],
    pub seq_num: [u8; 4],
    pub ack_num: [u8; 4],
    pub data_offset_flags: [u8; 2], // data offset (4 bits) + reserved (3) + flags (9)
    pub window: [u8; 2],
    pub checksum: [u8; 2],
    pub urgent_ptr: [u8; 2],
}

/// TCP flags
pub mod flags {
    pub const FIN: u16 = 0x001;
    pub const SYN: u16 = 0x002;
    pub const RST: u16 = 0x004;
    pub const PSH: u16 = 0x008;
    pub const ACK: u16 = 0x010;
    pub const URG: u16 = 0x020;
}

impl TcpHeader {
    pub fn parse(data: &[u8]) -> Option<(&TcpHeader, &[u8])> {
        if data.len() < 20 {
            return None;
        }
        let header = unsafe { &*(data.as_ptr() as *const TcpHeader) };
        let data_offset = (header.data_offset_flags[0] >> 4) as usize * 4;
        if data.len() < data_offset {
            return None;
        }
        let payload = &data[data_offset..];
        Some((header, payload))
    }

    #[inline(always)]
    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes(self.src_port)
    }

    #[inline(always)]
    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes(self.dst_port)
    }

    pub fn seq(&self) -> u32 {
        u32::from_be_bytes(self.seq_num)
    }

    pub fn ack(&self) -> u32 {
        u32::from_be_bytes(self.ack_num)
    }

    pub fn tcp_flags(&self) -> u16 {
        u16::from_be_bytes(self.data_offset_flags) & 0x01FF
    }

    pub fn window_size(&self) -> u16 {
        u16::from_be_bytes(self.window)
    }

    pub fn has_flag(&self, flag: u16) -> bool {
        self.tcp_flags() & flag != 0
    }
}

// ---------------------------------------------------------------------------
// TCP checksum
// ---------------------------------------------------------------------------

/// Compute TCP checksum over pseudo-header + TCP header + data.
/// `src_ip` and `dst_ip` are the IPv4 addresses from the IP header.
/// `tcp_segment` is the full TCP segment (header + data).
// hot path: called for every outgoing TCP segment (~5K/s on an active connection)
#[inline(always)]
pub fn tcp_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, tcp_segment: &[u8]) -> u16 {
    let tcp_len = tcp_segment.len() as u16;
    let mut buf = Vec::with_capacity(12 + tcp_segment.len() + 1);
    // Pseudo-header
    buf.extend_from_slice(&src_ip.0);
    buf.extend_from_slice(&dst_ip.0);
    buf.push(0);
    buf.push(ipv4::PROTO_TCP);
    buf.extend_from_slice(&tcp_len.to_be_bytes());
    buf.extend_from_slice(tcp_segment);
    if buf.len() % 2 != 0 {
        buf.push(0);
    }
    ipv4::internet_checksum(&buf)
}

/// Build a raw TCP segment (header + data) with computed checksum.
/// Returns the segment bytes (no IP header).
pub fn build_segment(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flag_bits: u16,
    window: u16,
    data: &[u8],
) -> Vec<u8> {
    let data_offset_flags: u16 = (5 << 12) | (flag_bits & 0x01FF);
    let mut seg = Vec::with_capacity(20 + data.len());

    // Build header
    let mut hdr = [0u8; 20];
    hdr[0..2].copy_from_slice(&src_port.to_be_bytes());
    hdr[2..4].copy_from_slice(&dst_port.to_be_bytes());
    hdr[4..8].copy_from_slice(&seq.to_be_bytes());
    hdr[8..12].copy_from_slice(&ack.to_be_bytes());
    hdr[12..14].copy_from_slice(&data_offset_flags.to_be_bytes());
    hdr[14..16].copy_from_slice(&window.to_be_bytes());
    // checksum [16..18] = 0, urgent [18..20] = 0

    seg.extend_from_slice(&hdr);
    seg.extend_from_slice(data);

    // Compute and fill checksum
    let cksum = tcp_checksum(src_ip, dst_ip, &seg);
    seg[16] = (cksum >> 8) as u8;
    seg[17] = cksum as u8;

    seg
}

/// Verify the checksum of a received TCP segment.
pub fn verify_checksum(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, tcp_segment: &[u8]) -> bool {
    tcp_checksum(src_ip, dst_ip, tcp_segment) == 0
}

// ---------------------------------------------------------------------------
// TCP connection states (RFC 793 state machine)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

// ---------------------------------------------------------------------------
// 4-tuple identifying a connection
// ---------------------------------------------------------------------------

/// A 4-tuple identifying a TCP connection
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TcpTuple {
    pub local_ip: [u8; 4],
    pub local_port: u16,
    pub remote_ip: [u8; 4],
    pub remote_port: u16,
}

// ---------------------------------------------------------------------------
// Retransmission queue entry
// ---------------------------------------------------------------------------

/// A segment awaiting acknowledgement
#[derive(Clone)]
struct RetxEntry {
    /// Sequence number of the first byte in this segment
    seq: u32,
    /// The segment data (payload only, header rebuilt on retransmit)
    data: Vec<u8>,
    /// Tick when this segment was first sent
    sent_tick: u64,
    /// Tick when retransmission is due
    retx_tick: u64,
    /// Number of retransmission attempts so far
    retx_count: u32,
    /// TCP flags used when this segment was sent
    flags: u16,
}

// ---------------------------------------------------------------------------
// Out-of-order reassembly segment
// ---------------------------------------------------------------------------

/// An out-of-order segment waiting to be delivered
#[derive(Clone)]
struct OooSegment {
    /// Payload data
    data: Vec<u8>,
    /// Whether the FIN flag was on this segment
    has_fin: bool,
}

// ---------------------------------------------------------------------------
// Congestion control state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CongestionPhase {
    SlowStart,
    CongestionAvoidance,
    FastRecovery,
}

// ---------------------------------------------------------------------------
// TCP connection control block
// ---------------------------------------------------------------------------

/// TCP connection control block
pub struct TcpConnection {
    pub state: TcpState,
    pub local_port: u16,
    pub remote_ip: Ipv4Addr,
    pub remote_port: u16,

    // Send sequence space
    pub snd_una: u32, // oldest unacknowledged sequence number
    pub snd_nxt: u32, // next sequence number to send
    pub snd_wnd: u16, // send window size (peer's advertised window)

    // Receive sequence space
    pub rcv_nxt: u32, // next expected sequence number
    pub rcv_wnd: u16, // receive window size

    // Buffers
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,

    // Congestion control
    pub cwnd: u32,     // congestion window (bytes)
    pub ssthresh: u32, // slow start threshold (bytes)

    // --- New fields ---
    /// MSS for this connection (negotiated or default)
    mss: u16,

    /// Retransmission queue: segments sent but not yet ACKed
    retx_queue: Vec<RetxEntry>,

    /// Current retransmission timeout in ticks
    rto_ticks: u64,

    /// Smoothed RTT estimate in ticks (scaled by 8: srtt * 8)
    srtt_x8: u64,

    /// RTT variation (scaled by 4: rttvar * 4)
    rttvar_x4: u64,

    /// Whether we have an RTT measurement yet
    rtt_measured: bool,

    /// Out-of-order reassembly queue: BTreeMap<seq_number, OooSegment>
    ooo_queue: BTreeMap<u32, OooSegment>,

    /// Congestion control phase
    cong_phase: CongestionPhase,

    /// Duplicate ACK counter (for fast retransmit)
    dup_ack_count: u32,

    /// The ACK number of the last received ACK (to detect duplicates)
    last_ack_received: u32,

    /// Recovery point for fast recovery (snd_nxt at time of entering)
    recovery_point: u32,

    /// Bytes ACKed during congestion avoidance (for additive increase)
    ca_bytes_acked: u32,

    /// Nagle's algorithm: when true, coalesce small writes
    nagle_enabled: bool,

    /// Whether there is a small (< MSS) segment in flight (Nagle)
    nagle_pending: bool,

    /// Keep-alive enabled
    keepalive_enabled: bool,

    /// Tick when last data was received (for keep-alive timer)
    last_recv_tick: u64,

    /// Number of keep-alive probes sent without response
    keepalive_probes_sent: u32,

    /// TIME_WAIT expiry tick (when to transition to Closed)
    timewait_expiry: u64,

    /// Send buffer capacity
    snd_buf_cap: usize,

    /// Receive buffer capacity
    rcv_buf_cap: usize,

    /// TCP_NODELAY: disable Nagle when true
    nodelay: bool,

    /// Total bytes sent (for statistics)
    total_bytes_sent: u64,

    /// Total bytes received (for statistics)
    total_bytes_received: u64,

    /// Number of retransmissions (for statistics)
    retx_count_total: u64,

    // -----------------------------------------------------------------------
    // RFC 7323 — Window Scaling
    // -----------------------------------------------------------------------
    /// Our window scale shift that we advertise in SYN/SYN-ACK (0–14).
    /// Applied when we report rcv_wnd to the peer: peer sees (rcv_wnd << snd_wscale).
    pub snd_wscale: u8,

    /// Peer's window scale shift (from their SYN/SYN-ACK option).
    /// Applied when reading the advertised window from incoming segments.
    pub rcv_wscale: u8,

    /// True once window-scale negotiation has been completed (both sides offered it).
    wscale_ok: bool,

    // -----------------------------------------------------------------------
    // RFC 7323 — Timestamps
    // -----------------------------------------------------------------------
    /// Most recent valid timestamp value received from the peer (TSval).
    pub ts_recent: u32,

    /// Uptime_ms tick at which ts_recent was recorded (for PAWS aging).
    pub ts_recent_age: u64,

    /// ACK number that was current when we last sent a segment with ts_recent
    /// as TSecr (used to check if ts_recent is still "recent").
    pub last_ack_sent: u32,

    /// Our monotonically-increasing timestamp counter.
    /// Incremented by `tcp_timer_tick()` (typically every 1 ms → wraps in ~49 days).
    pub ts_val: u32,

    /// Whether the peer offered the timestamp option in its SYN/SYN-ACK.
    ts_ok: bool,

    // -----------------------------------------------------------------------
    // SACK (RFC 2018)
    // -----------------------------------------------------------------------
    /// True when the peer offered SACK in its SYN/SYN-ACK.
    pub sack_permitted: bool,

    /// SACK blocks for out-of-order data we have received (used in ACK options).
    pub rcv_sack_blocks: [SackBlock; 4],

    /// Number of valid entries in rcv_sack_blocks.
    pub rcv_sack_count: u8,

    // -----------------------------------------------------------------------
    // RFC 6298 — RTT estimation in milliseconds
    // -----------------------------------------------------------------------
    // The existing srtt_x8 / rttvar_x4 fields work in ticks (≈ ms when the
    // timer fires at 1 kHz).  These parallel fields keep the canonical ms-scaled
    // values that the task spec calls for, updated by rtt_update_ms().
    /// Smoothed RTT in ms × 8  (Q3 fixed-point, first measurement: SRTT = R).
    pub rtt_srtt: u32,

    /// RTT variance in ms × 4  (Q2 fixed-point, first measurement: RTTVAR = R/2).
    pub rtt_rttvar: u32,

    /// Retransmit timeout in milliseconds (clamped to [RTO_MIN_MS, RTO_MAX_MS]).
    pub rto_ms: u32,

    // -----------------------------------------------------------------------
    // Delayed ACK
    // -----------------------------------------------------------------------
    /// Number of data segments received since the last ACK was sent.
    delayed_ack_segs: u32,

    /// Uptime_ms tick when we last sent an ACK (used for the 200 ms timer).
    delayed_ack_last_sent: u64,
}

impl TcpConnection {
    pub fn new(local_port: u16) -> Self {
        TcpConnection {
            state: TcpState::Closed,
            local_port,
            remote_ip: Ipv4Addr::ANY,
            remote_port: 0,
            snd_una: 0,
            snd_nxt: 0,
            snd_wnd: 65535,
            rcv_nxt: 0,
            rcv_wnd: DEFAULT_RCV_WND,
            send_buf: Vec::new(),
            recv_buf: Vec::new(),
            cwnd: DEFAULT_MSS as u32,
            ssthresh: INITIAL_SSTHRESH,
            mss: DEFAULT_MSS,
            retx_queue: Vec::new(),
            rto_ticks: INITIAL_RTO_TICKS,
            srtt_x8: 0,
            rttvar_x4: 0,
            rtt_measured: false,
            ooo_queue: BTreeMap::new(),
            cong_phase: CongestionPhase::SlowStart,
            dup_ack_count: 0,
            last_ack_received: 0,
            recovery_point: 0,
            ca_bytes_acked: 0,
            nagle_enabled: true,
            nagle_pending: false,
            keepalive_enabled: false,
            last_recv_tick: 0,
            keepalive_probes_sent: 0,
            timewait_expiry: 0,
            snd_buf_cap: DEFAULT_SND_BUF_CAP,
            rcv_buf_cap: DEFAULT_RCV_BUF_CAP,
            nodelay: false,
            total_bytes_sent: 0,
            total_bytes_received: 0,
            retx_count_total: 0,
            // RFC 7323 — window scaling
            snd_wscale: OUR_WSCALE,
            rcv_wscale: 0,
            wscale_ok: false,
            // RFC 7323 — timestamps
            ts_recent: 0,
            ts_recent_age: 0,
            last_ack_sent: 0,
            ts_val: 0,
            ts_ok: false,
            // SACK
            sack_permitted: false,
            rcv_sack_blocks: [SackBlock { left: 0, right: 0 }; 4],
            rcv_sack_count: 0,
            // RFC 6298 RTT (ms-domain)
            rtt_srtt: 0,
            rtt_rttvar: 0,
            rto_ms: RTO_MIN_MS,
            // Delayed ACK
            delayed_ack_segs: 0,
            delayed_ack_last_sent: 0,
        }
    }

    // ----- RFC 7323 option negotiation -----

    /// Apply negotiated options from the peer's SYN or SYN-ACK.
    /// Call this once per connection, immediately after the handshake options
    /// are parsed.
    pub fn apply_syn_options(&mut self, opts: &TcpOptions) {
        // Window scaling — both sides must offer it for it to be active.
        if let Some(peer_scale) = opts.wscale {
            self.rcv_wscale = peer_scale;
            self.wscale_ok = true;
        } else {
            // Peer didn't offer → disable our own scaling too (RFC 7323 §2.2).
            self.snd_wscale = 0;
            self.rcv_wscale = 0;
            self.wscale_ok = false;
        }

        // Timestamps — both sides must offer it.
        if opts.ts_val.is_some() {
            self.ts_ok = true;
            // Capture peer's initial TSval as ts_recent.
            if let Some(tv) = opts.ts_val {
                self.ts_recent = tv;
                self.ts_recent_age = crate::time::clock::uptime_ms();
            }
        } else {
            self.ts_ok = false;
        }

        // MSS
        if let Some(peer_mss) = opts.mss {
            if peer_mss > 0 && peer_mss < self.mss {
                self.mss = peer_mss;
            }
        }

        // SACK
        self.sack_permitted = opts.sack_permitted;
    }

    /// Update ts_recent when a valid incoming timestamp is received (PAWS check).
    /// Returns false if the timestamp fails PAWS (segment should be discarded).
    pub fn paws_check(&mut self, ts_val: u32) -> bool {
        if !self.ts_ok {
            return true; // no timestamps in use — always accept
        }
        // RFC 7323 §5.3: discard if ts_val < ts_recent (wrapping comparison).
        // Exception: if ts_recent is older than 24 days (PAWS aging window),
        // treat it as valid to prevent permanent lock-out on quiet connections.
        let now = crate::time::clock::uptime_ms();
        let age_ms = now.saturating_sub(self.ts_recent_age);
        // 24 days = 24*86400*1000 = 2_073_600_000 ms; use wrapping comparison.
        let paws_ok = age_ms > 2_073_600_000 || (ts_val.wrapping_sub(self.ts_recent) as i32) >= 0;
        if paws_ok {
            self.ts_recent = ts_val;
            self.ts_recent_age = now;
        }
        paws_ok
    }

    /// Return the current timestamp echo-reply value (ts_recent) for embedding
    /// in outgoing segments.
    #[inline]
    pub fn ts_ecr(&self) -> u32 {
        self.ts_recent
    }

    /// Increment our timestamp counter by one tick.  Should be called once per
    /// millisecond from the network timer.
    #[inline]
    pub fn tick_ts(&mut self) {
        self.ts_val = self.ts_val.wrapping_add(1);
    }

    /// Compute the actual effective send window, applying window scaling.
    /// `raw_window` is the 16-bit value from the peer's TCP header.
    #[inline]
    pub fn scale_peer_window(&self, raw_window: u16) -> u32 {
        (raw_window as u32) << self.rcv_wscale
    }

    /// Our advertised window value to place in the TCP header window field
    /// (right-shifted by our scale factor so it fits in 16 bits).
    #[inline]
    pub fn advertise_window(&self) -> u16 {
        let scaled = (self.rcv_wnd as u32) >> self.snd_wscale;
        scaled.min(65535) as u16
    }

    // ----- RFC 6298 RTT estimation (ms-domain) -----

    /// Update the ms-domain SRTT/RTTVAR/RTO from a new RTT sample in milliseconds.
    ///
    /// Algorithm (Jacobson/Karels):
    ///   First measurement:  SRTT = R,  RTTVAR = R/2
    ///   Subsequent:         RTTVAR = (3/4)RTTVAR + (1/4)|SRTT - R|
    ///                       SRTT   = (7/8)SRTT   + (1/8)R
    ///   Always:             RTO = SRTT + 4*RTTVAR  (clamped to [min, max])
    ///
    /// Internal representation: rtt_srtt in ms×8 (Q3), rtt_rttvar in ms×4 (Q2).
    pub fn rtt_update_ms(&mut self, rtt_sample_ms: u32) {
        if rtt_sample_ms == 0 {
            return; // degenerate sample — ignore
        }

        if !self.rtt_measured {
            // First measurement.
            self.rtt_srtt = rtt_sample_ms * 8; // SRTT = R, scaled ×8
            self.rtt_rttvar = rtt_sample_ms * 4 / 2; // RTTVAR = R/2, scaled ×4
            self.rtt_measured = true;
        } else {
            // |SRTT - R| (unsigned absolute difference)
            let srtt_ms = self.rtt_srtt / 8;
            let diff = if rtt_sample_ms > srtt_ms {
                rtt_sample_ms - srtt_ms
            } else {
                srtt_ms - rtt_sample_ms
            };

            // RTTVAR = (3/4)*RTTVAR + (1/4)*|SRTT-R|
            // In Q2: rttvar_q2 = rttvar_q2 - rttvar_q2/4 + diff
            self.rtt_rttvar = self
                .rtt_rttvar
                .saturating_sub(self.rtt_rttvar / 4)
                .saturating_add(diff);

            // SRTT = (7/8)*SRTT + (1/8)*R
            // In Q3: srtt_q3 = srtt_q3 - srtt_q3/8 + sample
            self.rtt_srtt = self
                .rtt_srtt
                .saturating_sub(self.rtt_srtt / 8)
                .saturating_add(rtt_sample_ms);
        }

        // RTO = SRTT + max(1 ms, 4*RTTVAR)   [RFC 6298 §2.3]
        let srtt_ms = self.rtt_srtt / 8;
        let rttvar_ms = self.rtt_rttvar / 4;
        let k_rttvar = if rttvar_ms * 4 > 1 { rttvar_ms * 4 } else { 1 };
        let rto = srtt_ms.saturating_add(k_rttvar);
        self.rto_ms = rto.max(RTO_MIN_MS).min(RTO_MAX_MS);
    }

    /// Double the RTO (exponential backoff on retransmit).  Clamped to RTO_MAX_MS.
    pub fn rto_backoff(&mut self) {
        self.rto_ms = self.rto_ms.saturating_mul(2).min(RTO_MAX_MS);
    }

    /// Reset RTO backoff after a successful ACK (restore to computed value).
    pub fn rto_reset(&mut self) {
        // Re-derive from current SRTT/RTTVAR.
        let srtt_ms = self.rtt_srtt / 8;
        let rttvar_ms = self.rtt_rttvar / 4;
        if srtt_ms == 0 {
            self.rto_ms = RTO_MIN_MS;
        } else {
            let k = if rttvar_ms * 4 > 1 { rttvar_ms * 4 } else { 1 };
            self.rto_ms = srtt_ms.saturating_add(k).max(RTO_MIN_MS).min(RTO_MAX_MS);
        }
    }

    // ----- SACK block management -----

    /// Record a newly-received out-of-order segment as a SACK block.
    pub fn sack_record_ooo(&mut self, left: u32, right: u32) {
        if !self.sack_permitted {
            return;
        }
        sack_add_block(
            &mut self.rcv_sack_blocks,
            &mut self.rcv_sack_count,
            left,
            right,
        );
    }

    /// Merge overlapping / adjacent blocks (delegates to tcp_options helper).
    pub fn sack_merge(&mut self) {
        sack_coalesce(&mut self.rcv_sack_blocks, &mut self.rcv_sack_count);
    }

    /// Remove SACK blocks now covered by a cumulative ACK.
    pub fn sack_prune_by_ack(&mut self, ack_seq: u32) {
        sack_prune(&mut self.rcv_sack_blocks, &mut self.rcv_sack_count, ack_seq);
    }

    /// Return true if `seq` is already covered by a SACK block (skip retransmit).
    pub fn is_sacked(&self, seq: u32) -> bool {
        sack_covers(&self.rcv_sack_blocks, self.rcv_sack_count, seq)
    }

    // ----- Delayed ACK (RFC 1122 §4.2.3.2) -----

    /// Called when an in-order data segment is received.  Returns true if we
    /// should send an ACK immediately (Nagle-mirror rule: always ACK if
    /// ≥ 2 unACK'd segments or delayed-ACK timer has expired).
    pub fn delayed_ack_check(&mut self) -> bool {
        self.delayed_ack_segs = self.delayed_ack_segs.saturating_add(1);
        let now = crate::time::clock::uptime_ms();
        let elapsed_ms = now.saturating_sub(self.delayed_ack_last_sent);
        let force =
            self.delayed_ack_segs >= DELAYED_ACK_SEGS || elapsed_ms >= DELAYED_ACK_MS as u64;
        if force {
            self.delayed_ack_segs = 0;
            self.delayed_ack_last_sent = now;
        }
        force
    }

    /// Advance the delayed-ACK timer by `elapsed_ms`.  Returns true if the
    /// timer has fired and a bare ACK should be sent.
    pub fn delayed_ack_tick(&mut self, elapsed_ms: u32) -> bool {
        if self.delayed_ack_segs == 0 {
            return false; // nothing pending
        }
        let now = crate::time::clock::uptime_ms();
        let since = now.saturating_sub(self.delayed_ack_last_sent);
        if since >= DELAYED_ACK_MS as u64 {
            self.delayed_ack_segs = 0;
            self.delayed_ack_last_sent = now;
            return true;
        }
        // Suppress "unused" warning on elapsed_ms — it's an alternative API
        // for callers that track time themselves.
        let _ = elapsed_ms;
        false
    }

    /// Mark that an ACK has just been sent (resets the delayed-ACK counters).
    #[inline]
    pub fn note_ack_sent(&mut self) {
        self.delayed_ack_segs = 0;
        self.delayed_ack_last_sent = crate::time::clock::uptime_ms();
        self.last_ack_sent = self.rcv_nxt;
    }

    // ----- RTT estimation (Jacobson/Karels, RFC 6298) -----

    /// Update the RTT estimator with a new sample (in ticks).
    fn update_rtt(&mut self, sample_ticks: u64) {
        if !self.rtt_measured {
            // First measurement
            self.srtt_x8 = sample_ticks * 8;
            self.rttvar_x4 = sample_ticks * 2; // rttvar = sample / 2, scaled by 4 => *2
            self.rtt_measured = true;
        } else {
            // RTTVAR = (1-1/4)*RTTVAR + (1/4)*|SRTT - R|
            let srtt = self.srtt_x8 / 8;
            let diff = if sample_ticks > srtt {
                sample_ticks - srtt
            } else {
                srtt - sample_ticks
            };
            // rttvar_x4 = (3/4)*rttvar_x4 + (1/4)*(diff*4)
            //           = rttvar_x4 - rttvar_x4/4 + diff
            self.rttvar_x4 = self.rttvar_x4 - self.rttvar_x4 / 4 + diff;
            // SRTT = (1-1/8)*SRTT + (1/8)*R
            // srtt_x8 = srtt_x8 - srtt_x8/8 + sample
            self.srtt_x8 = self.srtt_x8 - self.srtt_x8 / 8 + sample_ticks;
        }
        // RTO = SRTT + max(G, 4*RTTVAR) where G=1 tick
        let srtt = self.srtt_x8 / 8;
        let rttvar = self.rttvar_x4 / 4;
        let k_rttvar = if rttvar * 4 > 1 { rttvar * 4 } else { 1 };
        self.rto_ticks = srtt + k_rttvar;
        if self.rto_ticks < MIN_RTO_TICKS {
            self.rto_ticks = MIN_RTO_TICKS;
        }
        if self.rto_ticks > MAX_RTO_TICKS {
            self.rto_ticks = MAX_RTO_TICKS;
        }
    }

    // ----- Congestion control -----

    /// Called when new data is acknowledged (`acked_bytes` = number of newly acked bytes).
    fn on_ack_received_cc(&mut self, acked_bytes: u32) {
        match self.cong_phase {
            CongestionPhase::SlowStart => {
                // cwnd += min(acked, MSS) for each ACK (exponential growth)
                let inc = if acked_bytes > self.mss as u32 {
                    self.mss as u32
                } else {
                    acked_bytes
                };
                self.cwnd = self.cwnd.saturating_add(inc);
                if self.cwnd >= self.ssthresh {
                    self.cong_phase = CongestionPhase::CongestionAvoidance;
                    self.ca_bytes_acked = 0;
                }
            }
            CongestionPhase::CongestionAvoidance => {
                // Additive increase: cwnd += MSS once per RTT (approximately 1 MSS per cwnd
                // bytes acked).  Use an accumulator: when we've acked at least cwnd bytes,
                // add one MSS to cwnd.
                self.ca_bytes_acked = self.ca_bytes_acked.saturating_add(acked_bytes);
                if self.ca_bytes_acked >= self.cwnd {
                    // Subtract cwnd bytes worth of credit and add one MSS
                    let old_cwnd = self.cwnd;
                    self.cwnd = self.cwnd.saturating_add(self.mss as u32);
                    // Retain any excess credit (avoid underflow with saturating_sub)
                    self.ca_bytes_acked = self.ca_bytes_acked.saturating_sub(old_cwnd);
                }
            }
            CongestionPhase::FastRecovery => {
                // Inflate cwnd by acked bytes (NewReno)
                self.cwnd = self.cwnd.saturating_add(acked_bytes);
            }
        }
    }

    /// Enter fast retransmit / fast recovery
    fn enter_fast_recovery(&mut self) {
        // ssthresh = max(FlightSize/2, 2*MSS)
        let flight = self.flight_size();
        let half_flight = flight / 2;
        let two_mss = self.mss as u32 * 2;
        self.ssthresh = if half_flight > two_mss {
            half_flight
        } else {
            two_mss
        };
        // cwnd = ssthresh + 3*MSS (account for 3 dup ACKs)
        self.cwnd = self.ssthresh + self.mss as u32 * 3;
        self.recovery_point = self.snd_nxt;
        self.cong_phase = CongestionPhase::FastRecovery;
    }

    /// Exit fast recovery (when ACK covers recovery_point)
    fn exit_fast_recovery(&mut self) {
        self.cwnd = self.ssthresh;
        self.cong_phase = CongestionPhase::CongestionAvoidance;
        self.ca_bytes_acked = 0;
        self.dup_ack_count = 0;
    }

    /// Handle a retransmission timeout (RTO expiry)
    fn on_rto_timeout(&mut self) {
        // ssthresh = max(FlightSize/2, 2*MSS)
        let flight = self.flight_size();
        let half_flight = flight / 2;
        let two_mss = self.mss as u32 * 2;
        self.ssthresh = if half_flight > two_mss {
            half_flight
        } else {
            two_mss
        };
        // cwnd = 1 MSS
        self.cwnd = self.mss as u32;
        self.cong_phase = CongestionPhase::SlowStart;
        self.dup_ack_count = 0;
    }

    /// Bytes in flight (sent but unacked)
    fn flight_size(&self) -> u32 {
        self.snd_nxt.wrapping_sub(self.snd_una)
    }

    /// Effective send window: min(cwnd, snd_wnd) - flight_size
    fn effective_window(&self) -> u32 {
        let win = if self.cwnd < self.snd_wnd as u32 {
            self.cwnd
        } else {
            self.snd_wnd as u32
        };
        let flight = self.flight_size();
        if win > flight {
            win - flight
        } else {
            0
        }
    }

    // ----- Out-of-order reassembly -----

    /// Insert an out-of-order segment and attempt to deliver contiguous data.
    fn insert_ooo_segment(&mut self, seq: u32, data: &[u8], has_fin: bool) {
        if data.is_empty() && !has_fin {
            return;
        }
        // Record a SACK block for the received out-of-order range so we can
        // inform the sender in the next ACK.
        if !data.is_empty() {
            let right = seq.wrapping_add(data.len() as u32);
            self.sack_record_ooo(seq, right);
        }
        self.ooo_queue.insert(
            seq,
            OooSegment {
                data: Vec::from(data),
                has_fin,
            },
        );
        self.deliver_ooo_segments();
    }

    /// Try to deliver contiguous segments from the OOO queue.
    ///
    /// The OOO queue is keyed by the first sequence number of each segment's
    /// data payload.  For a pure FIN (no data), the key is the FIN's own seq.
    fn deliver_ooo_segments(&mut self) {
        loop {
            // Peek at the lowest-seq segment in the queue.
            let front_seq = match self.ooo_queue.keys().next() {
                Some(&s) => s,
                None => break,
            };

            // A segment can only be delivered if its start is reachable —
            // i.e. front_seq <= rcv_nxt.  If there's a gap, stop.
            if seq_gt(front_seq, self.rcv_nxt) {
                break;
            }

            // Remove the segment from the queue.
            let seg = match self.ooo_queue.remove(&front_seq) {
                Some(s) => s,
                None => break,
            };

            if !seg.data.is_empty() {
                // Calculate how many leading bytes of this segment we already have.
                // wrapping_sub is safe: front_seq <= rcv_nxt is guaranteed above.
                let overlap = self.rcv_nxt.wrapping_sub(front_seq) as usize;

                if overlap < seg.data.len() {
                    // There are new bytes to deliver.
                    let usable = &seg.data[overlap..];
                    if self.recv_buf.len() + usable.len() <= self.rcv_buf_cap {
                        self.recv_buf.extend_from_slice(usable);
                        self.rcv_nxt = self.rcv_nxt.wrapping_add(usable.len() as u32);
                        self.total_bytes_received = self
                            .total_bytes_received
                            .saturating_add(usable.len() as u64);
                    }
                    // else: receive buffer full — data is lost; keep trying other segs
                }
                // If overlap >= seg.data.len() the segment is entirely a duplicate.
            }

            // FIN immediately follows the data.  The FIN is in-order once all
            // data bytes in this segment have been consumed.
            if seg.has_fin {
                let expected_fin_seq = front_seq.wrapping_add(seg.data.len() as u32);
                if expected_fin_seq == self.rcv_nxt {
                    // In-order FIN — consume it and transition state.
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(1);
                    // Only transition from ESTABLISHED; FinWait2 handles its own FIN.
                    if self.state == TcpState::Established {
                        self.state = TcpState::CloseWait;
                    } else if self.state == TcpState::FinWait2 {
                        self.enter_timewait();
                    }
                }
                // If not yet in order (gap), it stays removed from the queue — this
                // is acceptable since insert_ooo_segment will re-insert it if needed.
                // For correctness we stop here; the connection will receive another FIN.
                break;
            }
        }
    }

    // ----- Retransmission queue -----

    /// Enqueue a sent segment for potential retransmission.
    fn enqueue_retx(&mut self, seq: u32, data: &[u8], sent_flags: u16) {
        let now = crate::time::clock::uptime_ms();
        self.retx_queue.push(RetxEntry {
            seq,
            data: Vec::from(data),
            sent_tick: now,
            retx_tick: now.saturating_add(self.rto_ticks),
            retx_count: 0,
            flags: sent_flags,
        });
    }

    /// Remove all retransmission entries that have been fully acknowledged.
    /// Also computes RTT samples for Jacobson/Karels estimator (Karn's algorithm).
    fn ack_retx_queue(&mut self, ack_num: u32) {
        let now = crate::time::clock::uptime_ms();
        let mut rtt_sample_ticks: Option<u64> = None;
        let mut rtt_sample_ms: Option<u32> = None;

        self.retx_queue.retain(|entry| {
            let end_seq = entry.seq.wrapping_add(entry.data.len() as u32);
            // SYN and FIN each consume one sequence number beyond the data
            let extra: u32 = if entry.flags & (flags::SYN | flags::FIN) != 0 {
                1
            } else {
                0
            };
            let effective_end = end_seq.wrapping_add(extra);
            if seq_leq(effective_end, ack_num) {
                // Fully acknowledged.
                // Only sample RTT from non-retransmitted segments (Karn's algorithm):
                // retransmitted segments are ambiguous — we don't know which send
                // the ACK corresponds to.
                if entry.retx_count == 0 && rtt_sample_ticks.is_none() {
                    let sample = now.saturating_sub(entry.sent_tick);
                    rtt_sample_ticks = Some(sample);
                    // Ticks are ms at our 1 kHz resolution; saturate to u32 for ms domain.
                    rtt_sample_ms = Some(sample.min(u32::MAX as u64) as u32);
                }
                false // remove from queue
            } else {
                true // keep — not yet fully acked
            }
        });

        // Apply the RTT sample outside the closure (needs &mut self)
        if let Some(sample) = rtt_sample_ticks {
            if sample > 0 {
                self.update_rtt(sample); // tick-domain estimator (existing)
            }
        }
        if let Some(ms) = rtt_sample_ms {
            if ms > 0 {
                self.rtt_update_ms(ms); // ms-domain estimator (RFC 6298)
            }
        }

        // Prune SACK blocks now covered by the cumulative ACK
        self.sack_prune_by_ack(ack_num);
    }

    /// Check the retransmission queue for timed-out segments.
    /// Returns a list of (seq, data, flags) to retransmit.
    ///
    /// SACK-aware: segments whose first byte is already acknowledged by a SACK
    /// block are skipped (no needless retransmit).
    pub fn check_retransmissions(&mut self) -> Vec<(u32, Vec<u8>, u16)> {
        let now = crate::time::clock::uptime_ms();
        let mut retx_list = Vec::new();
        let mut any_rto = false;

        for entry in &mut self.retx_queue {
            if now >= entry.retx_tick {
                if entry.retx_count >= MAX_RETRIES {
                    // Give up — mark connection for reset
                    continue;
                }

                entry.retx_count = entry.retx_count.saturating_add(1);
                self.retx_count_total = self.retx_count_total.saturating_add(1);
                // Exponential backoff: new_rto = base_rto * 2^retx_count.
                // Cap the shift at 6 (2^6 = 64x) so the result never exceeds u64::MAX,
                // then clamp to MAX_RTO_TICKS.
                let backoff_shift = entry.retx_count.min(6) as u32;
                let new_rto = (self.rto_ticks << backoff_shift).min(MAX_RTO_TICKS);
                entry.retx_tick = now.saturating_add(new_rto);
                entry.sent_tick = now;
                retx_list.push((entry.seq, entry.data.clone(), entry.flags));
                any_rto = true;
            }
        }

        if any_rto {
            self.on_rto_timeout();
            self.rto_backoff(); // also back off the ms-domain RTO
        }

        retx_list
    }

    /// Check the retransmission queue for timed-out segments, filtering out any
    /// entries whose first byte is already acknowledged by a SACK block.
    ///
    /// This is the SACK-aware variant of check_retransmissions().  Use this
    /// instead when SACK is negotiated to avoid unnecessary retransmits.
    pub fn check_retransmissions_sack_aware(&mut self) -> Vec<(u32, Vec<u8>, u16)> {
        let now = crate::time::clock::uptime_ms();
        let mut retx_list = Vec::new();
        let mut any_rto = false;

        // Snapshot SACK state before the mutable borrow of retx_queue entries.
        let sack_blocks = self.rcv_sack_blocks;
        let sack_count = self.rcv_sack_count;

        for entry in &mut self.retx_queue {
            if now >= entry.retx_tick {
                if entry.retx_count >= MAX_RETRIES {
                    continue;
                }

                // If this segment's first byte is already SACK-acknowledged, skip it.
                if sack_covers(&sack_blocks, sack_count, entry.seq) {
                    // Reschedule so we don't spam the check every tick.
                    entry.retx_tick = now.saturating_add(self.rto_ticks);
                    continue;
                }

                entry.retx_count = entry.retx_count.saturating_add(1);
                self.retx_count_total = self.retx_count_total.saturating_add(1);
                let backoff_shift = entry.retx_count.min(6) as u32;
                let new_rto = (self.rto_ticks << backoff_shift).min(MAX_RTO_TICKS);
                entry.retx_tick = now.saturating_add(new_rto);
                entry.sent_tick = now;
                retx_list.push((entry.seq, entry.data.clone(), entry.flags));
                any_rto = true;
            }
        }

        if any_rto {
            self.on_rto_timeout();
            self.rto_backoff();
        }

        retx_list
    }

    /// Check if any retransmission entry has exceeded MAX_RETRIES.
    pub fn has_max_retries_exceeded(&self) -> bool {
        self.retx_queue.iter().any(|e| e.retx_count >= MAX_RETRIES)
    }

    // ----- Nagle's algorithm -----

    /// Decide whether to send data now or coalesce (Nagle).
    /// Returns true if we should send immediately.
    fn nagle_should_send(&self, data_len: usize) -> bool {
        if self.nodelay || !self.nagle_enabled {
            return true;
        }
        // Send if: data fills an MSS, or no unacked data
        if data_len >= self.mss as usize {
            return true;
        }
        if self.snd_una == self.snd_nxt {
            // No unacked data — safe to send small segment
            return true;
        }
        false
    }

    // ----- Keep-alive -----

    /// Check if a keep-alive probe should be sent. Returns true if so.
    pub fn check_keepalive(&mut self) -> bool {
        if !self.keepalive_enabled || self.state != TcpState::Established {
            return false;
        }
        let now = crate::time::clock::uptime_ms();
        if now.saturating_sub(self.last_recv_tick) >= KEEPALIVE_INTERVAL_TICKS {
            if self.keepalive_probes_sent < KEEPALIVE_PROBES {
                self.keepalive_probes_sent = self.keepalive_probes_sent.saturating_add(1);
                self.last_recv_tick = now; // reset timer for next probe
                return true;
            } else {
                // Too many probes without response — connection dead
                self.state = TcpState::Closed;
            }
        }
        false
    }

    /// Build a keep-alive probe segment (ACK with seq = snd_una - 1).
    pub fn build_keepalive_probe(&self) -> (u32, u32, u16, u16) {
        (
            self.snd_una.wrapping_sub(1), // seq
            self.rcv_nxt,                 // ack
            flags::ACK,                   // flags
            self.rcv_wnd,                 // window
        )
    }

    // ----- TIME_WAIT management -----

    /// Start TIME_WAIT timer.
    fn enter_timewait(&mut self) {
        self.state = TcpState::TimeWait;
        self.timewait_expiry = crate::time::clock::uptime_ms().saturating_add(TIME_WAIT_TICKS);
        self.retx_queue.clear(); // No more retransmissions in TIME_WAIT
    }

    /// Check if TIME_WAIT has expired.
    pub fn timewait_expired(&self) -> bool {
        if self.state != TcpState::TimeWait {
            return false;
        }
        crate::time::clock::uptime_ms() >= self.timewait_expiry
    }

    // ----- Data send (segmentation) -----

    /// Queue data for sending. Returns the number of bytes accepted.
    pub fn queue_send(&mut self, data: &[u8]) -> usize {
        let available = self.snd_buf_cap.saturating_sub(self.send_buf.len());
        let to_copy = data.len().min(available);
        self.send_buf.extend_from_slice(&data[..to_copy]);
        to_copy
    }

    /// Segment and prepare outbound data from the send buffer.
    /// Returns a Vec of (seq, segment_data, flags) ready to send.
    pub fn prepare_segments(&mut self) -> Vec<(u32, Vec<u8>, u16)> {
        let mut segments = Vec::new();

        if self.state != TcpState::Established && self.state != TcpState::CloseWait {
            return segments;
        }

        loop {
            if self.send_buf.is_empty() {
                break;
            }

            let eff_win = self.effective_window();
            if eff_win == 0 {
                break;
            }

            let mss = self.mss as usize;
            let can_send = (eff_win as usize).min(self.send_buf.len()).min(mss);

            if can_send == 0 {
                break;
            }

            // Nagle check for sub-MSS segments
            if can_send < mss && !self.nagle_should_send(can_send) {
                break;
            }

            // Extract data from send_buf
            let seg_data: Vec<u8> = self.send_buf.drain(..can_send).collect();

            let seq = self.snd_nxt;
            let mut seg_flags = flags::ACK;

            // Set PSH on the last segment if buffer is now empty
            if self.send_buf.is_empty() {
                seg_flags |= flags::PSH;
            }

            self.snd_nxt = self.snd_nxt.wrapping_add(can_send as u32);
            self.total_bytes_sent = self.total_bytes_sent.saturating_add(can_send as u64);

            // Enqueue for retransmission
            self.enqueue_retx(seq, &seg_data, seg_flags);

            segments.push((seq, seg_data, seg_flags));
        }

        segments
    }

    // ----- Segment processing -----

    /// Process an incoming TCP segment for this connection
    pub fn process_segment(&mut self, header: &TcpHeader, data: &[u8]) {
        let now = crate::time::clock::uptime_ms();
        self.last_recv_tick = now;
        self.keepalive_probes_sent = 0;

        match self.state {
            TcpState::Listen => {
                if header.has_flag(flags::SYN) {
                    // Incoming connection — prepare SYN+ACK
                    self.rcv_nxt = header.seq().wrapping_add(1);
                    self.snd_nxt = generate_isn();
                    self.state = TcpState::SynReceived;
                    self.remote_port = header.src_port();
                    self.snd_wnd = header.window_size();
                }
            }
            TcpState::SynSent => {
                if header.has_flag(flags::RST) {
                    // RFC 793: RST in SYN_SENT with matching ACK → connection refused
                    if header.has_flag(flags::ACK) {
                        let ack = header.ack();
                        if seq_leq(self.snd_una, ack) && seq_leq(ack, self.snd_nxt) {
                            self.state = TcpState::Closed;
                            self.retx_queue.clear();
                        }
                    }
                    return;
                }
                if header.has_flag(flags::SYN) && header.has_flag(flags::ACK) {
                    // SYN+ACK received — three-way handshake completing.
                    // Capture the RTT sample from the SYN before ack_retx_queue removes it.
                    let rtt_sample = self
                        .retx_queue
                        .first()
                        .filter(|e| e.retx_count == 0)
                        .map(|e| now.saturating_sub(e.sent_tick));

                    // Parse TCP options from the SYN+ACK (data_offset encodes option length).
                    let data_offset = (header.data_offset_flags[0] >> 4) as usize * 4;
                    // `data` here is the payload slice (options already stripped by
                    // TcpHeader::parse); we reconstruct the options bytes from the raw
                    // header.  Since process_segment receives (header, data) where data
                    // is post-option payload, we approximate by parsing what we have.
                    // For correctness, callers that have the raw segment bytes should call
                    // apply_syn_options() directly after parse_tcp_options().
                    // In this path `data` is the payload, so we fall back to a no-op
                    // if options are not passed separately.  The apply_syn_options() path
                    // is also available publicly for callers that do pass raw bytes.
                    let _ = data_offset; // used by external parse path

                    self.rcv_nxt = header.seq().wrapping_add(1);
                    self.snd_una = header.ack();
                    // Apply window scaling: peer's snd_wnd is raw; scale by rcv_wscale.
                    let raw_wnd = header.window_size();
                    self.snd_wnd = if self.wscale_ok {
                        self.scale_peer_window(raw_wnd).min(u16::MAX as u32) as u16
                    } else {
                        raw_wnd
                    };
                    self.state = TcpState::Established;
                    self.last_recv_tick = now;
                    self.rto_reset(); // reset ms-domain backoff

                    // Remove the SYN from the retransmission queue (it's now acked).
                    self.ack_retx_queue(header.ack());

                    // Apply RTT sample collected before the queue drain.
                    if let Some(sample) = rtt_sample {
                        if sample > 0 {
                            self.update_rtt(sample);
                            let ms = sample.min(u32::MAX as u64) as u32;
                            if ms > 0 {
                                self.rtt_update_ms(ms);
                            }
                        }
                    }
                } else if header.has_flag(flags::SYN) {
                    // Simultaneous open: both sides sent SYN at the same time.
                    // RFC 793: go to SYN_RECEIVED and send SYN+ACK.
                    self.rcv_nxt = header.seq().wrapping_add(1);
                    self.snd_wnd = header.window_size();
                    self.state = TcpState::SynReceived;
                }
            }
            TcpState::SynReceived => {
                if header.has_flag(flags::RST) {
                    // RFC 793: RST in SYN_RECEIVED — if we were a passive open (came from
                    // LISTEN), go back to LISTEN; if active open, go to CLOSED.
                    self.state = TcpState::Closed;
                    self.retx_queue.clear();
                    return;
                }
                if header.has_flag(flags::ACK) {
                    let ack = header.ack();
                    // The ACK must acknowledge our SYN (ack == snd_nxt, which was
                    // incremented by 1 when we set snd_nxt after generating the ISN).
                    if seq_leq(self.snd_una, ack) && seq_leq(ack, self.snd_nxt) {
                        self.snd_una = ack;
                        self.snd_wnd = header.window_size();
                        self.state = TcpState::Established;
                        self.last_recv_tick = now;
                        self.ack_retx_queue(ack);
                    }
                    // If FIN also set — simultaneous open completes with data
                    if header.has_flag(flags::FIN) {
                        self.rcv_nxt = header.seq().wrapping_add(data.len() as u32).wrapping_add(1);
                        self.state = TcpState::CloseWait;
                    }
                }
            }
            TcpState::Established => {
                self.process_established(header, data, now);
            }
            TcpState::FinWait1 => {
                if header.has_flag(flags::RST) {
                    self.state = TcpState::Closed;
                    self.retx_queue.clear();
                    return;
                }
                // Accept any data in flight before the peer's FIN
                if !data.is_empty() {
                    let seg_seq = header.seq();
                    if seg_seq == self.rcv_nxt {
                        if self.recv_buf.len() + data.len() <= self.rcv_buf_cap {
                            self.recv_buf.extend_from_slice(data);
                            self.rcv_nxt = self.rcv_nxt.wrapping_add(data.len() as u32);
                            self.total_bytes_received =
                                self.total_bytes_received.saturating_add(data.len() as u64);
                        }
                    }
                }
                if header.has_flag(flags::ACK) {
                    let ack = header.ack();
                    self.snd_una = ack;
                    self.ack_retx_queue(ack);
                    self.snd_wnd = header.window_size();

                    if header.has_flag(flags::FIN) {
                        // Simultaneous close — received FIN+ACK: our FIN is ack'd,
                        // peer's FIN arrived simultaneously → go to TIME_WAIT.
                        let fin_seq = header.seq().wrapping_add(data.len() as u32);
                        if fin_seq == self.rcv_nxt {
                            self.rcv_nxt = fin_seq.wrapping_add(1);
                        }
                        self.enter_timewait();
                    } else {
                        // Our FIN was acked → move to FIN_WAIT_2
                        self.state = TcpState::FinWait2;
                    }
                } else if header.has_flag(flags::FIN) {
                    // Peer sent FIN without acking ours → simultaneous close → CLOSING
                    let fin_seq = header.seq().wrapping_add(data.len() as u32);
                    if fin_seq == self.rcv_nxt {
                        self.rcv_nxt = fin_seq.wrapping_add(1);
                    }
                    self.state = TcpState::Closing;
                }
            }
            TcpState::FinWait2 => {
                if header.has_flag(flags::RST) {
                    self.state = TcpState::Closed;
                    self.retx_queue.clear();
                    return;
                }
                // Update send window / ACK
                if header.has_flag(flags::ACK) {
                    let ack = header.ack();
                    if seq_leq(self.snd_una, ack) && seq_leq(ack, self.snd_nxt) {
                        self.snd_una = ack;
                        self.snd_wnd = header.window_size();
                        self.ack_retx_queue(ack);
                    }
                }
                // Accept data segments still arriving from peer (RFC 793 allows this)
                if !data.is_empty() {
                    let seg_seq = header.seq();
                    if seg_seq == self.rcv_nxt {
                        if self.recv_buf.len() + data.len() <= self.rcv_buf_cap {
                            self.recv_buf.extend_from_slice(data);
                            self.rcv_nxt = self.rcv_nxt.wrapping_add(data.len() as u32);
                            self.total_bytes_received =
                                self.total_bytes_received.saturating_add(data.len() as u64);
                        }
                    } else if seq_gt(seg_seq, self.rcv_nxt) {
                        // Out of order — queue it
                        self.insert_ooo_segment(seg_seq, data, false);
                    }
                }
                if header.has_flag(flags::FIN) {
                    // FIN consumes one sequence number; data may precede it in the same
                    // segment — account for data already processed above.
                    let fin_seq = header.seq().wrapping_add(data.len() as u32);
                    if fin_seq == self.rcv_nxt {
                        // In-order FIN
                        self.rcv_nxt = fin_seq.wrapping_add(1);
                        self.enter_timewait();
                    } else if seq_gt(fin_seq, self.rcv_nxt) {
                        // Out-of-order FIN — queue as OOO with fin flag
                        self.insert_ooo_segment(header.seq(), data, true);
                    }
                    // else: duplicate FIN — ignore
                }
            }
            TcpState::CloseWait => {
                // Waiting for application to close
                if header.has_flag(flags::ACK) {
                    self.snd_una = header.ack();
                    self.ack_retx_queue(header.ack());
                }
            }
            TcpState::Closing => {
                if header.has_flag(flags::ACK) {
                    self.snd_una = header.ack();
                    self.ack_retx_queue(header.ack());
                    self.enter_timewait();
                }
            }
            TcpState::LastAck => {
                if header.has_flag(flags::ACK) {
                    self.state = TcpState::Closed;
                }
            }
            TcpState::TimeWait => {
                // RFC 793: if another FIN arrives, re-acknowledge it and restart
                // the TIME_WAIT timer (the peer may have missed our final ACK).
                if header.has_flag(flags::FIN) {
                    self.timewait_expiry = now.saturating_add(TIME_WAIT_TICKS);
                }
            }
            TcpState::Closed => {}
        }
    }

    /// Process a segment received while in the Established state.
    ///
    /// Enhancements over base RFC 793:
    ///   - Window scaling (RFC 7323): peer window is shifted by rcv_wscale.
    ///   - Timestamps / PAWS (RFC 7323): incoming TSval validated; ts_recent updated.
    ///   - SACK (RFC 2018): out-of-order ranges recorded; ACK pruning on new cum-ACK.
    ///   - Delayed ACK (RFC 1122): ACK sent immediately only when threshold/timer fires.
    ///   - ms-domain RTT update fed alongside the existing tick-domain estimator.
    fn process_established(&mut self, header: &TcpHeader, data: &[u8], now: u64) {
        // Handle RST
        if header.has_flag(flags::RST) {
            self.state = TcpState::Closed;
            self.retx_queue.clear();
            return;
        }

        // Update send window — apply peer's window scale if negotiated.
        {
            let raw_wnd = header.window_size();
            let effective_wnd = if self.wscale_ok {
                self.scale_peer_window(raw_wnd).min(u16::MAX as u32) as u16
            } else {
                raw_wnd
            };
            self.snd_wnd = effective_wnd;
        }

        // Process ACK
        if header.has_flag(flags::ACK) {
            let ack = header.ack();

            if seq_gt(ack, self.snd_una) && seq_leq(ack, self.snd_nxt) {
                // New data acknowledged
                let acked_bytes = ack.wrapping_sub(self.snd_una);

                // RTT measurement from retx queue (Karn's algorithm)
                // Collect sample before the mutable borrow in ack_retx_queue.
                let rtt_tick_sample: Option<u64> = self
                    .retx_queue
                    .iter()
                    .find(|e| {
                        let end = e.seq.wrapping_add(e.data.len() as u32);
                        seq_leq(end, ack) && e.retx_count == 0
                    })
                    .map(|e| now.saturating_sub(e.sent_tick));

                if let Some(sample) = rtt_tick_sample {
                    if sample > 0 {
                        self.update_rtt(sample); // tick-domain
                        let ms = sample.min(u32::MAX as u64) as u32;
                        if ms > 0 {
                            self.rtt_update_ms(ms);
                        } // ms-domain
                    }
                }

                self.snd_una = ack;
                self.ack_retx_queue(ack); // also calls sack_prune_by_ack

                // Congestion control
                if self.cong_phase == CongestionPhase::FastRecovery {
                    if seq_geq(ack, self.recovery_point) {
                        self.exit_fast_recovery();
                    }
                }
                self.on_ack_received_cc(acked_bytes);
                self.rto_reset(); // successful ACK — reset ms-domain backoff

                self.dup_ack_count = 0;
                self.last_ack_received = ack;
            } else if ack == self.snd_una && self.snd_una != self.snd_nxt {
                // Duplicate ACK
                self.dup_ack_count = self.dup_ack_count.saturating_add(1);

                if self.dup_ack_count == DUP_ACK_THRESHOLD
                    && self.cong_phase != CongestionPhase::FastRecovery
                {
                    // Enter fast retransmit / fast recovery
                    self.enter_fast_recovery();
                } else if self.dup_ack_count > DUP_ACK_THRESHOLD
                    && self.cong_phase == CongestionPhase::FastRecovery
                {
                    // Inflate cwnd for each additional dup ACK
                    self.cwnd = self.cwnd.saturating_add(self.mss as u32);
                }
            }
        }

        // Process data payload (may arrive together with FIN in the same segment)
        if !data.is_empty() {
            let seg_seq = header.seq();

            if seg_seq == self.rcv_nxt {
                // In-order delivery
                if self.recv_buf.len() + data.len() <= self.rcv_buf_cap {
                    self.recv_buf.extend_from_slice(data);
                    self.rcv_nxt = self.rcv_nxt.wrapping_add(data.len() as u32);
                    self.total_bytes_received =
                        self.total_bytes_received.saturating_add(data.len() as u64);
                }
                // Deliver any queued OOO segments that are now contiguous.
                // After delivery, prune SACK blocks that are now cumulatively acked.
                self.deliver_ooo_segments();
                self.sack_prune_by_ack(self.rcv_nxt);

                // Delayed ACK: signal that a segment arrived; caller decides whether
                // to send immediately.  We set a flag here and the transport driver
                // reads it via `delayed_ack_check()` before building the reply.
                // (The delayed_ack_check() call is idempotent and cheap.)
                let _ = self.delayed_ack_check(); // resets segs counter if threshold hit
            } else if seq_gt(seg_seq, self.rcv_nxt) {
                // Future data — queue for reassembly (carry FIN flag if present).
                // insert_ooo_segment() already calls sack_record_ooo() internally.
                self.insert_ooo_segment(seg_seq, data, header.has_flag(flags::FIN));
                // If we queued the segment including the FIN, we're done — the FIN
                // will be processed when deliver_ooo_segments() dequeues it.
                // Update receive window and return.
                let avail = self.rcv_buf_cap.saturating_sub(self.recv_buf.len());
                self.rcv_wnd = avail.min(65535) as u16;
                return;
            }
            // else: old/duplicate data — silently discard

            // Update receive window
            let avail = self.rcv_buf_cap.saturating_sub(self.recv_buf.len());
            self.rcv_wnd = avail.min(65535) as u16;
        }

        // Process FIN — by now any in-band data has been consumed and rcv_nxt updated.
        // The FIN's sequence number must equal the current rcv_nxt (i.e. it follows
        // all the data in this segment).
        if header.has_flag(flags::FIN) {
            // fin_seq is the sequence number of the FIN byte itself
            let fin_seq = header.seq().wrapping_add(data.len() as u32);
            if fin_seq == self.rcv_nxt {
                // In-order FIN — consume it
                self.rcv_nxt = fin_seq.wrapping_add(1);
                self.state = TcpState::CloseWait;
                // Deliver any OOO segments that may have been waiting for data
                // immediately before the FIN (shouldn't normally happen, but be safe).
                self.deliver_ooo_segments();
                // Update window after FIN
                let avail = self.rcv_buf_cap.saturating_sub(self.recv_buf.len());
                self.rcv_wnd = avail.min(65535) as u16;
            } else if seq_gt(fin_seq, self.rcv_nxt) {
                // Out-of-order FIN (gap before it) — already queued above if data was
                // present; if this is a pure FIN with a gap, queue it explicitly.
                if data.is_empty() {
                    self.insert_ooo_segment(fin_seq, &[], true);
                }
                // else: was already queued in the data path above
            }
            // else: duplicate or old FIN — ignore
        }
    }

    /// Returns true if the delayed-ACK timer has fired and a bare ACK must be
    /// sent even though there is no outbound data to piggyback it on.
    ///
    /// This is the external polling API.  Call it from the network driver's
    /// periodic timer tick (every ~1 ms).
    pub fn needs_delayed_ack(&mut self) -> bool {
        if self.delayed_ack_segs == 0 {
            return false;
        }
        self.delayed_ack_tick(0)
    }

    /// Initiate a close (application calls close())
    pub fn close(&mut self) {
        match self.state {
            TcpState::Established => {
                self.state = TcpState::FinWait1;
            }
            TcpState::CloseWait => {
                self.state = TcpState::LastAck;
            }
            _ => {}
        }
    }

    // ----- Options -----

    /// Set TCP_NODELAY (disable Nagle)
    pub fn set_nodelay(&mut self, enabled: bool) {
        self.nodelay = enabled;
        if enabled {
            self.nagle_enabled = false;
        } else {
            self.nagle_enabled = true;
        }
    }

    /// Set keep-alive
    pub fn set_keepalive(&mut self, enabled: bool) {
        self.keepalive_enabled = enabled;
        if enabled {
            self.last_recv_tick = crate::time::clock::uptime_ms();
            self.keepalive_probes_sent = 0;
        }
    }

    /// Set send buffer capacity
    pub fn set_snd_buf(&mut self, cap: usize) {
        self.snd_buf_cap = cap;
    }

    /// Set receive buffer capacity
    pub fn set_rcv_buf(&mut self, cap: usize) {
        self.rcv_buf_cap = cap;
        let avail = cap.saturating_sub(self.recv_buf.len());
        self.rcv_wnd = avail.min(65535) as u16;
    }

    /// Get connection statistics
    pub fn stats(&self) -> TcpStats {
        TcpStats {
            bytes_sent: self.total_bytes_sent,
            bytes_received: self.total_bytes_received,
            retransmissions: self.retx_count_total,
            // Tick-domain SRTT (legacy field, kept for back-compat)
            rtt_ms: if self.rtt_measured {
                self.srtt_x8 / 8
            } else {
                0
            },
            // ms-domain RFC 6298 fields
            rtt_srtt_ms: self.rtt_srtt / 8,
            rtt_rttvar_ms: self.rtt_rttvar / 4,
            rto_ms: self.rto_ms,
            cwnd: self.cwnd,
            ssthresh: self.ssthresh,
            state: self.state,
            ooo_segments: self.ooo_queue.len(),
            retx_queue_len: self.retx_queue.len(),
            sack_permitted: self.sack_permitted,
            wscale_ok: self.wscale_ok,
            snd_wscale: self.snd_wscale,
            rcv_wscale: self.rcv_wscale,
        }
    }

    // ----- Option building helpers (for outgoing segments) -----

    /// Build SYN options into `buf` using this connection's current state.
    /// Returns bytes written.
    pub fn write_syn_options(&self, buf: &mut [u8]) -> usize {
        build_syn_options(buf, self.mss, self.ts_val, self.snd_wscale)
    }

    /// Build data/ACK options (timestamp only) into `buf`.
    /// Returns bytes written.
    pub fn write_data_options(&self, buf: &mut [u8]) -> usize {
        if !self.ts_ok {
            return 0;
        }
        build_data_options(buf, self.ts_val, self.ts_recent)
    }

    /// Build SACK + timestamp options into `buf` for an out-of-order ACK.
    /// Falls back to plain timestamp if no SACK blocks are present.
    /// Returns bytes written.
    pub fn write_sack_options(&self, buf: &mut [u8]) -> usize {
        if !self.ts_ok && (!self.sack_permitted || self.rcv_sack_count == 0) {
            return 0;
        }
        let n = self.rcv_sack_count as usize;
        build_sack_options(buf, &self.rcv_sack_blocks[..n], self.ts_val, self.ts_recent)
    }
}

/// TCP connection statistics snapshot
pub struct TcpStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub retransmissions: u64,
    /// Smoothed RTT in ms (tick-domain, legacy).
    pub rtt_ms: u64,
    /// Smoothed RTT in ms (RFC 6298 ms-domain, SRTT×8 / 8).
    pub rtt_srtt_ms: u32,
    /// RTT variance in ms (RFC 6298 ms-domain, RTTVAR×4 / 4).
    pub rtt_rttvar_ms: u32,
    /// Current retransmit timeout in ms (clamped [200, 120000]).
    pub rto_ms: u32,
    pub cwnd: u32,
    pub ssthresh: u32,
    pub state: TcpState,
    pub ooo_segments: usize,
    pub retx_queue_len: usize,
    /// Whether SACK was negotiated on this connection.
    pub sack_permitted: bool,
    /// Whether window scaling was negotiated.
    pub wscale_ok: bool,
    /// Our advertised window scale shift.
    pub snd_wscale: u8,
    /// Peer's window scale shift.
    pub rcv_wscale: u8,
}

// ---------------------------------------------------------------------------
// Sequence number comparison helpers (handles wrapping)
// ---------------------------------------------------------------------------

/// a < b (handling 32-bit wrap)
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

/// a <= b
fn seq_leq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}

/// a > b
fn seq_gt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// a >= b
fn seq_geq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

// ---------------------------------------------------------------------------
// Global TCP connection table
// ---------------------------------------------------------------------------

/// Global TCP connection table
pub static TCP_CONNECTIONS: Mutex<BTreeMap<u32, TcpConnection>> = Mutex::new(BTreeMap::new());
static NEXT_CONN_ID: Mutex<u32> = Mutex::new(1);

/// Generate an initial sequence number (simplified — should be more random)
fn generate_isn() -> u32 {
    let low: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") low, out("edx") _);
    }
    low
}

/// Create a new TCP connection (for connect() or listen())
pub fn create_connection(local_port: u16) -> u32 {
    let mut id = NEXT_CONN_ID.lock();
    let conn_id = *id;
    *id = id.saturating_add(1);

    let conn = TcpConnection::new(local_port);
    TCP_CONNECTIONS.lock().insert(conn_id, conn);
    conn_id
}

/// Start listening on a port
pub fn listen(port: u16) -> u32 {
    let conn_id = create_connection(port);
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.state = TcpState::Listen;
    }
    conn_id
}

/// Initiate a TCP connection (active open)
pub fn connect(local_port: u16, remote_ip: super::Ipv4Addr, remote_port: u16) -> u32 {
    let conn_id = create_connection(local_port);
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.remote_ip = remote_ip;
        conn.remote_port = remote_port;
        conn.snd_nxt = generate_isn();
        conn.snd_una = conn.snd_nxt;
        conn.state = TcpState::SynSent;
        // Enqueue SYN for retransmission
        conn.enqueue_retx(conn.snd_nxt, &[], flags::SYN);
        conn.snd_nxt = conn.snd_nxt.wrapping_add(1); // SYN consumes 1 seq
    }
    conn_id
}

/// Get connection state
pub fn get_state(conn_id: u32) -> Option<TcpState> {
    TCP_CONNECTIONS.lock().get(&conn_id).map(|c| c.state)
}

/// Read received data from a connection
pub fn read_data(conn_id: u32) -> Vec<u8> {
    let mut conns = TCP_CONNECTIONS.lock();
    if let Some(conn) = conns.get_mut(&conn_id) {
        let data = conn.recv_buf.clone();
        conn.recv_buf.clear();
        // Update receive window
        conn.rcv_wnd = conn.rcv_buf_cap.min(65535) as u16;
        data
    } else {
        Vec::new()
    }
}

/// Queue data for sending on a connection. Returns bytes accepted.
pub fn send_data(conn_id: u32, data: &[u8]) -> Result<usize, NetError> {
    let mut conns = TCP_CONNECTIONS.lock();
    let conn = conns.get_mut(&conn_id).ok_or(NetError::ConnectionRefused)?;
    if conn.state != TcpState::Established && conn.state != TcpState::CloseWait {
        return Err(NetError::ConnectionReset);
    }
    Ok(conn.queue_send(data))
}

/// Get number of active connections
pub fn connection_count() -> usize {
    TCP_CONNECTIONS.lock().len()
}

/// Close a connection (initiate graceful shutdown)
pub fn close_connection(conn_id: u32) {
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.close();
    }
}

/// Get statistics for a connection
pub fn get_stats(conn_id: u32) -> Option<TcpStats> {
    TCP_CONNECTIONS.lock().get(&conn_id).map(|c| c.stats())
}

/// Set TCP_NODELAY on a connection
pub fn set_nodelay(conn_id: u32, enabled: bool) {
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.set_nodelay(enabled);
    }
}

/// Set SO_KEEPALIVE on a connection
pub fn set_keepalive(conn_id: u32, enabled: bool) {
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.set_keepalive(enabled);
    }
}

/// Set SO_SNDBUF on a connection
pub fn set_sndbuf(conn_id: u32, size: usize) {
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.set_snd_buf(size);
    }
}

/// Set SO_RCVBUF on a connection
pub fn set_rcvbuf(conn_id: u32, size: usize) {
    if let Some(conn) = TCP_CONNECTIONS.lock().get_mut(&conn_id) {
        conn.set_rcv_buf(size);
    }
}

// ---------------------------------------------------------------------------
// Periodic timer — called from the network tick handler
// ---------------------------------------------------------------------------

/// Periodic TCP timer. Should be called every ~100ms or so.
///
/// Responsibilities:
///   - TIME_WAIT expiry: connections that have waited 2*MSL are removed.
///   - Dead-connection detection: connections where `check_retransmissions` has
///     bumped the retry counter past MAX_RETRIES are closed and removed.
///
/// NOTE: The actual retransmission check (and packet sending) is performed by
/// `net::tcp_retransmit_check()` in mod.rs, which calls `conn.check_retransmissions()`
/// and then sends the returned segments.  Do NOT call `check_retransmissions()` here
/// again — that would double-increment the retry counter and double-apply backoff.
pub fn tcp_timer_tick() {
    let mut conns = TCP_CONNECTIONS.lock();
    let mut to_remove = Vec::new();

    for (&id, conn) in conns.iter_mut() {
        // Advance RFC 7323 timestamp counter on every timer tick.
        // tcp_timer_tick() is called every ~100 ms; to keep the timestamp
        // counter monotonically increasing at ~1 ms resolution we increment by
        // the expected tick interval.  In practice the kernel timer calls this
        // more frequently — callers should pass `elapsed_ms` to tick_ts() when
        // precise granularity matters.  Here we approximate with 100 ticks.
        conn.ts_val = conn.ts_val.wrapping_add(100);

        // TIME_WAIT cleanup: once the 2*MSL timer expires, remove the connection.
        if conn.timewait_expired() {
            to_remove.push(id);
            continue;
        }

        // Dead-connection detection: if any retx entry has exceeded MAX_RETRIES
        // (set by check_retransmissions in the retransmit loop), close it.
        if conn.has_max_retries_exceeded() {
            crate::serial_println!("  TCP: conn {} max retries exceeded — aborting", id);
            conn.state = TcpState::Closed;
            conn.retx_queue.clear();
            to_remove.push(id);
        }
    }

    for id in to_remove {
        conns.remove(&id);
    }
}

/// Flush all closed and TIME_WAIT-expired connections.
pub fn cleanup_connections() {
    let mut conns = TCP_CONNECTIONS.lock();
    conns.retain(|_, conn| conn.state != TcpState::Closed && !conn.timewait_expired());
}

/// Get a summary of all connections for diagnostic display.
pub fn connection_summary() -> Vec<(u32, TcpState, u16, Ipv4Addr, u16)> {
    let conns = TCP_CONNECTIONS.lock();
    conns
        .iter()
        .map(|(&id, conn)| {
            (
                id,
                conn.state,
                conn.local_port,
                conn.remote_ip,
                conn.remote_port,
            )
        })
        .collect()
}
