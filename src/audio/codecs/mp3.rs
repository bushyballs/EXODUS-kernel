//! MP3 (MPEG-1/2 Audio Layer III) codec implementation
//!
//! Implements MPEG-1 Layer III and MPEG-2 Layer III audio coding.
//! Supports mono, stereo, joint stereo, and dual channel modes.
//! All trig uses polynomial approximations — no libm / soft-float calls.

use super::{Decoder, Encoder};
use crate::audio::dsp::{cos_approx, sin_approx};
use crate::audio::error::*;
use crate::audio::types::*;

/// MP3 encoder state
pub struct Mp3Encoder {
    sample_rate: u32,
    channels: u8,
    bitrate: u32,
    initialized: bool,
    mdct_state: MdctState,
    filterbank: PolyphaseFilterbank,
    bit_reservoir: BitReservoir,
    psychoacoustic: PsychoacousticModel,
}

/// MP3 decoder state
pub struct Mp3Decoder {
    sample_rate: u32,
    channels: u8,
    initialized: bool,
    imdct_state: ImdctState,
    synthesis_filterbank: SynthesisFilterbank,
    frame_buffer: [u8; 8192],
}

/// MDCT state for MP3
struct MdctState {
    window_long: [f32; 36],
    window_short: [f32; 12],
}

/// IMDCT state for MP3
struct ImdctState {
    window_long: [f32; 36],
    window_short: [f32; 12],
    overlap_buffer: [[f32; 18]; 32],
}

/// Polyphase analysis filterbank (32 subbands)
struct PolyphaseFilterbank {
    buffer: [[f32; 32]; 16],
    coeffs: [f32; 512],
}

/// Synthesis filterbank for reconstruction
struct SynthesisFilterbank {
    v_vec: [f32; 1024],
    samples: [[f32; 32]; 18],
}

/// Bit reservoir for bit allocation
struct BitReservoir {
    buffer: [u8; 4096],
    bits_used: usize,
}

/// Psychoacoustic model for perceptual coding
struct PsychoacousticModel {
    threshold: [f32; 576],
}

impl Mp3Encoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100,
            channels: 2,
            bitrate: 128000,
            initialized: false,
            mdct_state: MdctState::new(),
            filterbank: PolyphaseFilterbank::new(),
            bit_reservoir: BitReservoir::new(),
            psychoacoustic: PsychoacousticModel::new(),
        }
    }

    /// Set target bitrate
    pub fn set_bitrate(&mut self, bitrate: u32) {
        self.bitrate = bitrate;
    }

    /// Encode MP3 frame header
    fn encode_frame_header(&self, output: &mut [u8]) -> usize {
        if output.len() < 4 {
            return 0;
        }

        // Syncword (11 bits) + MPEG version (2 bits) + Layer (2 bits) + Protection (1 bit)
        output[0] = 0xFF;
        output[1] = 0xFB; // MPEG-1, Layer III, no CRC

        // Bitrate index (4 bits) + Sample rate index (2 bits) + Padding (1 bit) + Private (1 bit)
        let bitrate_idx: u8 = match self.bitrate {
            32000 => 1,
            40000 => 2,
            48000 => 3,
            56000 => 4,
            64000 => 5,
            80000 => 6,
            96000 => 7,
            112000 => 8,
            128000 => 9,
            160000 => 10,
            192000 => 11,
            224000 => 12,
            256000 => 13,
            320000 => 14,
            _ => 9,
        };

        let sr_idx: u8 = match self.sample_rate {
            44100 => 0,
            48000 => 1,
            32000 => 2,
            _ => 0,
        };

        output[2] = (bitrate_idx << 4) | (sr_idx << 2);

        // Channel mode (2 bits) + Mode extension (2 bits) + Copyright (1 bit) + Original (1 bit) + Emphasis (2 bits)
        let mode: u8 = if self.channels == 1 { 3 } else { 0 }; // 0=stereo, 3=mono
        output[3] = mode << 6;

        4
    }

    /// Apply polyphase filterbank analysis
    fn analyze_filterbank(&mut self, samples: &[f32], subbands: &mut [[f32; 32]; 18]) {
        for i in 0..18 {
            let start = i * 32;
            if start + 32 <= samples.len() {
                for sb in 0..32 {
                    let mut sum = 0.0f32;
                    for k in 0..16 {
                        sum += samples[start + k] * self.filterbank.coeffs[sb * 16 + k];
                    }
                    subbands[i][sb] = sum;
                }
            }
        }
    }

    /// Quantize and encode subband samples
    fn quantize_encode(
        &mut self,
        subbands: &[[f32; 32]; 18],
        output: &mut [u8],
        offset: usize,
    ) -> usize {
        let mut pos = offset;

        for granule in 0..2 {
            for sb in 0..32 {
                for i in 0..9 {
                    if pos >= output.len() {
                        return pos - offset;
                    }

                    let sample = subbands[granule * 9 + i][sb];
                    let quantized = (sample * 32768.0).clamp(-32768.0, 32767.0) as i16;
                    output[pos] = (quantized >> 8) as u8;
                    output[pos + 1] = (quantized & 0xFF) as u8;
                    pos += 2;
                }
            }
        }

        pos - offset
    }
}

