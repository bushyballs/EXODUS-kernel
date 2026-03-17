//! Audio container format parsers and muxers
//!
//! Handles container formats like MP4, OGG, MKV, WAV, etc.

use super::error::*;
use super::types::*;

/// Audio container format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    WAV,
    MP4,
    OGG,
    FLAC,
    MKV,
    WEBM,
    Raw,
}

/// Container format parser
pub trait FormatParser {
    /// Parse container header
    fn parse_header(&mut self, data: &[u8]) -> Result<FormatInfo>;

    /// Extract next audio packet
    fn next_packet(&mut self, data: &[u8]) -> Result<Option<AudioPacket>>;

    /// Get format information
    fn get_info(&self) -> &FormatInfo;
}

/// Container format muxer
pub trait FormatMuxer {
    /// Write container header
    fn write_header(&mut self, output: &mut [u8], info: &FormatInfo) -> Result<usize>;

    /// Write audio packet
    fn write_packet(&mut self, output: &mut [u8], packet: &AudioPacket) -> Result<usize>;

    /// Finalize container
    fn finalize(&mut self, output: &mut [u8]) -> Result<usize>;
}

/// Format information
#[derive(Debug, Clone)]
pub struct FormatInfo {
    pub codec: CodecId,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
    pub duration_us: u64,
    pub bitrate: u32,
}

/// Fallback FormatInfo returned by get_info() when parse_header() has not
/// yet been called successfully.  All fields are zero / PCM defaults so
/// callers receive a well-defined value instead of a kernel panic.
static DEFAULT_FORMAT_INFO: FormatInfo = FormatInfo {
    codec: CodecId::PCM,
    sample_rate: 0,
    channels: 0,
    bit_depth: 0,
    duration_us: 0,
    bitrate: 0,
};

/// WAV format parser
pub struct WavParser {
    info: Option<FormatInfo>,
    data_offset: usize,
    data_size: usize,
}

/// WAV format muxer
pub struct WavMuxer {
    info: FormatInfo,
    data_written: usize,
}

/// OGG format parser
pub struct OggParser {
    info: Option<FormatInfo>,
    page_offset: usize,
}

/// MP4 format parser
pub struct Mp4Parser {
    info: Option<FormatInfo>,
    current_sample: usize,
}

impl WavParser {
    pub fn new() -> Self {
        Self {
            info: None,
            data_offset: 0,
            data_size: 0,
        }
    }

    fn parse_fmt_chunk(&mut self, data: &[u8]) -> Result<()> {
        if data.len() < 16 {
            return Err(AudioError::InvalidFormat);
        }

        let audio_format = u16::from_le_bytes([data[0], data[1]]);
        if audio_format != 1 && audio_format != 3 {
            // PCM or IEEE float
            return Err(AudioError::InvalidFormat);
        }

        let channels = u16::from_le_bytes([data[2], data[3]]) as u8;
        let sample_rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let bits_per_sample = u16::from_le_bytes([data[14], data[15]]) as u8;

        self.info = Some(FormatInfo {
            codec: CodecId::PCM,
            sample_rate,
            channels,
            bit_depth: bits_per_sample,
            duration_us: 0,
            bitrate: sample_rate * channels as u32 * bits_per_sample as u32,
        });

        Ok(())
    }
}

impl FormatParser for WavParser {
    fn parse_header(&mut self, data: &[u8]) -> Result<FormatInfo> {
        if data.len() < 44 {
            return Err(AudioError::InvalidFormat);
        }

        // Check RIFF header
        if &data[0..4] != b"RIFF" {
            return Err(AudioError::InvalidFormat);
        }

        // Check WAVE format
        if &data[8..12] != b"WAVE" {
            return Err(AudioError::InvalidFormat);
        }

        let mut pos = 12;

        // Parse chunks
        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
                    as usize;

            pos += 8;

            if chunk_id == b"fmt " {
                if pos + chunk_size > data.len() {
                    return Err(AudioError::InvalidFormat);
                }
                self.parse_fmt_chunk(&data[pos..pos + chunk_size])?;
            } else if chunk_id == b"data" {
                self.data_offset = pos;
                self.data_size = chunk_size;
                break;
            }

            pos += chunk_size;
            if chunk_size % 2 != 0 {
                pos += 1; // Padding
            }
        }

        self.info.clone().ok_or(AudioError::InvalidFormat)
    }

    fn next_packet(&mut self, data: &[u8]) -> Result<Option<AudioPacket>> {
        if self.data_offset == 0 || self.data_offset >= data.len() {
            return Ok(None);
        }

        let packet_size = 4096.min(self.data_size);
        if self.data_offset + packet_size > data.len() {
            return Ok(None);
        }

        let packet = AudioPacket {
            data: data[self.data_offset..].as_ptr(),
            len: packet_size,
            codec: CodecId::PCM,
            timestamp_us: 0,
            duration_us: 0,
            is_keyframe: true,
        };

        self.data_offset += packet_size;
        self.data_size -= packet_size;

        Ok(Some(packet))
    }

    fn get_info(&self) -> &FormatInfo {
        // Fallback to DEFAULT_FORMAT_INFO if parse_header() was never called
        // successfully, avoiding a kernel panic in no_std context.
        self.info.as_ref().unwrap_or(&DEFAULT_FORMAT_INFO)
    }
}

impl WavMuxer {
    pub fn new(info: FormatInfo) -> Self {
        Self {
            info,
            data_written: 0,
        }
    }
}

