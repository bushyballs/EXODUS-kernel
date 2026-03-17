// video_codec/vp9.rs - VP9 codec implementation

#![no_std]

use crate::video_codec::types::*;
use crate::video_codec::hardware::{HardwareAccelerator, HWDecoderHandle, HWEncoderHandle};
use crate::video_codec::decoder::VideoDecoder;
use crate::video_codec::encoder::VideoEncoder;
use crate::video_codec::bitstream::BitstreamParser;
use crate::video_codec::transform::Transform;
use crate::video_codec::motion::MotionCompensation;
use crate::video_codec::entropy::BoolDecoder;
use crate::video_codec::filter::LoopFilter;

/// VP9 decoder
pub struct VP9Decoder {
    reference_frames: [Option<Frame>; 8],
    frame_context: FrameContext,
    hw_decoder: Option<HWDecoderHandle>,
    use_hardware: bool,
}

impl VP9Decoder {
    pub fn new(hw_accel: &HardwareAccelerator) -> Result<Self, CodecError> {
        let use_hardware = hw_accel.has_hw_decode(CodecType::VP9);
        let hw_decoder = if use_hardware {
            hw_accel.init_hw_decoder(CodecType::VP9).ok()
        } else {
            None
        };

        Ok(Self {
            reference_frames: Default::default(),
            frame_context: FrameContext::default(),
            hw_decoder,
            use_hardware,
        })
    }

    /// Parse uncompressed header
    fn parse_uncompressed_header(&mut self, parser: &mut BitstreamParser) -> Result<FrameHeader, CodecError> {
        let frame_marker = parser.read_bits(2)?;
        if frame_marker != 0x2 {
            return Err(CodecError::InvalidBitstream);
        }

        let profile_low_bit = parser.read_bits(1)?;
        let profile_high_bit = parser.read_bits(1)?;
        let _profile = (profile_high_bit << 1) | profile_low_bit;

        let show_existing_frame = parser.read_bits(1)?;
        if show_existing_frame == 1 {
            let _frame_to_show = parser.read_bits(3)?;
            return Err(CodecError::UnsupportedFeature);
        }

        let frame_type = parser.read_bits(1)?;
        let show_frame = parser.read_bits(1)?;
        let error_resilient = parser.read_bits(1)?;

        let mut header = FrameHeader {
            frame_type: if frame_type == 0 { FrameType::I } else { FrameType::P },
            show_frame: show_frame == 1,
            error_resilient: error_resilient == 1,
            width: 0,
            height: 0,
            refresh_frame_flags: 0,
        };

        if frame_type == 0 {
            // Key frame
            self.parse_frame_size(parser, &mut header)?;
            self.parse_render_size(parser, &mut header)?;
        } else {
            // Inter frame
            let _intra_only = if show_frame == 0 {
                parser.read_bits(1)?
            } else {
                0
            };

            if error_resilient == 0 {
                let _reset_frame_context = parser.read_bits(2)?;
            }

            // Reference frame indices
            for _ in 0..3 {
                let _ref_frame_idx = parser.read_bits(3)?;
            }
        }

        header.refresh_frame_flags = parser.read_bits(8)? as u8;

        Ok(header)
    }

    fn parse_frame_size(&self, parser: &mut BitstreamParser, header: &mut FrameHeader) -> Result<(), CodecError> {
        let width_minus_1 = parser.read_bits(16)?;
        let height_minus_1 = parser.read_bits(16)?;

        header.width = (width_minus_1 + 1) as u32;
        header.height = (height_minus_1 + 1) as u32;

        Ok(())
    }

    fn parse_render_size(&self, parser: &mut BitstreamParser, header: &mut FrameHeader) -> Result<(), CodecError> {
        let render_and_frame_size_different = parser.read_bits(1)?;

        if render_and_frame_size_different == 1 {
            let _render_width_minus_1 = parser.read_bits(16)?;
            let _render_height_minus_1 = parser.read_bits(16)?;
        }

        Ok(())
    }

    /// Decode tile
    fn decode_tile(&mut self, parser: &mut BitstreamParser, output: &mut Frame) -> Result<(), CodecError> {
        let sb_cols = (output.width + 63) / 64;
        let sb_rows = (output.height + 63) / 64;

        // Decode superblocks (64x64)
        for r in 0..sb_rows {
            for c in 0..sb_cols {
                self.decode_superblock(parser, c, r, output)?;
            }
        }

        Ok(())
    }

    /// Decode superblock (64x64)
    fn decode_superblock(
        &mut self,
        parser: &mut BitstreamParser,
        col: u32,
        row: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Partition tree
        let partition = parser.read_bits(2)?;

        match partition {
            0 => {
                // PARTITION_NONE: single 64x64 block
                self.decode_block(parser, col * 64, row * 64, 64, output)?;
            }
            1 => {
                // PARTITION_HORZ: two 64x32 blocks
                self.decode_block(parser, col * 64, row * 64, 32, output)?;
                self.decode_block(parser, col * 64, row * 64 + 32, 32, output)?;
            }
            2 => {
                // PARTITION_VERT: two 32x64 blocks
                self.decode_block(parser, col * 64, row * 64, 32, output)?;
                self.decode_block(parser, col * 64 + 32, row * 64, 32, output)?;
            }
            3 => {
                // PARTITION_SPLIT: four 32x32 blocks
                for i in 0..4 {
                    let x = col * 64 + (i % 2) * 32;
                    let y = row * 64 + (i / 2) * 32;
                    self.decode_block(parser, x, y, 32, output)?;
                }
            }
            _ => return Err(CodecError::InvalidBitstream),
        }

        Ok(())
    }

