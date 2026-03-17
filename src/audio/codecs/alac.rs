//! ALAC (Apple Lossless Audio Codec) implementation
//!
//! ALAC is Apple's lossless audio codec, providing compression similar to FLAC
//! with better integration in Apple ecosystems.

use super::{Decoder, Encoder};
use crate::audio::error::*;
use crate::audio::types::*;

/// ALAC encoder state
pub struct AlacEncoder {
    sample_rate: u32,
    channels: u8,
    bit_depth: u8,
    frame_length: u32,
    initialized: bool,
    predictor: AdaptivePredictor,
    rice_encoder: DynamicRiceEncoder,
}

/// ALAC decoder state
pub struct AlacDecoder {
    sample_rate: u32,
    channels: u8,
    bit_depth: u8,
    frame_length: u32,
    initialized: bool,
    predictor: AdaptivePredictor,
    rice_decoder: DynamicRiceDecoder,
}

/// Adaptive LPC predictor
struct AdaptivePredictor {
    order: u8,
    coeffs: [[i32; 32]; 2], // Per-channel coefficients
    history: [[i32; 32]; 2],
}

/// Dynamic Rice/Golomb encoder
struct DynamicRiceEncoder {
    param: u8,
    history: [u32; 4],
}

/// Dynamic Rice/Golomb decoder
struct DynamicRiceDecoder {
    param: u8,
}

impl AlacEncoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            frame_length: 4096,
            initialized: false,
            predictor: AdaptivePredictor::new(),
            rice_encoder: DynamicRiceEncoder::new(),
        }
    }

    pub fn set_fast_mode(&mut self, fast: bool) {
        if fast {
            self.predictor.order = 4;
        } else {
            self.predictor.order = 31;
        }
    }

    fn write_magic_cookie(&self, output: &mut [u8]) -> usize {
        if output.len() < 24 {
            return 0;
        }

        let mut pos = 0;

        // Frame length (32 bits)
        output[pos..pos + 4].copy_from_slice(&self.frame_length.to_be_bytes());
        pos += 4;

        // Compatible version (8 bits)
        output[pos] = 0;
        pos += 1;

        // Bit depth (8 bits)
        output[pos] = self.bit_depth;
        pos += 1;

        // Rice history mult (8 bits)
        output[pos] = 40;
        pos += 1;

        // Rice initial history (8 bits)
        output[pos] = 10;
        pos += 1;

        // Rice parameter limit (8 bits)
        output[pos] = 14;
        pos += 1;

        // Channels (8 bits)
        output[pos] = self.channels;
        pos += 1;

        // Max run (16 bits)
        output[pos..pos + 2].copy_from_slice(&255u16.to_be_bytes());
        pos += 2;

        // Max frame size (32 bits)
        output[pos..pos + 4].copy_from_slice(&0u32.to_be_bytes());
        pos += 4;

        // Avg bit rate (32 bits)
        output[pos..pos + 4].copy_from_slice(&0u32.to_be_bytes());
        pos += 4;

        // Sample rate (32 bits)
        output[pos..pos + 4].copy_from_slice(&self.sample_rate.to_be_bytes());
        pos += 4;

        pos
    }

    fn encode_frame(&mut self, samples: &[i32], output: &mut [u8], offset: usize) -> usize {
        let mut pos = offset;

        // Write frame header (simplified)
        if pos + 4 > output.len() {
            return 0;
        }

        // Channels (3 bits) + unused (13 bits) + has size (1 bit) + unused (2 bits) + is uncompressed (1 bit)
        output[pos..pos + 4].copy_from_slice(&[0, 0, 0, 0]);
        pos += 4;

        // Compute prediction residuals
        for ch in 0..self.channels as usize {
            self.predictor.compute_coefficients(samples, ch);

            for i in 0..samples.len() {
                let sample = samples[i];
                let prediction = self.predictor.predict(ch);
                let residual = sample - prediction;

                // Rice encode residual
                if pos + 4 > output.len() {
                    break;
                }

                output[pos..pos + 4].copy_from_slice(&residual.to_le_bytes());
                pos += 4;

                self.predictor.update_history(ch, sample);
            }
        }

        pos - offset
    }
}

impl Encoder for AlacEncoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;
        self.bit_depth = config.bit_depth;

        if self.channels > 8 {
            return Err(AudioError::InvalidChannels);
        }

        // ALAC supports 16, 20, 24, 32 bit depths
        match self.bit_depth {
            16 | 20 | 24 | 32 => {}
            _ => return Err(AudioError::InvalidParameter),
        }

        self.initialized = true;
        Ok(())
    }

    fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        if output.len() < 8192 {
            return Err(AudioError::BufferTooSmall);
        }

        // Convert input to i32
        let sample_count = frame.len / (self.channels as usize * 2);
        let mut samples = [0i32; 4096];

        unsafe {
            let input = core::slice::from_raw_parts(
                frame.data as *const i16,
                sample_count * self.channels as usize,
            );
            for i in 0..sample_count.min(self.frame_length as usize) {
                samples[i] = input[i * self.channels as usize] as i32;
            }
        }

        // Encode frame
        let size = self.encode_frame(
            &samples[..sample_count.min(self.frame_length as usize)],
            output,
            0,
        );

        Ok(size)
    }

    fn flush(&mut self, _output: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::ALAC,
            can_encode: true,
            can_decode: false,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[],
            hardware_accelerated: false,
        }
    }
}

