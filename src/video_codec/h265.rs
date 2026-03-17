// video_codec/h265.rs - H.265/HEVC codec implementation

#![no_std]

use crate::video_codec::types::*;
use crate::video_codec::hardware::{HardwareAccelerator, HWDecoderHandle, HWEncoderHandle};
use crate::video_codec::decoder::VideoDecoder;
use crate::video_codec::encoder::VideoEncoder;
use crate::video_codec::bitstream::BitstreamParser;
use crate::video_codec::transform::Transform;
use crate::video_codec::motion::MotionCompensation;
use crate::video_codec::entropy::CABAC;
use crate::video_codec::filter::SAOFilter;

/// H.265/HEVC decoder
pub struct H265Decoder {
    vps: Option<VideoParameterSet>,
    sps: Option<SequenceParameterSet>,
    pps: Option<PictureParameterSet>,
    reference_frames: [Option<Frame>; 16],
    hw_decoder: Option<HWDecoderHandle>,
    use_hardware: bool,
}

impl H265Decoder {
    pub fn new(hw_accel: &HardwareAccelerator) -> Result<Self, CodecError> {
        let use_hardware = hw_accel.has_hw_decode(CodecType::H265);
        let hw_decoder = if use_hardware {
            hw_accel.init_hw_decoder(CodecType::H265).ok()
        } else {
            None
        };

        Ok(Self {
            vps: None,
            sps: None,
            pps: None,
            reference_frames: Default::default(),
            hw_decoder,
            use_hardware,
        })
    }

    /// Parse NAL unit
    fn parse_nal(&mut self, data: &[u8]) -> Result<NalUnitType, CodecError> {
        if data.len() < 5 {
            return Err(CodecError::InvalidBitstream);
        }

        // Skip start code
        let offset = if data[0] == 0 && data[1] == 0 && data[2] == 0 && data[3] == 1 {
            4
        } else if data[0] == 0 && data[1] == 0 && data[2] == 1 {
            3
        } else {
            return Err(CodecError::InvalidBitstream);
        };

        // H.265 NAL header is 2 bytes
        let nal_header = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
        let nal_type = (nal_header >> 9) & 0x3F;

        match nal_type {
            0 | 1 => Ok(NalUnitType::H265TrailN),
            19 => Ok(NalUnitType::H265IDR),
            32 => {
                self.parse_vps(&data[offset..])?;
                Ok(NalUnitType::H265VPS)
            }
            33 => {
                self.parse_sps(&data[offset..])?;
                Ok(NalUnitType::H265SPS)
            }
            34 => {
                self.parse_pps(&data[offset..])?;
                Ok(NalUnitType::H265PPS)
            }
            39 => Ok(NalUnitType::H265SEI),
            _ => Err(CodecError::UnsupportedFeature),
        }
    }

    /// Parse Video Parameter Set
    fn parse_vps(&mut self, data: &[u8]) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        let _vps_id = parser.read_bits(4)?;

        let vps = VideoParameterSet {
            max_layers: 1,
            max_sub_layers: 1,
        };

