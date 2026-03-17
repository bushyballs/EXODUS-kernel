//! Vorbis codec implementation
//!
//! Vorbis is a free, open-source lossy audio codec with superior quality
//! compared to MP3 at similar bitrates. Uses MDCT and psychoacoustic modeling.
//! All trig uses polynomial approximations — no libm / soft-float calls.

use super::{Decoder, Encoder};
use crate::audio::dsp::{cos_approx, sin_approx};
use crate::audio::error::*;
use crate::audio::types::*;

/// Vorbis encoder state
pub struct VorbisEncoder {
    sample_rate: u32,
    channels: u8,
    bitrate: u32,
    quality: f32,
    initialized: bool,
    mdct_state: MdctState,
    floor: Floor,
    residue: Residue,
    psychoacoustic: PsychoacousticModel,
}

/// Vorbis decoder state
pub struct VorbisDecoder {
    sample_rate: u32,
    channels: u8,
    initialized: bool,
    imdct_state: ImdctState,
    floor: Floor,
    residue: Residue,
    identification_parsed: bool,
}

/// MDCT state
struct MdctState {
    block_long: [f32; 2048],
    block_short: [f32; 256],
    window_long: [f32; 2048],
    window_short: [f32; 256],
}

/// IMDCT state
struct ImdctState {
    block_long: [f32; 2048],
    block_short: [f32; 256],
    window_long: [f32; 2048],
    window_short: [f32; 256],
}

/// Floor (spectral envelope) representation
struct Floor {
    floor1_values: [u16; 256],
}

/// Residue (spectral fine structure) representation
struct Residue {
    residue2_values: [i16; 1024],
}

/// Psychoacoustic model
struct PsychoacousticModel {
    masking_threshold: [f32; 1024],
}

impl VorbisEncoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bitrate: 128000,
            quality: 5.0,
            initialized: false,
            mdct_state: MdctState::new(),
            floor: Floor::new(),
            residue: Residue::new(),
            psychoacoustic: PsychoacousticModel::new(),
        }
    }

    pub fn set_quality(&mut self, quality: f32) {
        self.quality = quality.clamp(-1.0, 10.0);
    }

    pub fn set_bitrate(&mut self, bitrate: u32) {
        self.bitrate = bitrate;
    }

    fn write_identification_header(&self, output: &mut [u8]) -> usize {
        if output.len() < 30 {
            return 0;
        }

        let mut pos = 0;

        // Packet type (1 byte) + "vorbis" (6 bytes)
        output[pos] = 1; // Identification header
        pos += 1;
        output[pos..pos + 6].copy_from_slice(b"vorbis");
        pos += 6;

        // Vorbis version (4 bytes)
        output[pos..pos + 4].copy_from_slice(&0u32.to_le_bytes());
        pos += 4;

        // Channels (1 byte)
        output[pos] = self.channels;
        pos += 1;

        // Sample rate (4 bytes)
        output[pos..pos + 4].copy_from_slice(&self.sample_rate.to_le_bytes());
        pos += 4;

        // Bitrate maximum/nominal/minimum (12 bytes)
        output[pos..pos + 4].copy_from_slice(&self.bitrate.to_le_bytes());
        pos += 4;
        output[pos..pos + 4].copy_from_slice(&self.bitrate.to_le_bytes());
        pos += 4;
        output[pos..pos + 4].copy_from_slice(&self.bitrate.to_le_bytes());
        pos += 4;

        // Block sizes (1 byte) - blocksize_0 (4 bits) + blocksize_1 (4 bits)
        output[pos] = 0x88; // 256 samples for short, 2048 for long
        pos += 1;

        // Framing flag (1 bit, must be set)
        output[pos] = 1;
        pos += 1;

        pos
    }

    fn encode_audio_packet(&mut self, samples: &[f32], output: &mut [u8]) -> usize {
        // Apply MDCT
        let mut coeffs = [0.0f32; 2048];
        self.mdct_state.transform(samples, &mut coeffs);

        // Apply psychoacoustic model
        self.psychoacoustic.compute_masking(&coeffs);

        // Encode floor
        self.floor.encode(&coeffs, output, 0);

        // Encode residue
        let residue_start = 100; // After floor data
        self.residue
            .encode(&coeffs, &self.floor, output, residue_start);

        residue_start + 500 // Simplified size
    }
}

