use crate::sync::Mutex;
/// HTTP/3 over QUIC
///
/// Provides HTTP/3 framing on top of QUIC transport. Implements
/// QPACK header compression, unidirectional control streams,
/// and request/response stream mapping.
///
/// Inspired by: RFC 9114 (HTTP/3), RFC 9204 (QPACK). All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// HTTP/3 frame types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H3FrameType {
    Data = 0x00,
    Headers = 0x01,
    CancelPush = 0x03,
    Settings = 0x04,
    PushPromise = 0x05,
    GoAway = 0x07,
    MaxPushId = 0x0D,
}

impl H3FrameType {
    pub fn from_u64(val: u64) -> Option<Self> {
        match val {
            0x00 => Some(H3FrameType::Data),
            0x01 => Some(H3FrameType::Headers),
            0x03 => Some(H3FrameType::CancelPush),
            0x04 => Some(H3FrameType::Settings),
            0x05 => Some(H3FrameType::PushPromise),
            0x07 => Some(H3FrameType::GoAway),
            0x0D => Some(H3FrameType::MaxPushId),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP/3 settings
// ---------------------------------------------------------------------------

pub const H3_SETTINGS_MAX_FIELD_SECTION_SIZE: u64 = 0x06;
pub const H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
pub const H3_SETTINGS_QPACK_BLOCKED_STREAMS: u64 = 0x07;

// Unidirectional stream types
pub const H3_STREAM_CONTROL: u64 = 0x00;
pub const H3_STREAM_PUSH: u64 = 0x01;
pub const H3_STREAM_QPACK_ENCODER: u64 = 0x02;
pub const H3_STREAM_QPACK_DECODER: u64 = 0x03;

// Error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H3Error {
    NoError = 0x100,
    GeneralProtocolError = 0x101,
    InternalError = 0x102,
    StreamCreationError = 0x103,
    ClosedCriticalStream = 0x104,
    FrameUnexpected = 0x105,
    FrameError = 0x106,
    ExcessiveLoad = 0x107,
    IdError = 0x108,
    SettingsError = 0x109,
    MissingSettings = 0x10A,
    RequestRejected = 0x10B,
    RequestCancelled = 0x10C,
    RequestIncomplete = 0x10D,
    MessageError = 0x10E,
    ConnectError = 0x10F,
    VersionFallback = 0x110,
    QpackDecompressionFailed = 0x200,
    QpackEncoderStreamError = 0x201,
    QpackDecoderStreamError = 0x202,
}

// ---------------------------------------------------------------------------
// QUIC variable-length integer encoding
// ---------------------------------------------------------------------------

/// Encode a variable-length integer (QUIC encoding)
pub fn encode_varint(val: u64) -> Vec<u8> {
    if val < 64 {
        alloc::vec![val as u8]
    } else if val < 16384 {
        let v = val as u16 | 0x4000;
        v.to_be_bytes().to_vec()
    } else if val < 1073741824 {
        let v = val as u32 | 0x80000000;
        v.to_be_bytes().to_vec()
    } else {
        let v = val | 0xC000000000000000;
        v.to_be_bytes().to_vec()
    }
}

/// Decode a variable-length integer
pub fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    let first = data[0];
    let length = 1 << (first >> 6);
    if data.len() < length {
        return None;
    }
    let val = match length {
        1 => (first & 0x3F) as u64,
        2 => {
            let v = u16::from_be_bytes([data[0], data[1]]);
            (v & 0x3FFF) as u64
        }
        4 => {
            let v = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
            (v & 0x3FFFFFFF) as u64
        }
        8 => {
            let v = u64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]);
            v & 0x3FFFFFFFFFFFFFFF
        }
        _ => return None,
    };
    Some((val, length))
}

// ---------------------------------------------------------------------------
// HTTP/3 frame
// ---------------------------------------------------------------------------

/// An HTTP/3 frame
#[derive(Debug, Clone)]
pub struct H3Frame {
    pub frame_type: u64,
    pub payload: Vec<u8>,
}

impl H3Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = encode_varint(self.frame_type);
        buf.extend_from_slice(&encode_varint(self.payload.len() as u64));
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        let (frame_type, t_len) = decode_varint(data)?;
        let (payload_len, p_len) = decode_varint(&data[t_len..])?;
        let header_len = t_len + p_len;
        let total = header_len + payload_len as usize;
        if data.len() < total {
            return None;
        }
        let payload = data[header_len..total].to_vec();
        Some((
            H3Frame {
                frame_type,
                payload,
            },
            total,
        ))
    }

    /// Build a SETTINGS frame
    pub fn settings(params: &[(u64, u64)]) -> Self {
        let mut payload = Vec::new();
        for &(id, val) in params {
            payload.extend_from_slice(&encode_varint(id));
            payload.extend_from_slice(&encode_varint(val));
        }
        H3Frame {
            frame_type: H3FrameType::Settings as u64,
            payload,
        }
    }

    /// Build a GOAWAY frame
    pub fn goaway(stream_id: u64) -> Self {
        H3Frame {
            frame_type: H3FrameType::GoAway as u64,
            payload: encode_varint(stream_id),
        }
    }
}

