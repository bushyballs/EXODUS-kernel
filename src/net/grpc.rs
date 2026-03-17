use crate::sync::Mutex;
/// gRPC binary RPC protocol
///
/// Provides gRPC framing over HTTP/2: length-prefixed protobuf messages,
/// unary and streaming call types, status codes, and metadata headers.
///
/// Inspired by: gRPC specification, gRPC over HTTP/2. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Status codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcStatus {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
}

impl GrpcStatus {
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => GrpcStatus::Ok,
            1 => GrpcStatus::Cancelled,
            3 => GrpcStatus::InvalidArgument,
            4 => GrpcStatus::DeadlineExceeded,
            5 => GrpcStatus::NotFound,
            12 => GrpcStatus::Unimplemented,
            13 => GrpcStatus::Internal,
            14 => GrpcStatus::Unavailable,
            _ => GrpcStatus::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// gRPC message framing
// ---------------------------------------------------------------------------

/// gRPC length-prefixed message
///
/// Format: [compressed(1 byte)][length(4 bytes big-endian)][message(length bytes)]
#[derive(Debug, Clone)]
pub struct GrpcMessage {
    pub compressed: bool,
    pub data: Vec<u8>,
}

impl GrpcMessage {
    pub fn new(data: Vec<u8>) -> Self {
        GrpcMessage {
            compressed: false,
            data,
        }
    }

    /// Encode to wire format (5-byte prefix + payload)
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(5 + self.data.len());
        buf.push(if self.compressed { 1 } else { 0 });
        buf.extend_from_slice(&(self.data.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Decode from wire format
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.len() < 5 {
            return None;
        }
        let compressed = data[0] != 0;
        let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
        if data.len() < 5 + length {
            return None;
        }
        let payload = data[5..5 + length].to_vec();
        Some((
            GrpcMessage {
                compressed,
                data: payload,
            },
            5 + length,
        ))
    }
}

// ---------------------------------------------------------------------------
// gRPC metadata (headers/trailers)
// ---------------------------------------------------------------------------

/// gRPC metadata entry
#[derive(Debug, Clone)]
pub struct GrpcMetadata {
    pub key: String,
    pub value: String,
}

/// Build gRPC request headers
pub fn build_request_headers(
    service: &str,
    method: &str,
    content_type: &str,
) -> Vec<(String, String)> {
    let path = alloc::format!("/{}/{}", service, method);
    alloc::vec![
        (String::from(":method"), String::from("POST")),
        (String::from(":scheme"), String::from("http")),
        (String::from(":path"), path),
        (String::from("content-type"), String::from(content_type)),
        (String::from("te"), String::from("trailers")),
        (String::from("grpc-encoding"), String::from("identity")),
    ]
}

/// Build gRPC response headers
pub fn build_response_headers() -> Vec<(String, String)> {
    alloc::vec![
        (String::from(":status"), String::from("200")),
        (
            String::from("content-type"),
            String::from("application/grpc")
        ),
    ]
}

/// Build gRPC trailers
pub fn build_trailers(status: GrpcStatus, message: &str) -> Vec<(String, String)> {
    let mut trailers = alloc::vec![(
        String::from("grpc-status"),
        alloc::format!("{}", status as u32)
    ),];
    if !message.is_empty() {
        trailers.push((String::from("grpc-message"), String::from(message)));
    }
    trailers
}

// ---------------------------------------------------------------------------
// Call types
// ---------------------------------------------------------------------------

/// gRPC call type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallType {
    Unary,
    ServerStreaming,
    ClientStreaming,
    BidiStreaming,
}

/// gRPC call state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Initial,
    HeadersSent,
    Streaming,
    HalfClosed,
    Closed,
}

/// A gRPC call
#[derive(Debug, Clone)]
pub struct GrpcCall {
    pub call_type: CallType,
    pub state: CallState,
    pub service: String,
    pub method: String,
    pub request_messages: Vec<GrpcMessage>,
    pub response_messages: Vec<GrpcMessage>,
    pub status: Option<GrpcStatus>,
    pub status_message: Option<String>,
    pub metadata: Vec<GrpcMetadata>,
    pub stream_id: u32,
}

// ---------------------------------------------------------------------------
// gRPC channel
// ---------------------------------------------------------------------------

/// gRPC channel (connection to a server)
pub struct GrpcChannel {
    pub host: String,
    pub port: u16,
    calls: Vec<GrpcCall>,
    next_call_id: u32,
}

impl GrpcChannel {
    pub fn new(host: &str, port: u16) -> Self {
        GrpcChannel {
            host: String::from(host),
            port,
            calls: Vec::new(),
            next_call_id: 1,
        }
    }

    /// Start a unary call
    pub fn call(&mut self, service: &str, method: &str, payload: &[u8]) -> Result<u32, GrpcStatus> {
        let call_id = self.next_call_id;
        self.next_call_id = self.next_call_id.saturating_add(1);
        self.calls.push(GrpcCall {
            call_type: CallType::Unary,
            state: CallState::Initial,
            service: String::from(service),
            method: String::from(method),
            request_messages: alloc::vec![GrpcMessage::new(payload.to_vec())],
            response_messages: Vec::new(),
            status: None,
            status_message: None,
            metadata: Vec::new(),
            stream_id: call_id,
        });
        Ok(call_id)
    }

    /// Get call result
    pub fn get_result(&self, call_id: u32) -> Option<&GrpcCall> {
        self.calls.iter().find(|c| c.stream_id == call_id)
    }

    /// Receive response for a call
    pub fn recv_response(&mut self, call_id: u32) -> Option<Vec<u8>> {
        let call = self.calls.iter_mut().find(|c| c.stream_id == call_id)?;
        if call.response_messages.is_empty() {
            None
        } else {
            Some(call.response_messages.remove(0).data)
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GRPC: Mutex<Option<Vec<GrpcChannel>>> = Mutex::new(None);

pub fn init() {
    *GRPC.lock() = Some(Vec::new());
    serial_println!("  Net: gRPC subsystem initialized");
}