        self.vps = Some(vps);
        Ok(())
    }

    /// Parse Sequence Parameter Set
    fn parse_sps(&mut self, data: &[u8]) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        let _sps_video_parameter_set_id = parser.read_bits(4)?;
        let _sps_max_sub_layers = parser.read_bits(3)?;

        let sps = SequenceParameterSet {
            width: 0,
            height: 0,
            bit_depth: 8,
        };

        self.sps = Some(sps);
        Ok(())
    }

    /// Parse Picture Parameter Set
    fn parse_pps(&mut self, data: &[u8]) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        let _pps_pic_parameter_set_id = parser.read_ue()?;
        let _pps_seq_parameter_set_id = parser.read_ue()?;

        let pps = PictureParameterSet {
            num_ref_idx_l0: 1,
            init_qp: 26,
        };

        self.pps = Some(pps);
        Ok(())
    }

    /// Decode slice
    fn decode_slice(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        let mut parser = BitstreamParser::new(data);

        // Parse slice header
        let _first_slice_segment_in_pic_flag = parser.read_bits(1)?;
        let slice_type = parser.read_ue()?;

        // Decode CTUs (Coding Tree Units)
        let ctu_width = (output.width + 63) / 64;
        let ctu_height = (output.height + 63) / 64;

        for ctu_y in 0..ctu_height {
            for ctu_x in 0..ctu_width {
                self.decode_ctu(&mut parser, ctu_x, ctu_y, slice_type as u32, output)?;
            }
        }

        // Apply SAO (Sample Adaptive Offset) filter
        SAOFilter::apply(output);

        Ok(())
    }

    /// Decode Coding Tree Unit (64x64 in HEVC)
    fn decode_ctu(
        &mut self,
        parser: &mut BitstreamParser,
        ctu_x: u32,
        ctu_y: u32,
        _slice_type: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Quadtree split decision
        let split_cu_flag = parser.read_bits(1)?;

        if split_cu_flag == 1 {
            // Recursively decode 32x32 CUs
            self.decode_cu(parser, ctu_x * 64, ctu_y * 64, 32, output)?;
        } else {
            // Decode single 64x64 CU
            self.decode_cu(parser, ctu_x * 64, ctu_y * 64, 64, output)?;
        }

        Ok(())
    }

    /// Decode Coding Unit
    fn decode_cu(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        let cu_type = parser.read_bits(1)?;

        if cu_type == 0 {
            // Intra prediction
            self.decode_intra_cu(parser, x, y, size, output)?;
        } else {
            // Inter prediction
            self.decode_inter_cu(parser, x, y, size, output)?;
        }

        Ok(())
    }

    /// Decode intra CU
    fn decode_intra_cu(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        let _intra_mode = parser.read_bits(6)?;

        // Decode transform coefficients using CABAC
        let mut coeffs = [0i16; 4096];
        CABAC::decode_block(parser, &mut coeffs[0..(size * size) as usize])?;

        // Inverse transform (supports multiple sizes in HEVC)
        let mut residual = [0i16; 4096];
        match size {
            4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
            8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
            16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
            32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
            _ => return Err(CodecError::InvalidParameter),
        }

        // Add residual to frame
        self.add_residual_to_frame(x, y, size, &residual, output);

        Ok(())
    }

    /// Decode inter CU
    fn decode_inter_cu(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Parse motion vectors
        let mvd_x = parser.read_se()?;
        let mvd_y = parser.read_se()?;

        let mv = MotionVector {
            x: mvd_x as i16,
            y: mvd_y as i16,
        };

        // Motion compensation
        if let Some(ref_frame) = &self.reference_frames[0] {
            MotionCompensation::compensate(ref_frame, output, x, y, size, mv);
        }

        // Decode residual
        let mut coeffs = [0i16; 4096];
        CABAC::decode_block(parser, &mut coeffs[0..(size * size) as usize])?;

        let mut residual = [0i16; 4096];
        match size {
            4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
            8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
            16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
            32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
            _ => return Err(CodecError::InvalidParameter),
        }

        self.add_residual_to_frame(x, y, size, &residual, output);

        Ok(())
    }

    fn add_residual_to_frame(&self, x: u32, y: u32, size: u32, residual: &[i16], output: &mut Frame) {
        unsafe {
            for dy in 0..size {
                for dx in 0..size {
                    if x + dx < output.width && y + dy < output.height {
                        let pixel_offset = ((y + dy) * output.y_stride + x + dx) as isize;
                        let current = *output.y_plane.offset(pixel_offset) as i16;
                        let new_val = (current + residual[(dy * size + dx) as usize]).clamp(0, 255);
                        *output.y_plane.offset(pixel_offset) = new_val as u8;
                    }
                }
            }
        }
    }
}