    /// Decode block
    fn decode_block(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        let is_inter = parser.read_bits(1)?;

        if is_inter == 0 {
            // Intra prediction
            self.decode_intra_block(parser, x, y, size, output)?;
        } else {
            // Inter prediction
            self.decode_inter_block(parser, x, y, size, output)?;
        }

        Ok(())
    }

    fn decode_intra_block(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        let _intra_mode = parser.read_bits(4)?;

        // Decode transform coefficients
        let mut coeffs = [0i16; 4096];
        BoolDecoder::decode_coeffs(parser, &mut coeffs[0..(size * size) as usize])?;

        // Inverse transform
        let mut residual = [0i16; 4096];
        match size {
            4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
            8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
            16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
            32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
            _ => return Err(CodecError::InvalidParameter),
        }

        self.add_residual(x, y, size, &residual, output);

        Ok(())
    }

    fn decode_inter_block(
        &mut self,
        parser: &mut BitstreamParser,
        x: u32,
        y: u32,
        size: u32,
        output: &mut Frame,
    ) -> Result<(), CodecError> {
        // Parse motion vector
        let mvd_x = parser.read_se()?;
        let mvd_y = parser.read_se()?;

        let mv = MotionVector {
            x: mvd_x as i16,
            y: mvd_y as i16,
        };

        // Motion compensation
        let ref_frame_idx = parser.read_bits(2)? as usize;
        if let Some(ref_frame) = &self.reference_frames[ref_frame_idx] {
            MotionCompensation::compensate(ref_frame, output, x, y, size, mv);
        }

        // Decode residual
        let mut coeffs = [0i16; 4096];
        BoolDecoder::decode_coeffs(parser, &mut coeffs[0..(size * size) as usize])?;

        let mut residual = [0i16; 4096];
        match size {
            4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
            8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
            16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
            32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
            _ => return Err(CodecError::InvalidParameter),
        }

        self.add_residual(x, y, size, &residual, output);

        Ok(())
    }

    fn add_residual(&self, x: u32, y: u32, size: u32, residual: &[i16], output: &mut Frame) {
        unsafe {
            for dy in 0..size {
                for dx in 0..size {
                    if x + dx < output.width && y + dy < output.height {
                        let offset = ((y + dy) * output.y_stride + x + dx) as isize;
                        let current = *output.y_plane.offset(offset) as i16;
                        let new_val = (current + residual[(dy * size + dx) as usize]).clamp(0, 255);
                        *output.y_plane.offset(offset) = new_val as u8;
                    }
                }
            }
        }
    }
}

impl VideoDecoder for VP9Decoder {
    fn decode(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        // Use hardware decoder if available
        if self.use_hardware {
            if let Some(hw_decoder) = &self.hw_decoder {
                return hw_decoder.decode(data, output);
            }
        }

        // Software decode
        let mut parser = BitstreamParser::new(data);

        let _header = self.parse_uncompressed_header(&mut parser)?;

        // Decode tile(s)
        self.decode_tile(&mut parser, output)?;

        // Apply loop filter
        LoopFilter::apply_vp9(output);

        Ok(())
    }

    fn flush(&mut self) {
        for frame in &mut self.reference_frames {
            *frame = None;
        }
    }
}

/// VP9 encoder
pub struct VP9Encoder {
    config: EncoderConfig,
    reference_frames: [Option<Frame>; 8],
    frame_num: u32,
    hw_encoder: Option<HWEncoderHandle>,
    use_hardware: bool,
}

impl VP9Encoder {
    pub fn new(hw_accel: &HardwareAccelerator, config: EncoderConfig) -> Result<Self, CodecError> {
        let use_hardware = config.use_hardware && hw_accel.has_hw_encode(CodecType::VP9);
        let hw_encoder = if use_hardware {
            hw_accel.init_hw_encoder(CodecType::VP9, &config).ok()
        } else {
            None
        };

        Ok(Self {
            config,
            reference_frames: Default::default(),
            frame_num: 0,
            hw_encoder,
            use_hardware,
        })
    }
}

impl VideoEncoder for VP9Encoder {
    fn encode(&mut self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        // Use hardware encoder if available
        if self.use_hardware {
            if let Some(hw_encoder) = &self.hw_encoder {
                return hw_encoder.encode(frame, output);
            }
        }

        // Software encode (simplified)
        let is_keyframe = self.frame_num % self.config.gop_size == 0;

        // Write uncompressed header
        let mut offset = 0;

        // Frame marker
        output[offset] = 0x82; // frame_marker = 2, profile = 0
        offset += 1;

        // Frame type and flags
        let frame_type_byte = if is_keyframe { 0x00 } else { 0x01 };
        output[offset] = frame_type_byte;
        offset += 1;

        if is_keyframe {
            // Write frame size
            let width_minus_1 = (frame.width - 1) as u16;
            let height_minus_1 = (frame.height - 1) as u16;

            output[offset] = (width_minus_1 & 0xFF) as u8;
            output[offset + 1] = ((width_minus_1 >> 8) & 0xFF) as u8;
            output[offset + 2] = (height_minus_1 & 0xFF) as u8;
            output[offset + 3] = ((height_minus_1 >> 8) & 0xFF) as u8;
            offset += 4;
        }

        // Simplified: just return header for now
        self.frame_num = self.frame_num.saturating_add(1);

        Ok(offset)
    }

    fn flush(&mut self) {
        self.frame_num = 0;
    }
}

// Internal structures

#[derive(Default)]
struct FrameContext {
    // Probability tables for entropy coding
}

struct FrameHeader {
    frame_type: FrameType,
    show_frame: bool,
    error_resilient: bool,
    width: u32,
    height: u32,
    refresh_frame_flags: u8,
}