// ---------------------------------------------------------------------------
// QPACK (simplified — static table only, no dynamic table)
// ---------------------------------------------------------------------------

/// QPACK static table lookup
fn qpack_static_entry(index: usize) -> Option<(&'static str, &'static str)> {
    match index {
        0 => Some((":authority", "")),
        1 => Some((":path", "/")),
        2 => Some(("age", "0")),
        3 => Some(("content-disposition", "")),
        4 => Some(("content-length", "0")),
        5 => Some(("cookie", "")),
        6 => Some(("date", "")),
        15 => Some((":method", "CONNECT")),
        16 => Some((":method", "DELETE")),
        17 => Some((":method", "GET")),
        18 => Some((":method", "HEAD")),
        19 => Some((":method", "OPTIONS")),
        20 => Some((":method", "POST")),
        21 => Some((":method", "PUT")),
        22 => Some((":scheme", "http")),
        23 => Some((":scheme", "https")),
        24 => Some((":status", "103")),
        25 => Some((":status", "200")),
        26 => Some((":status", "304")),
        27 => Some((":status", "404")),
        28 => Some((":status", "503")),
        63 => Some(("content-type", "application/json")),
        _ => None,
    }
}

/// Encode headers using QPACK (simplified: literal only)
pub fn qpack_encode(headers: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    // Required insert count = 0, delta base = 0
    buf.push(0x00);
    buf.push(0x00);
    for &(name, value) in headers {
        // Literal with name reference (0x5 prefix): 01NXXXXX
        // We use literal without name reference: 001NHXXX
        buf.push(0x20); // literal, no huffman, no name ref
                        // Name length
        let nb = name.as_bytes();
        buf.push(nb.len() as u8);
        buf.extend_from_slice(nb);
        // Value length
        let vb = value.as_bytes();
        buf.push(vb.len() as u8);
        buf.extend_from_slice(vb);
    }
    buf
}

// ---------------------------------------------------------------------------
// HTTP/3 connection
// ---------------------------------------------------------------------------

/// HTTP/3 connection state
pub struct Http3Connection {
    /// QUIC connection handle (opaque ID)
    pub quic_conn_id: u32,
    /// Next request stream ID
    next_stream_id: u64,
    /// Peer settings
    pub peer_max_field_section_size: u64,
    /// Whether we sent our SETTINGS
    pub settings_sent: bool,
    /// Whether we received peer SETTINGS
    pub settings_received: bool,
    /// Request/response data per stream
    pub streams: Vec<H3Stream>,
}

/// Per-stream state
#[derive(Debug, Clone)]
pub struct H3Stream {
    pub stream_id: u64,
    pub headers: Vec<(String, String)>,
    pub data: Vec<u8>,
    pub done: bool,
}

impl Http3Connection {
    pub fn new(quic_conn_id: u32) -> Self {
        Http3Connection {
            quic_conn_id,
            next_stream_id: 0,
            peer_max_field_section_size: 65536,
            settings_sent: false,
            settings_received: false,
            streams: Vec::new(),
        }
    }

    /// Build initial SETTINGS to send on control stream
    pub fn build_settings(&mut self) -> Vec<u8> {
        let frame = H3Frame::settings(&[
            (H3_SETTINGS_MAX_FIELD_SECTION_SIZE, 65536),
            (H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY, 0),
            (H3_SETTINGS_QPACK_BLOCKED_STREAMS, 0),
        ]);
        self.settings_sent = true;
        // Prepend stream type for unidirectional control stream
        let mut buf = encode_varint(H3_STREAM_CONTROL);
        buf.extend_from_slice(&frame.encode());
        buf
    }

    /// Create a request
    pub fn request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (u64, Vec<u8>) {
        let stream_id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.saturating_add(4); // client-initiated bidi streams: 0, 4, 8...

        let mut all_headers = Vec::new();
        all_headers.push((":method", method));
        all_headers.push((":path", path));
        for &h in headers {
            all_headers.push(h);
        }
        let encoded = qpack_encode(&all_headers);
        let headers_frame = H3Frame {
            frame_type: H3FrameType::Headers as u64,
            payload: encoded,
        };

        self.streams.push(H3Stream {
            stream_id,
            headers: Vec::new(),
            data: Vec::new(),
            done: false,
        });

        (stream_id, headers_frame.encode())
    }

    /// Process received frames on a stream
    pub fn process_stream_data(&mut self, stream_id: u64, data: &[u8]) {
        let mut offset = 0;
        while offset < data.len() {
            match H3Frame::decode(&data[offset..]) {
                Some((frame, consumed)) => {
                    offset += consumed;
                    if let Some(stream) = self.streams.iter_mut().find(|s| s.stream_id == stream_id)
                    {
                        match H3FrameType::from_u64(frame.frame_type) {
                            Some(H3FrameType::Data) => {
                                stream.data.extend_from_slice(&frame.payload);
                            }
                            Some(H3FrameType::Headers) => {
                                // Simplified: just store raw
                            }
                            _ => {}
                        }
                    }
                }
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static H3: Mutex<Option<Vec<Http3Connection>>> = Mutex::new(None);

pub fn init() {
    *H3.lock() = Some(Vec::new());
    serial_println!("  Net: HTTP/3 subsystem initialized");
}
