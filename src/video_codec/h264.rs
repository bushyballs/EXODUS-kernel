// video_codec/h264.rs - H.264/AVC codec implementation

#![no_std]

use crate::video_codec::types::*;
use crate::video_codec::hardware::{HardwareAccelerator, HWDecoderHandle, HWEncoderHandle};
use crate::video_codec::decoder::VideoDecoder;
use crate::video_codec::encoder::VideoEncoder;
use crate::video_codec::bitstream::BitstreamParser;
use crate::video_codec::transform::Transform;
use crate::video_codec::motion::MotionCompensation;
use crate::video_codec::entropy::CAVLC;
use crate::video_codec::filter::DeblockingFilter;

/// H.264 decoder
pub struct H264Decoder {
    sps: Option<SequenceParameterSet>,
    pps: Option<PictureParameterSet>,
    reference_frames: [Option<Frame>; 16],
    hw_decoder: Option<HWDecoderHandle>,
    use_hardware: bool,
}

impl H264Decoder {
    pub fn new(hw_accel: &HardwareAccelerator) -> Result<Self, CodecError> {
        let use_hardware = hw_accel.has_hw_decode(CodecType::H264);
        let hw_decoder = if use_hardware {
            hw_accel.init_hw_decoder(CodecType::H264).ok()
        } else {
            None
        };

        Ok(Self {
            sps: None,
            pps: None,
            reference_frames: Default::default(),
            hw_decoder,
            use_hardware,
        })
    }

    /// Parse NAL unit
    fn parse_nal(&mut self, data: &[u8]) -> Result<NalUnitType, CodecError> {
        if data.len() < 4 {
            return Err(CodecError::InvalidBitstream);
        }

        // Skip start code (0x00 0x00 0x00 0x01 or 0x00 0x00 0x01)
        let offset = if data[0] == 0 && data[1] == 0 && data[2] == 0 && data[3] == 1 {
            4
        } else if data[0] == 0 && data[1] == 0 && data[2] == 1 {
            3
        } else {
            return Err(CodecError::InvalidBitstream);
        };

        let nal_header = data[offset];
        let nal_type = nal_header & 0x1F;

        match nal_type {
            1 => Ok(NalUnitType::H264NonIDR),
            5 => Ok(NalUnitType::H264IDR),
            6 => Ok(NalUnitType::H264SEI),
            7 => {
                self.parse_sps(&data[offset..])?;
                Ok(NalUnitType::H264SPS)
            }
            8 => {
                self.parse_pps(&data[offset..])?;
                Ok(NalUnitType::H264PPS)
            }
            9 => Ok(NalUnitType::H264AUD),
            _ => Err(CodecError::UnsupportedFeature),
        }
    }

    /// Parse Sequence Parameter Set
    fn parse_sps(&mut self, data: &[u8]) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        let _profile_idc = parser.read_bits(8)?;
        let _constraint_flags = parser.read_bits(8)?;
        let _level_idc = parser.read_bits(8)?;
        let _seq_parameter_set_id = parser.read_ue()?;

        let sps = SequenceParameterSet {
            profile_idc: _profile_idc as u8,
            level_idc: _level_idc as u8,
            width: 0,  // Would parse from SPS
            height: 0,
        };