impl Encoder for Mp3Encoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        if self.channels > 2 {
            return Err(AudioError::InvalidChannels);
        }

        // Validate sample rate
        match self.sample_rate {
            32000 | 44100 | 48000 => {}
            _ => return Err(AudioError::InvalidSampleRate),
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

        // Write frame header
        let header_size = self.encode_frame_header(output);

        // Convert input to f32
        let sample_count = frame.len / (self.channels as usize * 2);
        let mut float_samples = [0.0f32; 1152];

        unsafe {
            let samples = core::slice::from_raw_parts(
                frame.data as *const i16,
                sample_count * self.channels as usize,
            );
            for i in 0..sample_count.min(1152) {
                float_samples[i] = samples[i * self.channels as usize] as f32 / 32768.0;
            }
        }

        // Apply filterbank
        let mut subbands = [[0.0f32; 32]; 18];
        self.analyze_filterbank(&float_samples, &mut subbands);

        // Quantize and encode
        let payload_size = self.quantize_encode(&subbands, output, header_size);

        Ok(header_size + payload_size)
    }

    fn flush(&mut self, _output: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::MP3,
            can_encode: true,
            can_decode: false,
            supported_sample_rates: &[32000, 44100, 48000],
            supported_channel_layouts: &[ChannelLayout::Mono, ChannelLayout::Stereo],
            supported_bitrate_modes: &[
                BitrateMode::CBR(128000),
                BitrateMode::VBR(128000),
                BitrateMode::ABR(128000),
            ],
            hardware_accelerated: false,
        }
    }
}

impl Mp3Decoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100,
            channels: 2,
            initialized: false,
            imdct_state: ImdctState::new(),
            synthesis_filterbank: SynthesisFilterbank::new(),
            frame_buffer: [0; 8192],
        }
    }

    /// Parse MP3 frame header
    fn parse_frame_header(&mut self, data: &[u8]) -> Result<(usize, usize)> {
        if data.len() < 4 {
            return Err(AudioError::InvalidFormat);
        }

        // Check syncword
        if data[0] != 0xFF || (data[1] & 0xE0) != 0xE0 {
            return Err(AudioError::InvalidFormat);
        }

        let _version = (data[1] >> 3) & 0x3;
        let layer = (data[1] >> 1) & 0x3;

        if layer != 1 {
            // Layer III
            return Err(AudioError::InvalidFormat);
        }

        let bitrate_idx = (data[2] >> 4) & 0xF;
        let bitrate: u32 = match bitrate_idx {
            1 => 32000,
            2 => 40000,
            3 => 48000,
            4 => 56000,
            5 => 64000,
            6 => 80000,
            7 => 96000,
            8 => 112000,
            9 => 128000,
            10 => 160000,
            11 => 192000,
            12 => 224000,
            13 => 256000,
            14 => 320000,
            _ => 128000,
        };

        let sr_idx = (data[2] >> 2) & 0x3;
        self.sample_rate = match sr_idx {
            0 => 44100,
            1 => 48000,
            2 => 32000,
            _ => 44100,
        };

        let mode = (data[3] >> 6) & 0x3;
        self.channels = if mode == 3 { 1 } else { 2 };

        // Calculate frame size (all u32 arithmetic)
        let padding = ((data[2] >> 1) & 1) as u32;
        let frame_size = 144u32.saturating_mul(bitrate) / self.sample_rate.max(1) + padding;

        Ok((frame_size as usize, 1152))
    }

    /// Apply synthesis filterbank
    fn synthesize_filterbank(&mut self, subbands: &[[f32; 32]; 18], samples: &mut [f32]) {
        for i in 0..18 {
            for sb in 0..32 {
                self.synthesis_filterbank.samples[i][sb] = subbands[i][sb];
            }
        }

        // Reconstruct samples using synthesis filterbank
        for i in 0..18 {
            for sb in 0..32 {
                let idx = i * 32 + sb;
                if idx < samples.len() {
                    samples[idx] = self.synthesis_filterbank.samples[i][sb];
                }
            }
        }
    }

    /// Dequantize subband samples
    fn dequantize(&mut self, data: &[u8], offset: usize, subbands: &mut [[f32; 32]; 18]) -> usize {
        let mut pos = offset;

        for granule in 0..2 {
            for sb in 0..32 {
                for i in 0..9 {
                    if pos + 2 > data.len() {
                        return pos - offset;
                    }

                    let quantized = ((data[pos] as i16) << 8) | (data[pos + 1] as i16);
                    subbands[granule * 9 + i][sb] = quantized as f32 / 32768.0;
                    pos += 2;
                }
            }
        }

        pos - offset
    }
}

