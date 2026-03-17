use crate::sync::Mutex;
/// WebSocket protocol implementation (RFC 6455)
///
/// Full WebSocket framing: frame encoding/decoding, masking, fragmentation,
/// control frames (ping/pong/close), and connection lifecycle management.
///
/// Inspired by: RFC 6455, autobahn test suite. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum frame payload (16 MB)
const MAX_PAYLOAD_SIZE: usize = 16 * 1024 * 1024;

/// Maximum control frame payload (125 bytes per RFC 6455)
const MAX_CONTROL_PAYLOAD: usize = 125;

/// Maximum queued messages per connection
const MAX_QUEUED_MESSAGES: usize = 256;

/// Maximum concurrent connections
const MAX_CONNECTIONS: usize = 128;

// ---------------------------------------------------------------------------
// Opcodes (4-bit)
// ---------------------------------------------------------------------------

/// WebSocket frame opcodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl Opcode {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val & 0x0F {
            0x0 => Some(Opcode::Continuation),
            0x1 => Some(Opcode::Text),
            0x2 => Some(Opcode::Binary),
            0x8 => Some(Opcode::Close),
            0x9 => Some(Opcode::Ping),
            0xA => Some(Opcode::Pong),
            _ => None,
        }
    }

    pub fn is_control(&self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }
}

// ---------------------------------------------------------------------------
// Close status codes
// ---------------------------------------------------------------------------

/// WebSocket close status codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCode {
    Normal = 1000,
    GoingAway = 1001,
    ProtocolError = 1002,
    UnsupportedData = 1003,
    NoStatus = 1005,
    Abnormal = 1006,
    InvalidPayload = 1007,
    PolicyViolation = 1008,
    MessageTooBig = 1009,
    MandatoryExtension = 1010,
    InternalError = 1011,
}

impl CloseCode {
    pub fn from_u16(val: u16) -> Self {
        match val {
            1000 => CloseCode::Normal,
            1001 => CloseCode::GoingAway,
            1002 => CloseCode::ProtocolError,
            1003 => CloseCode::UnsupportedData,
            1005 => CloseCode::NoStatus,
            1006 => CloseCode::Abnormal,
            1007 => CloseCode::InvalidPayload,
            1008 => CloseCode::PolicyViolation,
            1009 => CloseCode::MessageTooBig,
            1010 => CloseCode::MandatoryExtension,
            1011 => CloseCode::InternalError,
            _ => CloseCode::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket frame
// ---------------------------------------------------------------------------

/// A parsed WebSocket frame
#[derive(Debug, Clone)]
pub struct WsFrame {
    pub fin: bool,
    pub rsv1: bool,
    pub rsv2: bool,
    pub rsv3: bool,
    pub opcode: Opcode,
    pub masked: bool,
    pub masking_key: [u8; 4],
    pub payload: Vec<u8>,
}

impl WsFrame {
    /// Create a new frame
    pub fn new(opcode: Opcode, payload: Vec<u8>, fin: bool) -> Self {
        WsFrame {
            fin,
            rsv1: false,
            rsv2: false,
            rsv3: false,
            opcode,
            masked: false,
            masking_key: [0; 4],
            payload,
        }
    }

    /// Create a text frame
    pub fn text(data: &str) -> Self {
        Self::new(Opcode::Text, Vec::from(data.as_bytes()), true)
    }

    /// Create a binary frame
    pub fn binary(data: &[u8]) -> Self {
        Self::new(Opcode::Binary, data.to_vec(), true)
    }

    /// Create a ping frame
    pub fn ping(data: &[u8]) -> Self {
        let payload = if data.len() > MAX_CONTROL_PAYLOAD {
            data[..MAX_CONTROL_PAYLOAD].to_vec()
        } else {
            data.to_vec()
        };
        Self::new(Opcode::Ping, payload, true)
    }

    /// Create a pong frame (echo ping payload)
    pub fn pong(data: &[u8]) -> Self {
        let payload = if data.len() > MAX_CONTROL_PAYLOAD {
            data[..MAX_CONTROL_PAYLOAD].to_vec()
        } else {
            data.to_vec()
        };
        Self::new(Opcode::Pong, payload, true)
    }

    /// Create a close frame
    pub fn close(code: CloseCode, reason: &str) -> Self {
        let mut payload = Vec::new();
        payload.extend_from_slice(&(code as u16).to_be_bytes());
        let reason_bytes = reason.as_bytes();
        let max_reason = MAX_CONTROL_PAYLOAD - 2;
        let reason_len = reason_bytes.len().min(max_reason);
        payload.extend_from_slice(&reason_bytes[..reason_len]);
        Self::new(Opcode::Close, payload, true)
    }

    /// Set masking key and apply masking to payload
    pub fn set_mask(&mut self, key: [u8; 4]) {
        self.masked = true;
        self.masking_key = key;
        apply_mask(&mut self.payload, &key);
    }

    /// Encode to wire format
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // First byte: FIN, RSV1-3, opcode
        let mut b0 = self.opcode as u8;
        if self.fin {
            b0 |= 0x80;
        }
        if self.rsv1 {
            b0 |= 0x40;
        }
        if self.rsv2 {
            b0 |= 0x20;
        }
        if self.rsv3 {
            b0 |= 0x10;
        }
        buf.push(b0);

        // Second byte: MASK, payload length
        let len = self.payload.len();
        let mask_bit = if self.masked { 0x80u8 } else { 0 };

        if len < 126 {
            buf.push(mask_bit | len as u8);
        } else if len <= 65535 {
            buf.push(mask_bit | 126);
            buf.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            buf.push(mask_bit | 127);
            buf.extend_from_slice(&(len as u64).to_be_bytes());
        }

        // Masking key
        if self.masked {
            buf.extend_from_slice(&self.masking_key);
        }

        // Payload (already masked if masked=true)
        buf.extend_from_slice(&self.payload);

        buf
    }

    /// Decode a frame from wire bytes
    ///
    /// Returns (frame, bytes_consumed) or None if not enough data
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 2 {
            return None;
        }

        let b0 = data[0];
        let b1 = data[1];

        let fin = b0 & 0x80 != 0;
        let rsv1 = b0 & 0x40 != 0;
        let rsv2 = b0 & 0x20 != 0;
        let rsv3 = b0 & 0x10 != 0;
        let opcode = Opcode::from_u8(b0)?;
        let masked = b1 & 0x80 != 0;
        let len_indicator = b1 & 0x7F;

        let mut offset = 2usize;
        let payload_len: usize;

        if len_indicator < 126 {
            payload_len = len_indicator as usize;
        } else if len_indicator == 126 {
            if data.len() < offset + 2 {
                return None;
            }
            payload_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
            offset = offset.saturating_add(2);
        } else {
            if data.len() < offset + 8 {
                return None;
            }
            payload_len = u64::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]) as usize;
            offset = offset.saturating_add(8);
        }