impl FormatMuxer for WavMuxer {
    fn write_header(&mut self, output: &mut [u8], info: &FormatInfo) -> Result<usize> {
        if output.len() < 44 {
            return Err(AudioError::BufferTooSmall);
        }

        let mut pos = 0;

        // RIFF header
        output[pos..pos + 4].copy_from_slice(b"RIFF");
        pos += 4;

        // File size (placeholder, will be updated in finalize)
        output[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
        pos += 4;

        // WAVE format
        output[pos..pos + 4].copy_from_slice(b"WAVE");
        pos += 4;

        // fmt chunk
        output[pos..pos + 4].copy_from_slice(b"fmt ");
        pos += 4;

        // fmt chunk size
        output[pos..pos + 4].copy_from_slice(&16u32.to_le_bytes());
        pos += 4;

        // Audio format (1 = PCM)
        output[pos..pos + 2].copy_from_slice(&1u16.to_le_bytes());
        pos += 2;

        // Channels
        output[pos..pos + 2].copy_from_slice(&(info.channels as u16).to_le_bytes());
        pos += 2;

        // Sample rate
        output[pos..pos + 4].copy_from_slice(&info.sample_rate.to_le_bytes());
        pos += 4;

        // Byte rate
        let byte_rate = info.sample_rate * info.channels as u32 * (info.bit_depth as u32 / 8);
        output[pos..pos + 4].copy_from_slice(&byte_rate.to_le_bytes());
        pos += 4;

        // Block align
        let block_align = info.channels as u16 * (info.bit_depth as u16 / 8);
        output[pos..pos + 2].copy_from_slice(&block_align.to_le_bytes());
        pos += 2;

        // Bits per sample
        output[pos..pos + 2].copy_from_slice(&(info.bit_depth as u16).to_le_bytes());
        pos += 2;

        // data chunk
        output[pos..pos + 4].copy_from_slice(b"data");
        pos += 4;

        // data chunk size (placeholder)
        output[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
        pos += 4;

        Ok(pos)
    }

    fn write_packet(&mut self, output: &mut [u8], packet: &AudioPacket) -> Result<usize> {
        if output.len() < packet.len {
            return Err(AudioError::BufferTooSmall);
        }

        unsafe {
            let data = core::slice::from_raw_parts(packet.data, packet.len);
            output[..packet.len].copy_from_slice(data);
        }

        self.data_written += packet.len;

        Ok(packet.len)
    }

    fn finalize(&mut self, output: &mut [u8]) -> Result<usize> {
        if output.len() < 44 {
            return Err(AudioError::BufferTooSmall);
        }

        // Update RIFF chunk size (file size - 8)
        let riff_size = (36 + self.data_written) as u32;
        output[4..8].copy_from_slice(&riff_size.to_le_bytes());

        // Update data chunk size
        output[40..44].copy_from_slice(&(self.data_written as u32).to_le_bytes());

        Ok(0)
    }
}

impl OggParser {
    pub fn new() -> Self {
        Self {
            info: None,
            page_offset: 0,
        }
    }
}

impl FormatParser for OggParser {
    fn parse_header(&mut self, data: &[u8]) -> Result<FormatInfo> {
        if data.len() < 27 {
            return Err(AudioError::InvalidFormat);
        }

        // Check OGG page header
        if &data[0..4] != b"OggS" {
            return Err(AudioError::InvalidFormat);
        }

        // This is a simplified parser - real implementation would
        // parse Vorbis/Opus identification headers

        self.info = Some(FormatInfo {
            codec: CodecId::Vorbis,
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            duration_us: 0,
            bitrate: 128000,
        });

        self.page_offset = 27;

        self.info.clone().ok_or(AudioError::InvalidFormat)
    }

    fn next_packet(&mut self, _data: &[u8]) -> Result<Option<AudioPacket>> {
        // Simplified - would parse OGG pages and extract packets
        Ok(None)
    }

    fn get_info(&self) -> &FormatInfo {
        self.info.as_ref().unwrap_or(&DEFAULT_FORMAT_INFO)
    }
}

impl Mp4Parser {
    pub fn new() -> Self {
        Self {
            info: None,
            current_sample: 0,
        }
    }
}

impl FormatParser for Mp4Parser {
    fn parse_header(&mut self, data: &[u8]) -> Result<FormatInfo> {
        if data.len() < 8 {
            return Err(AudioError::InvalidFormat);
        }

        // Check for ftyp box
        if &data[4..8] != b"ftyp" {
            return Err(AudioError::InvalidFormat);
        }

        // Simplified MP4 parser - would parse moov/trak/mdia/minf boxes

        self.info = Some(FormatInfo {
            codec: CodecId::AAC,
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            duration_us: 0,
            bitrate: 128000,
        });

        self.info.clone().ok_or(AudioError::InvalidFormat)
    }

    fn next_packet(&mut self, _data: &[u8]) -> Result<Option<AudioPacket>> {
        // Simplified - would extract samples from mdat box
        Ok(None)
    }

    fn get_info(&self) -> &FormatInfo {
        self.info.as_ref().unwrap_or(&DEFAULT_FORMAT_INFO)
    }
}

/// Detect container format from data
pub fn detect_format(data: &[u8]) -> Option<ContainerFormat> {
    if data.len() < 12 {
        return None;
    }

    if &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        Some(ContainerFormat::WAV)
    } else if &data[0..4] == b"fLaC" {
        Some(ContainerFormat::FLAC)
    } else if &data[0..4] == b"OggS" {
        Some(ContainerFormat::OGG)
    } else if &data[4..8] == b"ftyp" {
        Some(ContainerFormat::MP4)
    } else if data.len() >= 4 && &data[0..4] == b"\x1A\x45\xDF\xA3" {
        Some(ContainerFormat::MKV)
    } else {
        None
    }
}