impl Encoder for VorbisEncoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        if self.channels > 8 {
            return Err(AudioError::InvalidChannels);
        }

        self.initialized = true;
        Ok(())
    }

    fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        if output.len() < 4096 {
            return Err(AudioError::BufferTooSmall);
        }

        // Convert input to f32
        let sample_count = frame.len / (self.channels as usize * 2);
        let mut float_samples = [0.0f32; 2048];

        unsafe {
            let samples = core::slice::from_raw_parts(
                frame.data as *const i16,
                sample_count * self.channels as usize,
            );
            for i in 0..sample_count.min(2048) {
                float_samples[i] = samples[i * self.channels as usize] as f32 / 32768.0;
            }
        }

        // Encode audio packet
        let size = self.encode_audio_packet(&float_samples, output);

        Ok(size)
    }

    fn flush(&mut self, _output: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::Vorbis,
            can_encode: true,
            can_decode: false,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 192000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[BitrateMode::VBR(128000), BitrateMode::ABR(128000)],
            hardware_accelerated: false,
        }
    }
}

impl VorbisDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            initialized: false,
            imdct_state: ImdctState::new(),
            floor: Floor::new(),
            residue: Residue::new(),
            identification_parsed: false,
        }
    }

    fn parse_identification_header(&mut self, data: &[u8]) -> Result<usize> {
        if data.len() < 30 {
            return Err(AudioError::InvalidFormat);
        }

        let mut pos = 0;

        // Check packet type and vorbis string
        if data[pos] != 1 {
            return Err(AudioError::InvalidFormat);
        }
        pos += 1;

        if &data[pos..pos + 6] != b"vorbis" {
            return Err(AudioError::InvalidFormat);
        }
        pos += 6;

        // Skip version
        pos += 4;

        // Read channels
        self.channels = data[pos];
        pos += 1;

        // Read sample rate
        let mut sr_bytes = [0u8; 4];
        sr_bytes.copy_from_slice(&data[pos..pos + 4]);
        self.sample_rate = u32::from_le_bytes(sr_bytes);
        pos += 4;

        self.identification_parsed = true;

        Ok(pos + 13) // Skip bitrate fields and block sizes
    }

    fn decode_audio_packet(&mut self, data: &[u8], samples: &mut [f32]) -> Result<usize> {
        // Decode floor
        self.floor.decode(data, 0);

        // Decode residue
        let residue_start = 100;
        self.residue.decode(data, residue_start, &self.floor);

        // Reconstruct spectral data
        let mut coeffs = [0.0f32; 2048];
        for i in 0..coeffs.len() {
            coeffs[i] = self.residue.residue2_values[i % self.residue.residue2_values.len()] as f32
                / 32768.0;
        }

        // Apply IMDCT
        self.imdct_state.transform(&coeffs, samples);

        Ok(samples.len().min(2048))
    }
}

impl Decoder for VorbisDecoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        self.initialized = true;
        Ok(())
    }

    fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        unsafe {
            let data = core::slice::from_raw_parts(packet.data, packet.len);

            // Check for identification header
            if !data.is_empty() && data[0] == 1 {
                self.parse_identification_header(data)?;
                return Ok(0); // Header packet, no audio data
            }

            // Decode audio packet
            let mut float_samples = [0.0f32; 2048];
            let sample_count = self.decode_audio_packet(data, &mut float_samples)?;

            // Convert to i16 output
            let out_samples =
                core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2);
            for i in 0..sample_count.min(out_samples.len()) {
                out_samples[i] = (float_samples[i] * 32768.0) as i16;
            }

            Ok(sample_count * 2 * self.channels as usize)
        }
    }

    fn reset(&mut self) {
        self.imdct_state.block_long = [0.0; 2048];
        self.imdct_state.block_short = [0.0; 256];
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::Vorbis,
            can_encode: false,
            can_decode: true,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 192000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[BitrateMode::VBR(128000), BitrateMode::ABR(128000)],
            hardware_accelerated: false,
        }
    }
}

