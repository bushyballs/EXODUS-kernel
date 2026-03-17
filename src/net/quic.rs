use crate::sync::Mutex;
/// QUIC protocol — UDP-based reliable transport (RFC 9000)
///
/// Provides QUIC connection management: connection IDs, packet formats,
/// stream multiplexing, flow control, and loss recovery. Encryption
/// is stubbed (would require TLS 1.3 integration).
///
/// Inspired by: RFC 9000 (QUIC Transport), RFC 9001 (QUIC TLS).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_CONNECTIONS: usize = 256;
const MAX_STREAMS_PER_CONN: usize = 128;
const DEFAULT_MAX_DATA: u64 = 1048576; // 1MB
const DEFAULT_MAX_STREAM_DATA: u64 = 262144; // 256KB
const INITIAL_CWND: u64 = 14720; // 10 * 1472

// QUIC packet types (long header)
const PKT_INITIAL: u8 = 0x00;
const PKT_ZERO_RTT: u8 = 0x01;
const PKT_HANDSHAKE: u8 = 0x02;
const PKT_RETRY: u8 = 0x03;

// Frame types
const FRAME_PADDING: u64 = 0x00;
const FRAME_PING: u64 = 0x01;
const FRAME_ACK: u64 = 0x02;
const FRAME_RESET_STREAM: u64 = 0x04;
const FRAME_STOP_SENDING: u64 = 0x05;
const FRAME_CRYPTO: u64 = 0x06;
const FRAME_NEW_TOKEN: u64 = 0x07;
const FRAME_STREAM: u64 = 0x08; // 0x08-0x0F
const FRAME_MAX_DATA: u64 = 0x10;
const FRAME_MAX_STREAM_DATA: u64 = 0x11;
const FRAME_MAX_STREAMS: u64 = 0x12;
const FRAME_CONNECTION_CLOSE: u64 = 0x1C;

// Stream ID types
// Client-initiated bidirectional: 0, 4, 8, ...
// Server-initiated bidirectional: 1, 5, 9, ...
// Client-initiated unidirectional: 2, 6, 10, ...
// Server-initiated unidirectional: 3, 7, 11, ...

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Initial,
    Handshake,
    Connected,
    Closing,
    Draining,
    Closed,
}

/// Stream state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Open,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
    Reset,
}

// ---------------------------------------------------------------------------
// Connection ID
// ---------------------------------------------------------------------------

/// QUIC connection ID (variable length, up to 20 bytes)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionId {
    pub bytes: Vec<u8>,
}

impl ConnectionId {
    pub fn new(bytes: &[u8]) -> Self {
        ConnectionId {
            bytes: bytes.to_vec(),
        }
    }

