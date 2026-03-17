use crate::sync::Mutex;
/// HTTP/2 frame format, HPACK header compression, and stream multiplexing
///
/// Provides HTTP/2 framing (DATA, HEADERS, SETTINGS, etc.), HPACK
/// static/dynamic table compression, stream lifecycle, and flow control.
///
/// Inspired by: RFC 7540 (HTTP/2), RFC 7541 (HPACK). All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// HTTP/2 connection preface
pub const CONNECTION_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Frame header size (9 bytes)
const FRAME_HDR_SIZE: usize = 9;

/// Default initial window size
const DEFAULT_WINDOW_SIZE: u32 = 65535;

/// Default max frame size
const DEFAULT_MAX_FRAME_SIZE: u32 = 16384;

/// Maximum frame size allowed
const MAX_FRAME_SIZE: u32 = 16777215;

/// Maximum concurrent streams
const MAX_CONCURRENT_STREAMS: u32 = 256;

/// HPACK static table size
const HPACK_STATIC_TABLE_SIZE: usize = 61;

// ---------------------------------------------------------------------------
// Frame types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Data = 0x0,
    Headers = 0x1,
    Priority = 0x2,
    RstStream = 0x3,
    Settings = 0x4,
    PushPromise = 0x5,
    Ping = 0x6,
    GoAway = 0x7,
    WindowUpdate = 0x8,
    Continuation = 0x9,
}

impl FrameType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0x0 => Some(FrameType::Data),
            0x1 => Some(FrameType::Headers),
            0x2 => Some(FrameType::Priority),
            0x3 => Some(FrameType::RstStream),
            0x4 => Some(FrameType::Settings),
            0x5 => Some(FrameType::PushPromise),
            0x6 => Some(FrameType::Ping),
            0x7 => Some(FrameType::GoAway),
            0x8 => Some(FrameType::WindowUpdate),
            0x9 => Some(FrameType::Continuation),
            _ => None,
        }
    }
}

// Frame flags
pub const FLAG_END_STREAM: u8 = 0x01;
pub const FLAG_END_HEADERS: u8 = 0x04;
pub const FLAG_PADDED: u8 = 0x08;
pub const FLAG_PRIORITY: u8 = 0x20;
pub const FLAG_ACK: u8 = 0x01; // for SETTINGS and PING

// Settings identifiers
pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

// Error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NoError = 0x0,
    ProtocolError = 0x1,
    InternalError = 0x2,
    FlowControlError = 0x3,
    SettingsTimeout = 0x4,
    StreamClosed = 0x5,
    FrameSizeError = 0x6,
    RefusedStream = 0x7,
    Cancel = 0x8,
    CompressionError = 0x9,
    ConnectError = 0xA,
    EnhanceYourCalm = 0xB,
    InadequateSecurity = 0xC,
    Http11Required = 0xD,
}

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// HTTP/2 frame
#[derive(Debug, Clone)]
pub struct Http2Frame {
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
    pub payload: Vec<u8>,
}

impl Http2Frame {
    /// Encode frame to bytes
    pub fn encode(&self) -> Vec<u8> {
        let len = self.payload.len();
        let mut buf = Vec::with_capacity(FRAME_HDR_SIZE + len);
        // Length (24-bit)
        buf.push(((len >> 16) & 0xFF) as u8);
        buf.push(((len >> 8) & 0xFF) as u8);
        buf.push((len & 0xFF) as u8);
        // Type
        buf.push(self.frame_type as u8);
        // Flags
        buf.push(self.flags);
        // Stream ID (31-bit, MSB reserved)
        let sid = self.stream_id & 0x7FFFFFFF;
        buf.extend_from_slice(&sid.to_be_bytes());
        // Payload
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode frame from bytes
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < FRAME_HDR_SIZE {
            return None;
        }
        let length = ((data[0] as u32) << 16) | ((data[1] as u32) << 8) | (data[2] as u32);
        let frame_type = FrameType::from_u8(data[3])?;
        let flags = data[4];
        let stream_id = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) & 0x7FFFFFFF;
        let total = FRAME_HDR_SIZE + length as usize;
        if data.len() < total {
            return None;
        }
        let payload = data[FRAME_HDR_SIZE..total].to_vec();
        Some((
            Http2Frame {
                frame_type,
                flags,
                stream_id,
                payload,
            },
            total,
        ))
    }

    /// Build a SETTINGS frame
    pub fn settings(pairs: &[(u16, u32)]) -> Self {
        let mut payload = Vec::new();
        for &(id, val) in pairs {
            payload.extend_from_slice(&id.to_be_bytes());
            payload.extend_from_slice(&val.to_be_bytes());
        }
        Http2Frame {
            frame_type: FrameType::Settings,
            flags: 0,
            stream_id: 0,
            payload,
        }
    }

    /// Build a SETTINGS ACK
    pub fn settings_ack() -> Self {
        Http2Frame {
            frame_type: FrameType::Settings,
            flags: FLAG_ACK,
            stream_id: 0,
            payload: Vec::new(),
        }
    }

    /// Build a WINDOW_UPDATE frame
    pub fn window_update(stream_id: u32, increment: u32) -> Self {
        Http2Frame {
            frame_type: FrameType::WindowUpdate,
            flags: 0,
            stream_id,
            payload: (increment & 0x7FFFFFFF).to_be_bytes().to_vec(),
        }
    }

    /// Build a PING frame
    pub fn ping(data: [u8; 8]) -> Self {
        Http2Frame {
            frame_type: FrameType::Ping,
            flags: 0,
            stream_id: 0,
            payload: data.to_vec(),
        }
    }

    /// Build a GOAWAY frame
    pub fn goaway(last_stream_id: u32, error_code: ErrorCode) -> Self {
        let mut payload = Vec::new();
        payload.extend_from_slice(&last_stream_id.to_be_bytes());
        payload.extend_from_slice(&(error_code as u32).to_be_bytes());
        Http2Frame {
            frame_type: FrameType::GoAway,
            flags: 0,
            stream_id: 0,
            payload,
        }
    }

    /// Build a RST_STREAM frame
    pub fn rst_stream(stream_id: u32, error_code: ErrorCode) -> Self {
        Http2Frame {
            frame_type: FrameType::RstStream,
            flags: 0,
            stream_id,
            payload: (error_code as u32).to_be_bytes().to_vec(),
        }
    }
}

