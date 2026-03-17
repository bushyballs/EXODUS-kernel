//! Opus codec implementation
//!
//! Opus is a versatile audio codec for interactive speech and music transmission over the internet.
//! Combines SILK (for speech) and CELT (for general audio) codecs.

use super::{Decoder, Encoder};
use crate::audio::error::*;
use crate::audio::types::*;

/// Opus encoder state
pub struct OpusEncoder {
    sample_rate: u32,
    channels: u8,
    bitrate: u32,
    frame_size_ms: u8,
    application: OpusApplication,
    initialized: bool,
    silk_encoder: SilkEncoder,
    celt_encoder: CeltEncoder,
    range_coder: RangeCoder,
}

/// Opus decoder state
pub struct OpusDecoder {
    sample_rate: u32,
    channels: u8,
    initialized: bool,
    silk_decoder: SilkDecoder,
    celt_decoder: CeltDecoder,
    range_decoder: RangeDecoder,
    plc_state: PacketLossConcealment,
}

/// Opus application mode
#[derive(Debug, Clone, Copy)]
pub enum OpusApplication {
    VoIP,     // Optimize for voice
    Audio,    // Optimize for music
    LowDelay, // Optimize for low delay
}

/// SILK encoder for speech
struct SilkEncoder {
    lpc_coeffs: [f32; 16],
    pitch_lag: u16,
}

/// SILK decoder for speech
struct SilkDecoder {
    lpc_coeffs: [f32; 16],
    pitch_lag: u16,
    synthesis_buffer: [f32; 480],
}

/// CELT encoder for general audio
struct CeltEncoder {
    mdct_state: MdctState,
    bands: [f32; 21],
}

/// CELT decoder for general audio
struct CeltDecoder {
    imdct_state: ImdctState,
    bands: [f32; 21],
}

/// MDCT state
struct MdctState {
    overlap: [f32; 240],
}

/// IMDCT state
struct ImdctState {
    overlap: [f32; 240],
}

/// Range coder for entropy coding
struct RangeCoder {
    range: u32,
    low: u32,
}

/// Range decoder
struct RangeDecoder {
    range: u32,
    code: u32,
}

/// Packet loss concealment
struct PacketLossConcealment {
    last_frame: [f32; 960],
}

impl OpusEncoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            bitrate: 64000,
            frame_size_ms: 20,
            application: OpusApplication::Audio,
            initialized: false,
            silk_encoder: SilkEncoder::new(),
            celt_encoder: CeltEncoder::new(),
            range_coder: RangeCoder::new(),
        }
    }

    pub fn set_application(&mut self, app: OpusApplication) {
        self.application = app;
    }

    pub fn set_bitrate(&mut self, bitrate: u32) {
        self.bitrate = bitrate;
    }

    pub fn set_frame_size(&mut self, ms: u8) {
        self.frame_size_ms = ms;
    }

    fn encode_toc(&self, output: &mut [u8]) -> usize {
        if output.is_empty() {
            return 0;
        }

        // TOC byte: config (5 bits) + stereo (1 bit) + frame count (2 bits)
        let config = match self.application {
            OpusApplication::VoIP => 16,     // SILK-only
            OpusApplication::Audio => 20,    // CELT-only
            OpusApplication::LowDelay => 28, // Hybrid
        };

        let stereo_bit = if self.channels == 2 { 1 } else { 0 };
        output[0] = ((config << 3) | (stereo_bit << 2)) as u8;

        1
    }
}

impl Encoder for OpusEncoder {
    fn init(&mut self, config: &AudioConfig) -> Result<()> {
        self.sample_rate = config.sample_rate;
        self.channels = config.channels;

        // Opus only supports specific sample rates
        match self.sample_rate {
            8000 | 12000 | 16000 | 24000 | 48000 => {}
            _ => return Err(AudioError::InvalidSampleRate),
        }

        if self.channels > 2 {
            return Err(AudioError::InvalidChannels);
        }

        self.initialized = true;
        Ok(())
    }

    fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        if !self.initialized {
            return Err(AudioError::CodecInitFailed);
        }

        if output.len() < 1500 {
            return Err(AudioError::BufferTooSmall);
        }

        let toc_size = self.encode_toc(output);
        let mut pos = toc_size;

        // Convert input to f32
        let sample_count = frame.len / (self.channels as usize * 2);
        let mut float_samples = [0.0f32; 960];

        unsafe {
            let samples = core::slice::from_raw_parts(
                frame.data as *const i16,
                sample_count * self.channels as usize,
            );
            for i in 0..sample_count.min(960) {
                float_samples[i] = samples[i * self.channels as usize] as f32 / 32768.0;
            }
        }

        // Encode based on application mode
        match self.application {
            OpusApplication::VoIP => {
                // SILK encoding for speech
                self.silk_encoder.encode(
                    &float_samples[..sample_count],
                    &mut output[pos..],
                    &mut self.range_coder,
                );
                pos += 200; // Simplified
            }
            OpusApplication::Audio => {
                // CELT encoding for music
                self.celt_encoder.encode(
                    &float_samples[..sample_count],
                    &mut output[pos..],
                    &mut self.range_coder,
                );
                pos += 250; // Simplified
            }
            OpusApplication::LowDelay => {
                // Hybrid encoding
                self.celt_encoder.encode(
                    &float_samples[..sample_count],
                    &mut output[pos..],
                    &mut self.range_coder,
                );
                pos += 220; // Simplified
            }
        }