        if payload_len > MAX_PAYLOAD_SIZE {
            return None;
        }

        let mut masking_key = [0u8; 4];
        if masked {
            if data.len() < offset + 4 {
                return None;
            }
            masking_key.copy_from_slice(&data[offset..offset + 4]);
            offset = offset.saturating_add(4);
        }

        if data.len() < offset + payload_len {
            return None;
        }

        let mut payload = data[offset..offset + payload_len].to_vec();
        if masked {
            apply_mask(&mut payload, &masking_key);
        }

        let total = offset + payload_len;

        Some((
            WsFrame {
                fin,
                rsv1,
                rsv2,
                rsv3,
                opcode,
                masked,
                masking_key,
                payload,
            },
            total,
        ))
    }
}

/// Apply XOR masking to a payload
fn apply_mask(data: &mut [u8], key: &[u8; 4]) {
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i % 4];
    }
}

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// WebSocket connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Connecting,
    Open,
    Closing,
    Closed,
}

/// WebSocket connection
pub struct WebSocket {
    pub id: u32,
    pub state: WsState,
    pub url: String,
    /// Receive buffer (raw bytes from TCP)
    recv_raw: Vec<u8>,
    /// Decoded message queue
    recv_messages: Vec<WsMessage>,
    /// Fragmentation buffer (for multi-frame messages)
    frag_buf: Vec<u8>,
    frag_opcode: Option<Opcode>,
    /// Whether we are the client side (must mask frames)
    is_client: bool,
    /// Simple masking key counter
    mask_counter: u32,
    /// Statistics
    pub frames_sent: u64,
    pub frames_recv: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
}

/// A complete WebSocket message (may span multiple frames)
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub opcode: Opcode,
    pub data: Vec<u8>,
}

impl WebSocket {
    pub fn new(id: u32, url: &str, is_client: bool) -> Self {
        WebSocket {
            id,
            state: WsState::Connecting,
            url: String::from(url),
            recv_raw: Vec::new(),
            recv_messages: Vec::new(),
            frag_buf: Vec::new(),
            frag_opcode: None,
            is_client,
            mask_counter: 0x12345678,
            frames_sent: 0,
            frames_recv: 0,
            bytes_sent: 0,
            bytes_recv: 0,
        }
    }

    /// Feed raw bytes from TCP into the WebSocket decoder
    pub fn feed(&mut self, data: &[u8]) {
        self.recv_raw.extend_from_slice(data);
        self.process_incoming();
    }

    /// Process buffered raw bytes into frames/messages
    fn process_incoming(&mut self) {
        loop {
            match WsFrame::decode(&self.recv_raw) {
                Some((frame, consumed)) => {
                    self.recv_raw = self.recv_raw[consumed..].to_vec();
                    self.frames_recv = self.frames_recv.saturating_add(1);
                    self.bytes_recv = self.bytes_recv.saturating_add(frame.payload.len() as u64);
                    self.handle_frame(frame);
                }
                None => break,
            }
        }
    }