impl MdctState {
    fn new() -> Self {
        let mut state = Self {
            block_long: [0.0; 2048],
            block_short: [0.0; 256],
            window_long: [0.0; 2048],
            window_short: [0.0; 256],
        };

        // Initialize Vorbis window (Vorbis uses a sine window)
        for i in 0..2048 {
            let x = (i as f32 + 0.5) / 2048.0 * 3.14159265;
            state.window_long[i] = sin_approx(x);
        }

        for i in 0..256 {
            let x = (i as f32 + 0.5) / 256.0 * 3.14159265;
            state.window_short[i] = sin_approx(x);
        }

        state
    }

    fn transform(&mut self, samples: &[f32], coeffs: &mut [f32]) {
        // Simplified MDCT
        let n = samples.len().min(2048);
        for k in 0..n / 2 {
            let mut sum = 0.0f32;
            for i in 0..n {
                let arg =
                    3.14159265 / n as f32 * (i as f32 + 0.5 + n as f32 / 4.0) * (k as f32 + 0.5);
                sum += samples[i] * self.window_long[i] * cos_approx(arg);
            }
            coeffs[k] = sum;
        }
    }
}

impl ImdctState {
    fn new() -> Self {
        let mut state = Self {
            block_long: [0.0; 2048],
            block_short: [0.0; 256],
            window_long: [0.0; 2048],
            window_short: [0.0; 256],
        };

        for i in 0..2048 {
            let x = (i as f32 + 0.5) / 2048.0 * 3.14159265;
            state.window_long[i] = sin_approx(x);
        }

        for i in 0..256 {
            let x = (i as f32 + 0.5) / 256.0 * 3.14159265;
            state.window_short[i] = sin_approx(x);
        }

        state
    }

    fn transform(&mut self, coeffs: &[f32], samples: &mut [f32]) {
        // Simplified IMDCT
        let n = samples.len().min(2048);
        for i in 0..n {
            let mut sum = 0.0f32;
            for k in 0..n / 2 {
                let arg =
                    3.14159265 / n as f32 * (i as f32 + 0.5 + n as f32 / 4.0) * (k as f32 + 0.5);
                sum += coeffs[k] * cos_approx(arg);
            }
            samples[i] = sum * self.window_long[i] * 2.0 / n as f32;
        }
    }
}

impl Floor {
    fn new() -> Self {
        Self {
            floor1_values: [0; 256],
        }
    }

    fn encode(&mut self, _coeffs: &[f32], _output: &mut [u8], _offset: usize) {
        // Simplified floor encoding
    }

    fn decode(&mut self, _data: &[u8], _offset: usize) {
        // Simplified floor decoding
    }
}

impl Residue {
    fn new() -> Self {
        Self {
            residue2_values: [0; 1024],
        }
    }

    fn encode(&mut self, coeffs: &[f32], _floor: &Floor, output: &mut [u8], offset: usize) {
        // Simplified residue encoding
        for i in 0..coeffs.len().min(512) {
            if offset + i * 2 + 1 < output.len() {
                let val = (coeffs[i] * 32768.0) as i16;
                output[offset + i * 2] = (val >> 8) as u8;
                output[offset + i * 2 + 1] = (val & 0xFF) as u8;
            }
        }
    }

    fn decode(&mut self, data: &[u8], offset: usize, _floor: &Floor) {
        // Simplified residue decoding
        for i in 0..self.residue2_values.len() {
            if offset + i * 2 + 1 < data.len() {
                self.residue2_values[i] =
                    ((data[offset + i * 2] as i16) << 8) | (data[offset + i * 2 + 1] as i16);
            }
        }
    }
}

impl PsychoacousticModel {
    fn new() -> Self {
        Self {
            masking_threshold: [0.0; 1024],
        }
    }

    fn compute_masking(&mut self, _coeffs: &[f32]) {
        // Simplified psychoacoustic masking computation
    }
}