// ---------------------------------------------------------------------------
// Stream state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Open,
    ReservedLocal,
    ReservedRemote,
    HalfClosedLocal,
    HalfClosedRemote,
    Closed,
}

/// An HTTP/2 stream
#[derive(Debug, Clone)]
pub struct Stream {
    pub id: u32,
    pub state: StreamState,
    pub send_window: i32,
    pub recv_window: i32,
    pub recv_data: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// HPACK (simplified static table only)
// ---------------------------------------------------------------------------

/// HPACK static table entries (index 1-61)
fn hpack_static_entry(index: usize) -> Option<(&'static str, &'static str)> {
    match index {
        1 => Some((":authority", "")),
        2 => Some((":method", "GET")),
        3 => Some((":method", "POST")),
        4 => Some((":path", "/")),
        5 => Some((":path", "/index.html")),
        6 => Some((":scheme", "http")),
        7 => Some((":scheme", "https")),
        8 => Some((":status", "200")),
        9 => Some((":status", "204")),
        10 => Some((":status", "206")),
        11 => Some((":status", "304")),
        12 => Some((":status", "400")),
        13 => Some((":status", "404")),
        14 => Some((":status", "500")),
        15 => Some(("accept-charset", "")),
        16 => Some(("accept-encoding", "gzip, deflate")),
        17 => Some(("accept-language", "")),
        28 => Some(("content-length", "")),
        31 => Some(("content-type", "")),
        32 => Some(("date", "")),
        58 => Some(("user-agent", "")),
        _ => None,
    }
}

/// Encode headers using HPACK (simplified: literal without indexing)
pub fn hpack_encode(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    for &(name, value) in headers {
        // Literal Header Field without Indexing (0000 pattern)
        buf.push(0x00);
        // Name length + name
        let name_bytes = name.as_bytes();
        encode_int(&mut buf, name_bytes.len() as u32, 7);
        buf.extend_from_slice(name_bytes);
        // Value length + value
        let value_bytes = value.as_bytes();
        encode_int(&mut buf, value_bytes.len() as u32, 7);
        buf.extend_from_slice(value_bytes);
    }
    buf
}

/// Encode an integer with HPACK prefix encoding
fn encode_int(buf: &mut Vec<u8>, value: u32, prefix_bits: u8) {
    let max_prefix = (1u32 << prefix_bits) - 1;
    if value < max_prefix {
        if let Some(last) = buf.last_mut() {
            *last |= value as u8;
        } else {
            buf.push(value as u8);
        }
    } else {
        if let Some(last) = buf.last_mut() {
            *last |= max_prefix as u8;
        } else {
            buf.push(max_prefix as u8);
        }
        let mut remaining = value - max_prefix;
        while remaining >= 128 {
            buf.push((remaining & 0x7F) as u8 | 0x80);
            remaining >>= 7;
        }
        buf.push(remaining as u8);
    }
}

// ---------------------------------------------------------------------------
// HTTP/2 connection
// ---------------------------------------------------------------------------

/// HTTP/2 connection
pub struct Http2Connection {
    /// Next stream ID to assign (odd for client, even for server)
    next_stream_id: u32,
    /// Active streams
    pub streams: Vec<Stream>,
    /// Connection-level send window
    pub conn_send_window: i32,
    /// Connection-level recv window
    pub conn_recv_window: i32,
    /// Peer settings
    pub peer_max_frame_size: u32,
    pub peer_max_concurrent: u32,
    pub peer_initial_window: u32,
    /// Whether this is the client side
    is_client: bool,
    /// Whether SETTINGS have been exchanged
    pub settings_acked: bool,
}

impl Http2Connection {
    pub fn new(is_client: bool) -> Self {
        Http2Connection {
            next_stream_id: if is_client { 1 } else { 2 },
            streams: Vec::new(),
            conn_send_window: DEFAULT_WINDOW_SIZE as i32,
            conn_recv_window: DEFAULT_WINDOW_SIZE as i32,
            peer_max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            peer_max_concurrent: MAX_CONCURRENT_STREAMS,
            peer_initial_window: DEFAULT_WINDOW_SIZE,
            is_client,
            settings_acked: false,
        }
    }