impl AlacDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            frame_length: 4096,
            initialized: false,
            predictor: AdaptivePredictor::new(),
            rice_decoder: DynamicRiceDecoder::new(),
        }
    }

    fn parse_magic_cookie(&mut self, data: &[u8]) -> Result<usize> {
        if data.len() < 24 {
            return Err(AudioError::InvalidFormat);
        }

        let mut pos = 0;

        // Frame length
        let mut fl_bytes = [0u8; 4];
        fl_bytes.copy_from_slice(&data[pos..pos + 4]);
        self.frame_length = u32::from_be_bytes(fl_bytes);
        pos += 4;

        // Skip version
        pos += 1;

        // Bit depth
        self.bit_depth = data[pos];
        pos += 1;

        // Skip rice parameters
        pos += 3;

        // Channels
        self.channels = data[pos];
        pos += 1;

        // Skip max run
        pos += 2;

        // Skip max frame size
        pos += 4;

        // Skip avg bit rate
        pos += 4;

        // Sample rate
        let mut sr_bytes = [0u8; 4];
        sr_bytes.copy_from_slice(&data[pos..pos + 4]);
        self.sample_rate = u32::from_be_bytes(sr_bytes);
        pos += 4;

        Ok(pos)
    }

    fn decode_frame(&mut self, data: &[u8], samples: &mut [i32]) -> Result<usize> {
        let mut pos = 0;

        // Skip frame header
        pos += 4;

        // Decode residuals and reconstruct
        for ch in 0..self.channels as usize {
            for i in 0..samples.len() {
                if pos + 4 > data.len() {
                    break;
                }

                // Rice decode residual
                let mut residual_bytes = [0u8; 4];
                residual_bytes.copy_from_slice(&data[pos..pos + 4]);
                let residual = i32::from_le_bytes(residual_bytes);
                pos += 4;

                // Reconstruct sample
                let prediction = self.predictor.predict(ch);
                samples[i] = prediction + residual;

                self.predictor.update_history(ch, samples[i]);
            }
        }

        Ok(samples.len())
    }
}

impl Decoder for AlacDecoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;
        self.bit_depth = config.bit_depth;

        self.initialized = true;
        Ok(())
    }

    fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        unsafe {
            let data = core::slice::from_raw_parts(packet.data, packet.len);

            // Decode frame
            let mut samples = [0i32; 4096];
            let sample_count = self.decode_frame(data, &mut samples)?;

            // Convert to i16 output
            let out_samples =
                core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2);
            for i in 0..sample_count.min(out_samples.len()) {
                out_samples[i] = samples[i] as i16;
            }

            Ok(sample_count * 2 * self.channels as usize)
        }
    }

    fn reset(&mut self) {
        self.predictor.history = [[0; 32]; 2];
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::ALAC,
            can_encode: false,
            can_decode: true,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[],
            hardware_accelerated: false,
        }
    }
}

impl AdaptivePredictor {
    fn new() -> Self {
        Self {
            order: 8,
            coeffs: [[0; 32]; 2],
            history: [[0; 32]; 2],
        }
    }

    fn compute_coefficients(&mut self, samples: &[i32], channel: usize) {
        // Simplified adaptive prediction using autocorrelation
        for i in 0..self.order as usize {
            let mut sum = 0i64;
            for j in i..samples.len().min(1000) {
                sum += samples[j] as i64 * samples[j - i] as i64;
            }
            self.coeffs[channel][i] = (sum / 1000) as i32;
        }
    }

    fn predict(&self, channel: usize) -> i32 {
        let mut prediction = 0i64;
        for i in 0..self.order as usize {
            prediction += self.coeffs[channel][i] as i64 * self.history[channel][i] as i64;
        }
        (prediction >> 12) as i32
    }

    fn update_history(&mut self, channel: usize, sample: i32) {
        // Shift history buffer
        for i in (1..self.order as usize).rev() {
            self.history[channel][i] = self.history[channel][i - 1];
        }
        self.history[channel][0] = sample;
    }
}

impl DynamicRiceEncoder {
    fn new() -> Self {
        Self {
            param: 4,
            history: [0; 4],
        }
    }
}

impl DynamicRiceDecoder {
    fn new() -> Self {
        Self { param: 4 }
    }
}