    /// Handle a decoded frame
    fn handle_frame(&mut self, frame: WsFrame) {
        match frame.opcode {
            Opcode::Ping => {
                // Auto-respond with pong
                let pong = WsFrame::pong(&frame.payload);
                // Queue for sending (caller must actually send)
                self.recv_messages.push(WsMessage {
                    opcode: Opcode::Pong,
                    data: pong.encode(),
                });
            }
            Opcode::Pong => {
                // Pong received — nothing to do
            }
            Opcode::Close => {
                self.state = WsState::Closing;
                // Parse close code
                if frame.payload.len() >= 2 {
                    let code = u16::from_be_bytes([frame.payload[0], frame.payload[1]]);
                    serial_println!("  WS: close code={}", code);
                }
            }
            Opcode::Continuation => {
                self.frag_buf.extend_from_slice(&frame.payload);
                if frame.fin {
                    if let Some(opcode) = self.frag_opcode.take() {
                        let data = core::mem::take(&mut self.frag_buf);
                        if self.recv_messages.len() < MAX_QUEUED_MESSAGES {
                            self.recv_messages.push(WsMessage { opcode, data });
                        }
                    }
                }
            }
            Opcode::Text | Opcode::Binary => {
                if frame.fin {
                    // Complete single-frame message
                    if self.recv_messages.len() < MAX_QUEUED_MESSAGES {
                        self.recv_messages.push(WsMessage {
                            opcode: frame.opcode,
                            data: frame.payload,
                        });
                    }
                } else {
                    // Start of fragmented message
                    self.frag_opcode = Some(frame.opcode);
                    self.frag_buf = frame.payload;
                }
            }
        }
    }

    /// Build a frame to send (applies masking if client)
    pub fn build_send_frame(&mut self, opcode: Opcode, data: &[u8]) -> Vec<u8> {
        let mut frame = WsFrame::new(opcode, data.to_vec(), true);
        if self.is_client {
            self.mask_counter = self
                .mask_counter
                .wrapping_mul(1103515245)
                .wrapping_add(12345);
            let key = self.mask_counter.to_le_bytes();
            frame.set_mask(key);
        }
        self.frames_sent = self.frames_sent.saturating_add(1);
        self.bytes_sent = self.bytes_sent.saturating_add(data.len() as u64);
        frame.encode()
    }

    /// Send a text message (returns encoded bytes to transmit)
    pub fn send_text(&mut self, text: &str) -> Vec<u8> {
        self.build_send_frame(Opcode::Text, text.as_bytes())
    }

    /// Send a binary message
    pub fn send_binary(&mut self, data: &[u8]) -> Vec<u8> {
        self.build_send_frame(Opcode::Binary, data)
    }

    /// Send a ping
    pub fn send_ping(&mut self, data: &[u8]) -> Vec<u8> {
        self.build_send_frame(Opcode::Ping, data)
    }

    /// Send a close frame
    pub fn send_close(&mut self, code: CloseCode, reason: &str) -> Vec<u8> {
        self.state = WsState::Closing;
        let close_frame = WsFrame::close(code, reason);
        let payload = close_frame.payload.clone();
        self.build_send_frame(Opcode::Close, &payload)
    }

    /// Receive a complete message (non-blocking)
    pub fn recv(&mut self) -> Option<WsMessage> {
        // Skip pong responses in the queue
        let idx = self
            .recv_messages
            .iter()
            .position(|m| m.opcode != Opcode::Pong);
        match idx {
            Some(i) => Some(self.recv_messages.remove(i)),
            None => None,
        }
    }

    /// Mark connection as open (after handshake)
    pub fn set_open(&mut self) {
        self.state = WsState::Open;
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct WsSubsystem {
    connections: Vec<WebSocket>,
    next_id: u32,
}

static WS: Mutex<Option<WsSubsystem>> = Mutex::new(None);

/// Initialize the WebSocket subsystem
pub fn init() {
    *WS.lock() = Some(WsSubsystem {
        connections: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: WebSocket subsystem initialized");
}

/// Create a new WebSocket connection
pub fn create_connection(url: &str, is_client: bool) -> Option<u32> {
    let mut guard = WS.lock();
    let sys = guard.as_mut()?;
    if sys.connections.len() >= MAX_CONNECTIONS {
        return None;
    }
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.connections.push(WebSocket::new(id, url, is_client));
    Some(id)
}

/// Close a WebSocket connection
pub fn close_connection(id: u32) {
    let mut guard = WS.lock();
    if let Some(sys) = guard.as_mut() {
        sys.connections.retain(|c| c.id != id);
    }
}
