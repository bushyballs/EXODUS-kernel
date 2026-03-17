//! Core audio types and structures

use core::fmt;

/// Audio sample format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Unsigned 8-bit PCM
    U8,
    /// Signed 16-bit PCM (little-endian)
    S16LE,
    /// Signed 24-bit PCM (little-endian)
    S24LE,
    /// Signed 32-bit PCM (little-endian)
    S32LE,
    /// 32-bit floating point
    F32LE,
    /// 64-bit floating point
    F64LE,
}

impl SampleFormat {
    pub fn bytes_per_sample(&self) -> usize {
        match self {
            Self::U8 => 1,
            Self::S16LE => 2,
            Self::S24LE => 3,
            Self::S32LE => 4,
            Self::F32LE => 4,
            Self::F64LE => 8,
        }
    }
}

/// Audio codec identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecId {
    AAC,
    MP3,
    Opus,
    FLAC,
    Vorbis,
    ALAC,
    LDAC,
    PCM,
}

impl fmt::Display for CodecId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecId::AAC => write!(f, "AAC"),
            CodecId::MP3 => write!(f, "MP3"),
            CodecId::Opus => write!(f, "Opus"),
            CodecId::FLAC => write!(f, "FLAC"),
            CodecId::Vorbis => write!(f, "Vorbis"),
            CodecId::ALAC => write!(f, "ALAC"),
            CodecId::LDAC => write!(f, "LDAC"),
            CodecId::PCM => write!(f, "PCM"),
        }
    }
}

/// Audio channel layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Surround2_1,
    Surround3_0,
    Surround4_0,
    Surround5_0,
    Surround5_1,
    Surround7_1,
}

impl ChannelLayout {
    pub fn channel_count(&self) -> u8 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Surround2_1 => 3,
            Self::Surround3_0 => 3,
            Self::Surround4_0 => 4,
            Self::Surround5_0 => 5,
            Self::Surround5_1 => 6,
            Self::Surround7_1 => 8,
        }
    }
}

/// Audio bitrate mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitrateMode {
    /// Constant bitrate
    CBR(u32),
    /// Variable bitrate with target average
    VBR(u32),
    /// Average bitrate
    ABR(u32),
}

/// Audio frame information
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub data: *const u8,
    pub len: usize,
    pub sample_rate: u32,
    pub channels: u8,
    pub format: SampleFormat,
    pub timestamp_us: u64,
}

/// Audio packet (encoded data)
#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub data: *const u8,
    pub len: usize,
    pub codec: CodecId,
    pub timestamp_us: u64,
    pub duration_us: u64,
    pub is_keyframe: bool,
}

/// Audio configuration
#[derive(Debug, Clone, Copy)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
    pub buffer_size: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            buffer_size: 4096,
        }
    }
}

/// Codec capabilities
#[derive(Debug, Clone)]
pub struct CodecCapabilities {
    pub codec_id: CodecId,
    pub can_encode: bool,
    pub can_decode: bool,
    pub supported_sample_rates: &'static [u32],
    pub supported_channel_layouts: &'static [ChannelLayout],
    pub supported_bitrate_modes: &'static [BitrateMode],
    pub hardware_accelerated: bool,
}