        Ok(pos)
    }

    fn flush(&mut self, _output: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::Opus,
            can_encode: true,
            can_decode: false,
            supported_sample_rates: &[8000, 12000, 16000, 24000, 48000],
            supported_channel_layouts: &[ChannelLayout::Mono, ChannelLayout::Stereo],
            supported_bitrate_modes: &[BitrateMode::VBR(64000), BitrateMode::CBR(64000)],
            hardware_accelerated: false,
        }
    }
}

impl OpusDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            initialized: false,
            silk_decoder: SilkDecoder::new(),
            celt_decoder: CeltDecoder::new(),
            range_decoder: RangeDecoder::new(),
            plc_state: PacketLossConcealment::new(),
        }
    }

    fn parse_toc(&mut self, data: &[u8]) -> Result<(OpusApplication, usize)> {
        if data.is_empty() {
            return Err(AudioError::InvalidFormat);
        }

        let config = (data[0] >> 3) & 0x1F;
        let stereo = ((data[0] >> 2) & 1) == 1;

        self.channels = if stereo { 2 } else { 1 };

        let app = if config < 12 {
            OpusApplication::VoIP
        } else if config < 16 {
            OpusApplication::Audio
        } else {
            OpusApplication::LowDelay
        };

        Ok((app, 1))
    }
}

impl Decoder for OpusDecoder {
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
            let (app, toc_size) = self.parse_toc(data)?;

            let mut float_samples = [0.0f32; 960];

            // Decode based on mode
            match app {
                OpusApplication::VoIP => {
                    self.silk_decoder.decode(
                        &data[toc_size..],
                        &mut float_samples,
                        &mut self.range_decoder,
                    );
                }
                OpusApplication::Audio => {
                    self.celt_decoder.decode(
                        &data[toc_size..],
                        &mut float_samples,
                        &mut self.range_decoder,
                    );
                }
                OpusApplication::LowDelay => {
                    self.celt_decoder.decode(
                        &data[toc_size..],
                        &mut float_samples,
                        &mut self.range_decoder,
                    );
                }
            }

            // Convert to i16 output
            let out_samples =
                core::slice::from_raw_parts_mut(output.as_mut_ptr() as *mut i16, output.len() / 2);
            for i in 0..960.min(out_samples.len()) {
                out_samples[i] = (float_samples[i] * 32768.0) as i16;
            }

            Ok(960 * 2 * self.channels as usize)
        }
    }

    fn reset(&mut self) {
        self.silk_decoder.synthesis_buffer = [0.0; 480];
        self.celt_decoder.imdct_state.overlap = [0.0; 240];
    }

    fn capabilities(&self) -> CodecCapabilities {
        CodecCapabilities {
            codec_id: CodecId::Opus,
            can_encode: false,
            can_decode: true,
            supported_sample_rates: &[8000, 12000, 16000, 24000, 48000],
            supported_channel_layouts: &[ChannelLayout::Mono, ChannelLayout::Stereo],
            supported_bitrate_modes: &[BitrateMode::VBR(64000), BitrateMode::CBR(64000)],
            hardware_accelerated: false,
        }
    }
}

impl SilkEncoder {
    fn new() -> Self {
        Self {
            lpc_coeffs: [0.0; 16],
            pitch_lag: 0,
        }
    }

    fn encode(&mut self, samples: &[f32], output: &mut [u8], _coder: &mut RangeCoder) {
        // Simplified SILK encoding - LPC analysis and pitch estimation
        for i in 0..output.len().min(200) {
            output[i] = (samples[i % samples.len()] * 127.0) as u8;
        }
    }
}

impl SilkDecoder {
    fn new() -> Self {
        Self {
            lpc_coeffs: [0.0; 16],
            pitch_lag: 0,
            synthesis_buffer: [0.0; 480],
        }
    }

    fn decode(&mut self, data: &[u8], samples: &mut [f32], _decoder: &mut RangeDecoder) {
        for i in 0..samples.len().min(data.len()) {
            samples[i] = data[i] as f32 / 127.0 - 1.0;
        }
    }
}

impl CeltEncoder {
    fn new() -> Self {
        Self {
            mdct_state: MdctState::new(),
            bands: [0.0; 21],
        }
    }

    fn encode(&mut self, samples: &[f32], output: &mut [u8], _coder: &mut RangeCoder) {
        // Simplified CELT encoding - band analysis
        for i in 0..output.len().min(250) {
            output[i] = (samples[i % samples.len()] * 127.0 + 128.0) as u8;
        }
    }
}

impl CeltDecoder {
    fn new() -> Self {
        Self {
            imdct_state: ImdctState::new(),
            bands: [0.0; 21],
        }
    }

    fn decode(&mut self, data: &[u8], samples: &mut [f32], _decoder: &mut RangeDecoder) {
        for i in 0..samples.len().min(data.len()) {
            samples[i] = (data[i] as f32 - 128.0) / 127.0;
        }
    }
}

impl MdctState {
    fn new() -> Self {
        Self {
            overlap: [0.0; 240],
        }
    }
}

impl ImdctState {
    fn new() -> Self {
        Self {
            overlap: [0.0; 240],
        }
    }
}

impl RangeCoder {
    fn new() -> Self {
        Self {
            range: 0xFFFFFFFF,
            low: 0,
        }
    }
}

impl RangeDecoder {
    fn new() -> Self {
        Self {
            range: 0xFFFFFFFF,
            code: 0,
        }
    }
}

impl PacketLossConcealment {
    fn new() -> Self {
        Self {
            last_frame: [0.0; 960],
        }
    }
}