    pub fn empty() -> Self {
        ConnectionId { bytes: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// A QUIC stream
#[derive(Debug, Clone)]
pub struct QuicStream {
    pub id: u64,
    pub state: StreamState,
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,
    pub send_offset: u64,
    pub recv_offset: u64,
    pub max_send_data: u64,
    pub max_recv_data: u64,
    pub fin_sent: bool,
    pub fin_received: bool,
}

impl QuicStream {
    fn new(id: u64) -> Self {
        QuicStream {
            id,
            state: StreamState::Open,
            send_buf: Vec::new(),
            recv_buf: Vec::new(),
            send_offset: 0,
            recv_offset: 0,
            max_send_data: DEFAULT_MAX_STREAM_DATA,
            max_recv_data: DEFAULT_MAX_STREAM_DATA,
            fin_sent: false,
            fin_received: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Congestion control (simplified NewReno)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CongestionCtrl {
    cwnd: u64,
    ssthresh: u64,
    bytes_in_flight: u64,
    rtt_estimate: u64, // in ticks
}

impl CongestionCtrl {
    fn new() -> Self {
        CongestionCtrl {
            cwnd: INITIAL_CWND,
            ssthresh: u64::MAX,
            bytes_in_flight: 0,
            rtt_estimate: 100,
        }
    }

    fn on_ack(&mut self, acked_bytes: u64) {
        self.bytes_in_flight = self.bytes_in_flight.saturating_sub(acked_bytes);
        if self.cwnd < self.ssthresh {
            // Slow start
            self.cwnd = self.cwnd.saturating_add(acked_bytes);
        } else {
            // Congestion avoidance
            let inc = if self.cwnd > 0 {
                (acked_bytes * 1472) / self.cwnd
            } else {
                0
            };
            self.cwnd = self.cwnd.saturating_add(inc);
        }
    }

    fn on_loss(&mut self) {
        self.ssthresh = (self.cwnd / 2).max(2 * 1472);
        self.cwnd = self.ssthresh;
    }

    fn can_send(&self, bytes: u64) -> bool {
        self.bytes_in_flight + bytes <= self.cwnd
    }
}

// ---------------------------------------------------------------------------
// QUIC connection
// ---------------------------------------------------------------------------

/// QUIC connection
pub struct QuicConnection {
    pub id: u32,
    pub state: ConnState,
    /// Source connection ID
    pub src_cid: ConnectionId,
    /// Destination connection ID
    pub dst_cid: ConnectionId,
    /// Streams
    pub streams: Vec<QuicStream>,
    /// Next stream IDs
    next_bidi_stream: u64,
    next_uni_stream: u64,
    /// Connection-level flow control
    pub max_data_local: u64,
    pub max_data_remote: u64,
    pub data_sent: u64,
    pub data_received: u64,
    /// Congestion control
    cc: CongestionCtrl,
    /// Packet number space
    next_pkt_num: u64,
    /// Is client
    is_client: bool,
    /// Statistics
    pub packets_sent: u64,
    pub packets_recv: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
}

impl QuicConnection {
    pub fn new(id: u32, src_cid: ConnectionId, dst_cid: ConnectionId, is_client: bool) -> Self {
        QuicConnection {
            id,
            state: ConnState::Initial,
            src_cid,
            dst_cid,
            streams: Vec::new(),
            next_bidi_stream: if is_client { 0 } else { 1 },
            next_uni_stream: if is_client { 2 } else { 3 },
            max_data_local: DEFAULT_MAX_DATA,
            max_data_remote: DEFAULT_MAX_DATA,
            data_sent: 0,
            data_received: 0,
            cc: CongestionCtrl::new(),
            next_pkt_num: 0,
            is_client,
            packets_sent: 0,
            packets_recv: 0,
            bytes_sent: 0,
            bytes_recv: 0,
        }
    }

    /// Open a new bidirectional stream
    pub fn open_bidi_stream(&mut self) -> Option<u64> {
        if self.streams.len() >= MAX_STREAMS_PER_CONN {
            return None;
        }
        let id = self.next_bidi_stream;
        self.next_bidi_stream = self.next_bidi_stream.saturating_add(4);
        self.streams.push(QuicStream::new(id));
        Some(id)
    }

    /// Open a new unidirectional stream
    pub fn open_uni_stream(&mut self) -> Option<u64> {
        if self.streams.len() >= MAX_STREAMS_PER_CONN {
            return None;
        }
        let id = self.next_uni_stream;
        self.next_uni_stream = self.next_uni_stream.saturating_add(4);
        self.streams.push(QuicStream::new(id));
        Some(id)
    }

    /// Send data on a stream
    pub fn send(&mut self, stream_id: u64, data: &[u8]) -> Result<usize, QuicError> {
        if self.state != ConnState::Connected {
            return Err(QuicError::NotConnected);
        }
        let stream = self
            .streams
            .iter_mut()
            .find(|s| s.id == stream_id)
            .ok_or(QuicError::StreamNotFound)?;
        if !matches!(
            stream.state,
            StreamState::Open | StreamState::HalfClosedRemote
        ) {
            return Err(QuicError::StreamClosed);
        }
        let allowed = stream.max_send_data.saturating_sub(stream.send_offset) as usize;
        let to_send = data.len().min(allowed);
        stream.send_buf.extend_from_slice(&data[..to_send]);
        stream.send_offset = stream.send_offset.saturating_add(to_send as u64);
        self.data_sent = self.data_sent.saturating_add(to_send as u64);
        self.bytes_sent = self.bytes_sent.saturating_add(to_send as u64);
        Ok(to_send)
    }

    /// Receive data from a stream
    pub fn recv(&mut self, stream_id: u64) -> Result<Vec<u8>, QuicError> {
        let stream = self
            .streams
            .iter_mut()
            .find(|s| s.id == stream_id)
            .ok_or(QuicError::StreamNotFound)?;
        let data = core::mem::take(&mut stream.recv_buf);
        Ok(data)
    }

    /// Close a stream
    pub fn close_stream(&mut self, stream_id: u64) {
        if let Some(stream) = self.streams.iter_mut().find(|s| s.id == stream_id) {
            stream.fin_sent = true;
            match stream.state {
                StreamState::Open => stream.state = StreamState::HalfClosedLocal,
                StreamState::HalfClosedRemote => stream.state = StreamState::Closed,
                _ => {}
            }
        }
    }

    /// Close the connection
    pub fn close(&mut self, error_code: u64, reason: &str) {
        self.state = ConnState::Closing;
        serial_println!(
            "  QUIC: connection {} closing (error={}, reason={})",
            self.id,
            error_code,
            reason
        );
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuicError {
    NotInitialized,
    NotConnected,
    StreamNotFound,
    StreamClosed,
    FlowControlLimit,
    ConnectionClosed,
    MaxConnectionsReached,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct QuicSubsystem {
    connections: Vec<QuicConnection>,
    next_id: u32,
}

static QUIC: Mutex<Option<QuicSubsystem>> = Mutex::new(None);

pub fn init() {
    *QUIC.lock() = Some(QuicSubsystem {
        connections: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: QUIC transport subsystem initialized");
}

/// Create a new QUIC connection
pub fn create_connection(
    src_cid: &[u8],
    dst_cid: &[u8],
    is_client: bool,
) -> Result<u32, QuicError> {
    let mut guard = QUIC.lock();
    let sys = guard.as_mut().ok_or(QuicError::NotInitialized)?;
    if sys.connections.len() >= MAX_CONNECTIONS {
        return Err(QuicError::MaxConnectionsReached);
    }
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.connections.push(QuicConnection::new(
        id,
        ConnectionId::new(src_cid),
        ConnectionId::new(dst_cid),
        is_client,
    ));
    Ok(id)
}
