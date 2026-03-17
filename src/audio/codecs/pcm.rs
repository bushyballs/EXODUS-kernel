//! PCM (Pulse Code Modulation) codec implementation
//!
//! PCM is uncompressed linear audio data. This codec handles various
//! sample formats, byte orders, and conversions.

use super::{Decoder, Encoder};
use crate::audio::error::*;
use crate::audio::types::*;

/// PCM encoder state
pub struct PcmEncoder {
    sample_rate: u32,
    channels: u8,
    format: SampleFormat,
    initialized: bool,
}

/// PCM decoder state
pub struct PcmDecoder {
    sample_rate: u32,
    channels: u8,
    format: SampleFormat,
    initialized: bool,
}

impl PcmEncoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            format: SampleFormat::S16LE,
            initialized: false,
        }
    }

    pub fn set_format(&mut self, format: SampleFormat) {
        self.format = format;
    }

    fn convert_samples(&self, input: &[u8], output: &mut [u8]) -> usize {
        // Direct copy for PCM - no compression
        let len = input.len().min(output.len());
        output[..len].copy_from_slice(&input[..len]);
        len
    }
}

impl Encoder for PcmEncoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        // Set format based on bit depth
        self.format = match config.bit_depth {
            8 => SampleFormat::U8,
            16 => SampleFormat::S16LE,
            24 => SampleFormat::S24LE,
            32 => SampleFormat::S32LE,
            _ => return Err(AudioError::InvalidParameter),
        };

        self.initialized = true;
        Ok(())
    }

    fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        if output.len() < frame.len {
            return Err(AudioError::BufferTooSmall);
        }

        unsafe {
            let input = core::slice::from_raw_parts(frame.data, frame.len);
            let size = self.convert_samples(input, output);
            Ok(size)
        }
    }

    fn flush(&mut self, _output: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::PCM,
            can_encode: true,
            can_decode: false,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
                352800, 384000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround2_1,
                ChannelLayout::Surround3_0,
                ChannelLayout::Surround4_0,
                ChannelLayout::Surround5_0,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[],
            hardware_accelerated: false,
        }
    }
}

impl PcmDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            format: SampleFormat::S16LE,
            initialized: false,
        }
    }

    pub fn set_format(&mut self, format: SampleFormat) {
        self.format = format;
    }

    fn convert_samples(&self, input: &[u8], output: &mut [u8]) -> usize {
        // Direct copy for PCM - no decompression needed
        let len = input.len().min(output.len());
        output[..len].copy_from_slice(&input[..len]);
        len
    }

    fn convert_u8_to_s16(&self, input: &[u8], output: &mut [u8]) -> usize {
        let sample_count = input.len();
        let out_samples = unsafe {
            core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2)
        };

        for i in 0..sample_count.min(out_samples.len()) {
            // Convert u8 [0, 255] to i16 [-32768, 32767]
            out_samples[i] = (input[i] as i16 - 128) << 8;
        }

        sample_count * 2
    }

    fn convert_s24_to_s16(&self, input: &[u8], output: &mut [u8]) -> usize {
        let sample_count = input.len() / 3;
        let out_samples = unsafe {
            core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2)
        };

        for i in 0..sample_count.min(out_samples.len()) {
            // Extract 24-bit sample (little-endian)
            let s24 = ((input[i * 3 + 2] as i32) << 16)
                | ((input[i * 3 + 1] as i32) << 8)
                | (input[i * 3] as i32);

            // Sign extend and convert to 16-bit
            let s24_signed = if s24 & 0x800000 != 0 {
                s24 | 0xFF000000u32 as i32
            } else {
                s24
            };

            out_samples[i] = (s24_signed >> 8) as i16;
        }

        sample_count * 2
    }

    fn convert_s32_to_s16(&self, input: &[u8], output: &mut [u8]) -> usize {
        let in_samples =
            unsafe { core::slice::from_raw_parts(input.as_ptr() as *const i32, input.len() / 4) };
        let out_samples = unsafe {
            core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2)
        };

        let sample_count = in_samples.len().min(out_samples.len());

        for i in 0..sample_count {
            out_samples[i] = (in_samples[i] >> 16) as i16;
        }

        sample_count * 2
    }

    fn convert_f32_to_s16(&self, input: &[u8], output: &mut [u8]) -> usize {
        let in_samples =
            unsafe { core::slice::from_raw_parts(input.as_ptr() as *const f32, input.len() / 4) };
        let out_samples = unsafe {
            core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2)
        };

        let sample_count = in_samples.len().min(out_samples.len());

        for i in 0..sample_count {
            let sample = (in_samples[i] * 32768.0).clamp(-32768.0, 32767.0);
            out_samples[i] = sample as i16;
        }

        sample_count * 2
    }

    fn convert_f64_to_s16(&self, input: &[u8], output: &mut [u8]) -> usize {
        let in_samples =
            unsafe { core::slice::from_raw_parts(input.as_ptr() as *const f64, input.len() / 8) };
        let out_samples = unsafe {
            core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2)
        };

        let sample_count = in_samples.len().min(out_samples.len());

        for i in 0..sample_count {
            let sample = (in_samples[i] * 32768.0).clamp(-32768.0, 32767.0);
            out_samples[i] = sample as i16;
        }

        sample_count * 2
    }
}

impl Decoder for PcmDecoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        self.format = match config.bit_depth {
            8 => SampleFormat::U8,
            16 => SampleFormat::S16LE,
            24 => SampleFormat::S24LE,
            32 => SampleFormat::S32LE,
            _ => return Err(AudioError::InvalidParameter),
        };

        self.initialized = true;
        Ok(())
    }

    fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        unsafe {
            let input = core::slice::from_raw_parts(packet.data, packet.len);

            // Convert based on input format to S16LE output
            let size = match self.format {
                SampleFormat::U8 => self.convert_u8_to_s16(input, output),
                SampleFormat::S16LE => self.convert_samples(input, output),
                SampleFormat::S24LE => self.convert_s24_to_s16(input, output),
                SampleFormat::S32LE => self.convert_s32_to_s16(input, output),
                SampleFormat::F32LE => self.convert_f32_to_s16(input, output),
                SampleFormat::F64LE => self.convert_f64_to_s16(input, output),
            };

            Ok(size)
        }
    }

    fn reset(&mut self) {
        // No state to reset for PCM
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::PCM,
            can_encode: false,
            can_decode: true,
            supported_sample_rates: &[
                8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
                352800, 384000,
            ],
            supported_channel_layouts: &[
                ChannelLayout::Mono,
                ChannelLayout::Stereo,
                ChannelLayout::Surround2_1,
                ChannelLayout::Surround3_0,
                ChannelLayout::Surround4_0,
                ChannelLayout::Surround5_0,
                ChannelLayout::Surround5_1,
                ChannelLayout::Surround7_1,
            ],
            supported_bitrate_modes: &[],
            hardware_accelerated: false,
        }
    }
}