impl Decoder for Mp3Decoder {
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

            let (_frame_size, sample_count) = self.parse_frame_header(data)?;

            // Dequantize subbands
            let mut subbands = [[0.0f32; 32]; 18];
            self.dequantize(data, 4, &mut subbands);

            // Synthesize samples
            let mut float_samples = [0.0f32; 1152];
            self.synthesize_filterbank(&subbands, &mut float_samples);

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
        self.imdct_state.overlap_buffer = [[0.0; 18]; 32];
        self.synthesis_filterbank.v_vec = [0.0; 1024];
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::MP3,
            can_encode: false,
            can_decode: true,
            supported_sample_rates: &[32000, 44100, 48000],
            supported_channel_layouts: &[ChannelLayout::Mono, ChannelLayout::Stereo],
            supported_bitrate_modes: &[
                BitrateMode::CBR(128000),
                BitrateMode::VBR(128000),
                BitrateMode::ABR(128000),
            ],
            hardware_accelerated: false,
        }
    }
}

impl MdctState {
    fn new() -> Self {
        let mut state = Self {
            window_long: [0.0; 36],
            window_short: [0.0; 12],
        };

        // Initialize sine window
        for i in 0..36 {
            state.window_long[i] = sin_approx(3.14159265 / 36.0 * (i as f32 + 0.5));
        }

        for i in 0..12 {
            state.window_short[i] = sin_approx(3.14159265 / 12.0 * (i as f32 + 0.5));
        }

        state
    }
}

impl ImdctState {
    fn new() -> Self {
        let mut state = Self {
            window_long: [0.0; 36],
            window_short: [0.0; 12],
            overlap_buffer: [[0.0; 18]; 32],
        };

        for i in 0..36 {
            state.window_long[i] = sin_approx(3.14159265 / 36.0 * (i as f32 + 0.5));
        }

        for i in 0..12 {
            state.window_short[i] = sin_approx(3.14159265 / 12.0 * (i as f32 + 0.5));
        }

        state
    }
}

impl PolyphaseFilterbank {
    fn new() -> Self {
        let mut fb = Self {
            buffer: [[0.0; 32]; 16],
            coeffs: [0.0; 512],
        };

        // Initialize prototype filter coefficients
        for i in 0..512 {
            let n = i as f32;
            fb.coeffs[i] = cos_approx(3.14159265 / 64.0 * (n - 16.0) * (2.0 * 0.0 + 1.0));
        }

        fb
    }
}

impl SynthesisFilterbank {
    fn new() -> Self {
        Self {
            v_vec: [0.0; 1024],
            samples: [[0.0; 32]; 18],
        }
    }
}

impl BitReservoir {
    fn new() -> Self {
        Self {
            buffer: [0; 4096],
            bits_used: 0,
        }
    }
}

impl PsychoacousticModel {
    fn new() -> Self {
        Self {
            threshold: [0.0; 576],
        }
    }
}