    /// Open a new stream, returns stream ID
    pub fn open_stream(&mut self) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.saturating_add(2);
        self.streams.push(Stream {
            id,
            state: StreamState::Open,
            send_window: self.peer_initial_window as i32,
            recv_window: DEFAULT_WINDOW_SIZE as i32,
            recv_data: Vec::new(),
            headers: Vec::new(),
        });
        id
    }

    /// Build a HEADERS frame for a stream
    pub fn send_headers(
        &mut self,
        stream_id: u32,
        headers: &[(&str, &str)],
        end_stream: bool,
    ) -> Vec<u8> {
        let encoded = hpack_encode(headers);
        let mut flags = FLAG_END_HEADERS;
        if end_stream {
            flags |= FLAG_END_STREAM;
        }
        let frame = Http2Frame {
            frame_type: FrameType::Headers,
            flags,
            stream_id,
            payload: encoded,
        };
        frame.encode()
    }

    /// Build a DATA frame for a stream
    pub fn send_data(&mut self, stream_id: u32, data: &[u8], end_stream: bool) -> Vec<u8> {
        let flags = if end_stream { FLAG_END_STREAM } else { 0 };
        let frame = Http2Frame {
            frame_type: FrameType::Data,
            flags,
            stream_id,
            payload: data.to_vec(),
        };
        self.conn_send_window -= data.len() as i32;
        if let Some(stream) = self.streams.iter_mut().find(|s| s.id == stream_id) {
            stream.send_window -= data.len() as i32;
        }
        frame.encode()
    }

    /// Process a received frame
    pub fn process_frame(&mut self, frame: &Http2Frame) {
        match frame.frame_type {
            FrameType::Settings => {
                if frame.flags & FLAG_ACK != 0 {
                    self.settings_acked = true;
                } else {
                    // Parse settings
                    let mut i = 0;
                    while i + 5 < frame.payload.len() {
                        let id = u16::from_be_bytes([frame.payload[i], frame.payload[i + 1]]);
                        let val = u32::from_be_bytes([
                            frame.payload[i + 2],
                            frame.payload[i + 3],
                            frame.payload[i + 4],
                            frame.payload[i + 5],
                        ]);
                        match id {
                            SETTINGS_MAX_FRAME_SIZE => self.peer_max_frame_size = val,
                            SETTINGS_MAX_CONCURRENT_STREAMS => self.peer_max_concurrent = val,
                            SETTINGS_INITIAL_WINDOW_SIZE => self.peer_initial_window = val,
                            _ => {}
                        }
                        i = i.saturating_add(6);
                    }
                }
            }
            FrameType::WindowUpdate => {
                if frame.payload.len() >= 4 {
                    let increment = u32::from_be_bytes([
                        frame.payload[0],
                        frame.payload[1],
                        frame.payload[2],
                        frame.payload[3],
                    ]) & 0x7FFFFFFF;
                    if frame.stream_id == 0 {
                        self.conn_send_window += increment as i32;
                    } else if let Some(s) =
                        self.streams.iter_mut().find(|s| s.id == frame.stream_id)
                    {
                        s.send_window += increment as i32;
                    }
                }
            }
            FrameType::Data => {
                if let Some(s) = self.streams.iter_mut().find(|s| s.id == frame.stream_id) {
                    s.recv_data.extend_from_slice(&frame.payload);
                    if frame.flags & FLAG_END_STREAM != 0 {
                        s.state = StreamState::HalfClosedRemote;
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static H2: Mutex<Option<Vec<Http2Connection>>> = Mutex::new(None);

pub fn init() {
    *H2.lock() = Some(Vec::new());
    serial_println!("  Net: HTTP/2 subsystem initialized");
}
