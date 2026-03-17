//! Audio subsystem error types

use core::fmt;

/// Audio subsystem errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioError {
    /// Codec not found or not supported
    CodecNotFound,
    /// Codec initialization failed
    CodecInitFailed,
    /// Invalid audio format
    InvalidFormat,
    /// Decoding error
    DecodeError,
    /// Encoding error
    EncodeError,
    /// Buffer too small
    BufferTooSmall,
    /// Buffer overflow
    BufferOverflow,
    /// Invalid sample rate
    InvalidSampleRate,
    /// Invalid channel count
    InvalidChannels,
    /// Device not available
    DeviceUnavailable,
    /// Device I/O error
    DeviceIoError,
    /// Out of memory
    OutOfMemory,
    /// Feature not implemented
    NotImplemented,
    /// Invalid parameter
    InvalidParameter,
    /// Stream error
    StreamError,
    /// Hardware acceleration not available
    NoHardwareAcceleration,
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodecNotFound => write!(f, "Codec not found"),
            Self::CodecInitFailed => write!(f, "Codec initialization failed"),
            Self::InvalidFormat => write!(f, "Invalid audio format"),
            Self::DecodeError => write!(f, "Decoding error"),
            Self::EncodeError => write!(f, "Encoding error"),
            Self::BufferTooSmall => write!(f, "Buffer too small"),
            Self::BufferOverflow => write!(f, "Buffer overflow"),
            Self::InvalidSampleRate => write!(f, "Invalid sample rate"),
            Self::InvalidChannels => write!(f, "Invalid channel count"),
            Self::DeviceUnavailable => write!(f, "Audio device unavailable"),
            Self::DeviceIoError => write!(f, "Device I/O error"),
            Self::OutOfMemory => write!(f, "Out of memory"),
            Self::NotImplemented => write!(f, "Feature not implemented"),
            Self::InvalidParameter => write!(f, "Invalid parameter"),
            Self::StreamError => write!(f, "Stream error"),
            Self::NoHardwareAcceleration => write!(f, "Hardware acceleration unavailable"),
        }
    }
}

pub type Result<T> = core::result::Result<T, AudioError>;