impl VideoDecoder for H265Decoder {
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
            NalUnitType::H265IDR | NalUnitType::H265TrailN => {
                self.decode_slice(data, output)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn flush(&mut self) {
        for frame in &mut self.reference_frames {
            *frame = None;
        }
    }
}

/// H.265/HEVC encoder
pub struct H265Encoder {
    config: EncoderConfig,
    vps: VideoParameterSet,
    sps: SequenceParameterSet,
    pps: PictureParameterSet,
    reference_frames: [Option<Frame>; 16],
    frame_num: u32,
    hw_encoder: Option<HWEncoderHandle>,
    use_hardware: bool,
}

impl H265Encoder {
    pub fn new(hw_accel: &HardwareAccelerator, config: EncoderConfig) -> Result<Self, CodecError> {
        let use_hardware = config.use_hardware && hw_accel.has_hw_encode(CodecType::H265);
        let hw_encoder = if use_hardware {
            hw_accel.init_hw_encoder(CodecType::H265, &config).ok()
        } else {
            None
        };

        let vps = VideoParameterSet {
            max_layers: 1,
            max_sub_layers: 1,
        };

        let sps = SequenceParameterSet {
            width: config.width,
            height: config.height,
            bit_depth: 8,
        };

        let pps = PictureParameterSet {
            num_ref_idx_l0: 1,
            init_qp: 26,
        };

        Ok(Self {
            config,
            vps,
            sps,
            pps,
            reference_frames: Default::default(),
            frame_num: 0,
            hw_encoder,
            use_hardware,
        })
    }
}

impl VideoEncoder for H265Encoder {
    fn encode(&mut self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        // Use hardware encoder if available
        if self.use_hardware {
            if let Some(hw_encoder) = &self.hw_encoder {
                return hw_encoder.encode(frame, output);
            }
        }

        // Software encode (simplified)
        let mut offset = 0;

        // Write VPS, SPS, PPS on IDR frames
        if self.frame_num == 0 || self.frame_num % self.config.gop_size == 0 {
            // VPS
            let vps_data = [0u8; 32];
            offset += self.write_nal(output, offset, 32 << 1, &vps_data);

            // SPS
            let sps_data = [0u8; 64];
            offset += self.write_nal(output, offset, 33 << 1, &sps_data);

            // PPS
            let pps_data = [0u8; 32];
            offset += self.write_nal(output, offset, 34 << 1, &pps_data);
        }

        // Encode slice
        let slice_data = [0u8; 2048];
        let nal_type = if self.frame_num % self.config.gop_size == 0 {
            19 << 1  // IDR
        } else {
            1 << 1   // TRAIL_R
        };
        offset += self.write_nal(output, offset, nal_type, &slice_data);

        self.frame_num = self.frame_num.saturating_add(1);

        Ok(offset)
    }

    fn flush(&mut self) {
        self.frame_num = 0;
    }
}

impl H265Encoder {
    fn write_nal(&self, output: &mut [u8], offset: usize, nal_type: u8, payload: &[u8]) -> usize {
        output[offset] = 0x00;
        output[offset + 1] = 0x00;
        output[offset + 2] = 0x00;
        output[offset + 3] = 0x01;

        // H.265 NAL header (2 bytes)
        output[offset + 4] = nal_type;
        output[offset + 5] = 0x01; // nuh_layer_id = 0, nuh_temporal_id_plus1 = 1

        let len = payload.len().min(output.len() - offset - 6);
        output[offset + 6..offset + 6 + len].copy_from_slice(&payload[..len]);

        6 + len
    }
}

// Internal structures

#[derive(Clone, Copy)]
struct VideoParameterSet {
    max_layers: u8,
    max_sub_layers: u8,
}

#[derive(Clone, Copy)]
struct SequenceParameterSet {
    width: u32,
    height: u32,
    bit_depth: u8,
}

#[derive(Clone, Copy)]
struct PictureParameterSet {
    num_ref_idx_l0: u32,
    init_qp: u8,
}