        self.sps = Some(sps);
        Ok(())
    }

    /// Parse Picture Parameter Set
    fn parse_pps(&mut self, data: &[u8]) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        let _pic_parameter_set_id = parser.read_ue()?;
        let _seq_parameter_set_id = parser.read_ue()?;

        let pps = PictureParameterSet {
            entropy_coding_mode: 0,
            num_ref_idx_l0: 1,
        };

        self.pps = Some(pps);
        Ok(())
    }

    /// Decode slice
    fn decode_slice(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        // Parse slice header
        let _first_mb_in_slice = parser.read_ue()?;
        let slice_type = parser.read_ue()?;
        let _pic_parameter_set_id = parser.read_ue()?;

        // Decode macroblocks
        let mb_width = (output.width + 15) / 16;
        let mb_height = (output.height + 15) / 16;

        for mb_y in 0..mb_height {
            for mb_x in 0..mb_width {
                self.decode_macroblock(&mut parser, mb_x, mb_y, slice_type as u32, output)?;
            }
        }

        // Apply deblocking filter
        DeblockingFilter::apply(output);

        Ok(())
    }

    /// Decode single macroblock
    fn decode_macroblock(
        &mut self,
        parser: &mut BitstreamParser,
        mb_x: u32,
        mb_y: u32,
        slice_type: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        let mb_type = parser.read_ue()?;

        match mb_type {
            0 => self.decode_intra_mb(parser, mb_x, mb_y, output)?,
            _ => self.decode_inter_mb(parser, mb_x, mb_y, slice_type, output)?,
        }

        Ok(())
    }

    /// Decode intra macroblock
    fn decode_intra_mb(
        &mut self,
        parser: &mut BitstreamParser,
        mb_x: u32,
        mb_y: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Parse prediction mode
        let _intra_mode = parser.read_ue()?;

        // Decode residual
        let mut coeffs = [0i16; 256];
        CAVLC::decode_block(parser, &mut coeffs[0..16])?;

        // Inverse transform
        let mut residual = [0i16; 256];
        Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]);

        // Add to prediction and write to output
        self.add_residual_to_frame(mb_x, mb_y, &residual, output);

        Ok(())
    }

    /// Decode inter macroblock
    fn decode_inter_mb(
        &mut self,
        parser: &mut BitstreamParser,
        mb_x: u32,
        mb_y: u32,
        _slice_type: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Parse motion vectors
        let mvd_x = parser.read_se()?;
        let mvd_y = parser.read_se()?;

        let mv = MotionVector {
            x: mvd_x as i16,
            y: mvd_y as i16,
        };

        // Motion compensation from reference frame
        if let Some(ref_frame) = &self.reference_frames[0] {
            MotionCompensation::compensate_16x16(ref_frame, output, mb_x, mb_y, mv);
        }

        // Decode residual
        let mut coeffs = [0i16; 256];
        CAVLC::decode_block(parser, &mut coeffs[0..16])?;

        let mut residual = [0i16; 256];
        Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]);

        self.add_residual_to_frame(mb_x, mb_y, &residual, output);

        Ok(())
    }

    fn add_residual_to_frame(&self, mb_x: u32, mb_y: u32, residual: &[i16], output: &mut Frame) {
        // Add residual to output frame
        let x_offset = mb_x * 16;
        let y_offset = mb_y * 16;

        unsafe {
            for y in 0..16 {
                for x in 0..16 {
                    if x_offset + x < output.width && y_offset + y < output.height {
                        let pixel_offset = ((y_offset + y) * output.y_stride + x_offset + x) as isize;
                        let current = *output.y_plane.offset(pixel_offset) as i16;
                        let new_val = (current + residual[y as usize * 16 + x as usize]).clamp(0, 255);
                        *output.y_plane.offset(pixel_offset) = new_val as u8;
                    }
                }
            }
        }
    }
}

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        // Use hardware decoder if available
        if self.use_hardware {
            if let Some(hw_decoder) = &self.hw_decoder {
                return hw_decoder.decode(data, output);
            }
        }

        // Software decode
        let nal_type = self.parse_nal(data)?;

        match nal_type {
            NalUnitType::H264IDR | NalUnitType::H264NonIDR => {
                self.decode_slice(data, output)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn flush(&mut self) {
        // Clear reference frames
        for frame in &mut self.reference_frames {
            *frame = None;
        }
    }
}

/// H.264 encoder
pub struct H264Encoder {
    config: EncoderConfig,
    sps: SequenceParameterSet,
    pps: PictureParameterSet,
    reference_frames: [Option<Frame>; 16],
    frame_num: u32,
    hw_encoder: Option<HWEncoderHandle>,
    use_hardware: bool,
}

impl H264Encoder {
    pub fn new(hw_accel: &HardwareAccelerator, config: EncoderConfig) -> Result<Self, CodecError> {
        let use_hardware = config.use_hardware && hw_accel.has_hw_encode(CodecType::H264);
        let hw_encoder = if use_hardware {
            hw_accel.init_hw_encoder(CodecType::H264, &config).ok()
        } else {
            None
        };

        let sps = SequenceParameterSet {
            profile_idc: 100,  // High profile
            level_idc: 41,     // Level 4.1
            width: config.width,
            height: config.height,
        };

        let pps = PictureParameterSet {
            entropy_coding_mode: 1,  // CABAC
            num_ref_idx_l0: 1,
        };

        Ok(Self {
            config,
            sps,
            pps,
            reference_frames: Default::default(),
            frame_num: 0,
            hw_encoder,
            use_hardware,
        })
    }

    /// Write NAL unit with start code
    fn write_nal(&self, output: &mut [u8], offset: usize, nal_type: u8, payload: &[u8]) -> usize {
        // Write start code
        output[offset] = 0x00;
        output[offset + 1] = 0x00;
        output[offset + 2] = 0x00;
        output[offset + 3] = 0x01;

        // Write NAL header
        output[offset + 4] = nal_type;

        // Write payload
        let len = payload.len().min(output.len() - offset - 5);
        output[offset + 5..offset + 5 + len].copy_from_slice(&payload[..len]);

        5 + len
    }
}

impl VideoEncoder for H264Encoder {
    fn encode(&mut self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        // Use hardware encoder if available
        if self.use_hardware {
            if let Some(hw_encoder) = &self.hw_encoder {
                return hw_encoder.encode(frame, output);
            }
        }

        // Software encode
        let mut offset = 0;

        // Write SPS (on first frame or IDR)
        if self.frame_num == 0 || self.frame_num % self.config.gop_size == 0 {
            let sps_data = [0u8; 32]; // Simplified
            offset += self.write_nal(output, offset, 0x67, &sps_data);

            let pps_data = [0u8; 16]; // Simplified
            offset += self.write_nal(output, offset, 0x68, &pps_data);
        }

        // Encode slice
        let slice_data = [0u8; 1024]; // Placeholder
        let nal_type = if self.frame_num % self.config.gop_size == 0 {
            0x65  // IDR
        } else {
            0x61  // Non-IDR
        };
        offset += self.write_nal(output, offset, nal_type, &slice_data);

        self.frame_num = self.frame_num.saturating_add(1);

        Ok(offset)
    }

    fn flush(&mut self) {
        self.frame_num = 0;
    }
}

// Internal structures

#[derive(Clone, Copy)]
struct SequenceParameterSet {
    profile_idc: u8,
    level_idc: u8,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct PictureParameterSet {
    entropy_coding_mode: u8,
    num_ref_idx_l0: u32,
}
