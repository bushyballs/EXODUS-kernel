// video_codec/mod.rs - Video codec framework for Genesis OS
// Supports H.264, H.265, VP9, AV1 with hardware acceleration

#![no_std]

pub mod types;
pub mod h264;
pub mod h265;
pub mod vp9;
pub mod av1;
pub mod hardware;
pub mod decoder;
pub mod encoder;
pub mod buffer;
pub mod bitstream;
pub mod transform;
pub mod motion;
pub mod entropy;
pub mod filter;

use crate::video_codec::{
    types::*,
    decoder::VideoDecoder,
    encoder::VideoEncoder,
    hardware::HardwareAccelerator,
};

/// Main video codec manager
pub struct CodecManager {
    h264_decoder: Option<h264::H264Decoder>,
    h264_encoder: Option<h264::H264Encoder>,
    h265_decoder: Option<h265::H265Decoder>,
    h265_encoder: Option<h265::H265Encoder>,
    vp9_decoder: Option<vp9::VP9Decoder>,
    vp9_encoder: Option<vp9::VP9Encoder>,
    av1_decoder: Option<av1::AV1Decoder>,
    av1_encoder: Option<av1::AV1Encoder>,
    hw_accel: HardwareAccelerator,
}

impl CodecManager {
    /// Initialize the codec manager
    pub fn new() -> Self {
        let hw_accel = HardwareAccelerator::new();

        Self {
            h264_decoder: None,
            h264_encoder: None,
            h265_decoder: None,
            h265_encoder: None,
            vp9_decoder: None,
            vp9_encoder: None,
            av1_decoder: None,
            av1_encoder: None,
            hw_accel,
        }
    }

    /// Initialize a decoder for the given codec
    pub fn init_decoder(&mut self, codec: CodecType) -> Result<(), CodecError> {
        match codec {
            CodecType::H264 => {
                self.h264_decoder = Some(h264::H264Decoder::new(&self.hw_accel)?);
            }
            CodecType::H265 => {
                self.h265_decoder = Some(h265::H265Decoder::new(&self.hw_accel)?);
            }
            CodecType::VP9 => {
                self.vp9_decoder = Some(vp9::VP9Decoder::new(&self.hw_accel)?);
            }
            CodecType::AV1 => {
                self.av1_decoder = Some(av1::AV1Decoder::new(&self.hw_accel)?);
            }
        }
        Ok(())
    }

    /// Initialize an encoder for the given codec
    pub fn init_encoder(&mut self, codec: CodecType, config: EncoderConfig) -> Result<(), CodecError> {
        match codec {
            CodecType::H264 => {
                self.h264_encoder = Some(h264::H264Encoder::new(&self.hw_accel, config)?);
            }
            CodecType::H265 => {
                self.h265_encoder = Some(h265::H265Encoder::new(&self.hw_accel, config)?);
            }
            CodecType::VP9 => {
                self.vp9_encoder = Some(vp9::VP9Encoder::new(&self.hw_accel, config)?);
            }
            CodecType::AV1 => {
                self.av1_encoder = Some(av1::AV1Encoder::new(&self.hw_accel, config)?);
            }
        }
        Ok(())
    }

    /// Decode a video frame
    pub fn decode(&mut self, codec: CodecType, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        match codec {
            CodecType::H264 => {
                if let Some(decoder) = &mut self.h264_decoder {
                    decoder.decode(data, output)
                } else {
                    Err(CodecError::DecoderNotInitialized)
                }
            }
            CodecType::H265 => {
                if let Some(decoder) = &mut self.h265_decoder {
                    decoder.decode(data, output)
                } else {
                    Err(CodecError::DecoderNotInitialized)
                }
            }
            CodecType::VP9 => {
                if let Some(decoder) = &mut self.vp9_decoder {
                    decoder.decode(data, output)
                } else {
                    Err(CodecError::DecoderNotInitialized)
                }
            }
            CodecType::AV1 => {
                if let Some(decoder) = &mut self.av1_decoder {
                    decoder.decode(data, output)
                } else {
                    Err(CodecError::DecoderNotInitialized)
                }
            }
        }
    }

    /// Encode a video frame
    pub fn encode(&mut self, codec: CodecType, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        match codec {
            CodecType::H264 => {
                if let Some(encoder) = &mut self.h264_encoder {
                    encoder.encode(frame, output)
                } else {
                    Err(CodecError::EncoderNotInitialized)
                }
            }
            CodecType::H265 => {
                if let Some(encoder) = &mut self.h265_encoder {
                    encoder.encode(frame, output)
                } else {
                    Err(CodecError::EncoderNotInitialized)
                }
            }
            CodecType::VP9 => {
                if let Some(encoder) = &mut self.vp9_encoder {
                    encoder.encode(frame, output)
                } else {
                    Err(CodecError::EncoderNotInitialized)
                }
            }
            CodecType::AV1 => {
                if let Some(encoder) = &mut self.av1_encoder {
                    encoder.encode(frame, output)
                } else {
                    Err(CodecError::EncoderNotInitialized)
                }
            }
        }
    }

    /// Get hardware acceleration capabilities
    pub fn get_hw_capabilities(&self) -> &HardwareCapabilities {
        self.hw_accel.get_capabilities()
    }

    /// Flush all codec buffers
    pub fn flush(&mut self, codec: CodecType) -> Result<(), CodecError> {
        match codec {
            CodecType::H264 => {
                if let Some(decoder) = &mut self.h264_decoder {
                    decoder.flush();
                }
                if let Some(encoder) = &mut self.h264_encoder {
                    encoder.flush();
                }
            }
            CodecType::H265 => {
                if let Some(decoder) = &mut self.h265_decoder {
                    decoder.flush();
                }
                if let Some(encoder) = &mut self.h265_encoder {
                    encoder.flush();
                }
            }
            CodecType::VP9 => {
                if let Some(decoder) = &mut self.vp9_decoder {
                    decoder.flush();
                }
                if let Some(encoder) = &mut self.vp9_encoder {
                    encoder.flush();
                }
            }
            CodecType::AV1 => {
                if let Some(decoder) = &mut self.av1_decoder {
                    decoder.flush();
                }
                if let Some(encoder) = &mut self.av1_encoder {
                    encoder.flush();
                }
            }
        }
        Ok(())
    }
}
