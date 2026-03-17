// video_codec/av1.rs - AV1 codec implementation

#![no_std]

use crate::video_codec::types::*;
use crate::video_codec::hardware::{HardwareAccelerator, HWDecoderHandle, HWEncoderHandle};
use crate::video_codec::decoder::VideoDecoder;
use crate::video_codec::encoder::VideoEncoder;
use crate::video_codec::bitstream::BitstreamParser;
use crate::video_codec::transform::Transform;
use crate::video_codec::motion::MotionCompensation;
use crate::video_codec::entropy::SymbolDecoder;
use crate::video_codec::filter::CDEF;

/// AV1 decoder
pub struct AV1Decoder {
    sequence_header: Option<SequenceHeader>,
    reference_frames: [Option<Frame>; 8],
    frame_context: AV1FrameContext,
    hw_decoder: Option<HWDecoderHandle>,
    use_hardware: bool,
}

impl AV1Decoder {
    pub fn new(hw_accel: &HardwareAccelerator) -> Result<Self, CodecError> {
        let use_hardware = hw_accel.has_hw_decode(CodecType::AV1);
        let hw_decoder = if use_hardware {
            hw_accel.init_hw_decoder(CodecType::AV1).ok()
        } else {
            None
        };

        Ok(Self {
            sequence_header: None,
            reference_frames: Default::default(),
            frame_context: AV1FrameContext::default(),
            hw_decoder,
            use_hardware,
        })
    }

    /// Parse OBU (Open Bitstream Unit) header
    fn parse_obu_header(&self, parser: &mut BitstreamParser) -> Result<OBUHeader, CodecError> {
        let obu_forbidden_bit = parser.read_bits(1)?;
        if obu_forbidden_bit != 0 {
            return Err(CodecError::InvalidBitstream);
        }

        let obu_type = parser.read_bits(4)?;
        let obu_extension_flag = parser.read_bits(1)?;
        let obu_has_size_field = parser.read_bits(1)?;
        let _obu_reserved = parser.read_bits(1)?;

        let mut temporal_id = 0;
        let mut spatial_id = 0;

        if obu_extension_flag == 1 {
            temporal_id = parser.read_bits(3)?;
            spatial_id = parser.read_bits(2)?;
            let _extension_reserved = parser.read_bits(3)?;
        }

        let obu_size = if obu_has_size_field == 1 {
            parser.read_leb128()?
        } else {
            0
        };

        Ok(OBUHeader {
            obu_type: obu_type as u8,
            temporal_id: temporal_id as u8,
            spatial_id: spatial_id as u8,
            obu_size: obu_size as u32,
        })
    }

    /// Parse sequence header OBU
    fn parse_sequence_header(&mut self, parser: &mut BitstreamParser) -> Result<(), CodecError> {
        let seq_profile = parser.read_bits(3)?;
        let still_picture = parser.read_bits(1)?;
        let reduced_still_picture_header = parser.read_bits(1)?;

        let mut seq_header = SequenceHeader {
            seq_profile: seq_profile as u8,
            still_picture: still_picture == 1,
            max_frame_width: 0,
            max_frame_height: 0,
            bit_depth: 8,
        };

        if reduced_still_picture_header == 0 {
            let _timing_info_present = parser.read_bits(1)?;
            let _decoder_model_info_present = parser.read_bits(1)?;
            let _initial_display_delay_present = parser.read_bits(1)?;
            let _operating_points_cnt = parser.read_bits(5)?;
        }

        // Frame size
        let frame_width_bits = parser.read_bits(4)? + 1;
        let frame_height_bits = parser.read_bits(4)? + 1;

        seq_header.max_frame_width = (parser.read_bits(frame_width_bits as usize)? + 1) as u32;
        seq_header.max_frame_height = (parser.read_bits(frame_height_bits as usize)? + 1) as u32;

        self.sequence_header = Some(seq_header);
        Ok(())
    }

    /// Parse frame header OBU
    fn parse_frame_header(&mut self, parser: &mut BitstreamParser) -> Result<FrameHeaderAV1, CodecError> {
        let show_existing_frame = parser.read_bits(1)?;
        if show_existing_frame == 1 {
            let _frame_to_show = parser.read_bits(3)?;
            return Err(CodecError::UnsupportedFeature);
        }

        let frame_type = parser.read_bits(2)?;
        let show_frame = parser.read_bits(1)?;

        let mut header = FrameHeaderAV1 {
            frame_type: match frame_type {
                0 => FrameType::I,
                1 => FrameType::P,
                _ => FrameType::B,
            },
            show_frame: show_frame == 1,
            width: 0,
            height: 0,
            refresh_frame_flags: 0,
        };

        if frame_type == 0 || frame_type == 2 {
            // Key frame or intra-only frame
            self.parse_frame_size(parser, &mut header)?;
        } else {
            // Inter frame
            header.refresh_frame_flags = parser.read_bits(8)? as u8;
        }

        Ok(header)
    }

    fn parse_frame_size(&self, parser: &mut BitstreamParser, header: &mut FrameHeaderAV1) -> Result<(), CodecError> {
        if let Some(seq_header) = &self.sequence_header {
            let frame_size_override = parser.read_bits(1)?;

            if frame_size_override == 1 {
                let width_minus_1 = parser.read_bits(16)?;
                let height_minus_1 = parser.read_bits(16)?;
                header.width = (width_minus_1 + 1) as u32;
                header.height = (height_minus_1 + 1) as u32;
            } else {
                header.width = seq_header.max_frame_width;
                header.height = seq_header.max_frame_height;
            }
        }

        Ok(())
    }

    /// Decode tile group
    fn decode_tile_group(&mut self, parser: &mut BitstreamParser, output: &mut Frame) -> Result<(), CodecError> {
        let tile_cols = (output.width + 63) / 64;
        let tile_rows = (output.height + 63) / 64;

        for tile_row in 0..tile_rows {
            for tile_col in 0..tile_cols {
                self.decode_tile(parser, tile_col, tile_row, output)?;
            }
        }

        Ok(())
    }

    /// Decode single tile
    fn decode_tile(&mut self, parser: &mut BitstreamParser, col: u32, row: u32, output: &mut Frame) -> Result<(), CodecError> {
        let sb_cols = 1; // Simplified: one superblock per tile
        let sb_rows = 1;

        for r in 0..sb_rows {
            for c in 0..sb_cols {
                self.decode_superblock(parser, col + c, row + r, output)?;
            }
        }

        Ok(())
    }

    /// Decode superblock (128x128 in AV1)
    fn decode_superblock(&mut self, parser: &mut BitstreamParser, col: u32, row: u32, output: &mut Frame) -> Result<(), CodecError> {
        // Parse partition tree
        let partition = parser.read_bits(2)?;

        let base_x = col * 128;
        let base_y = row * 128;

        match partition {
            0 => {
                // Single 128x128 block
                self.decode_coding_unit(parser, base_x, base_y, 128, output)?;
            }
            1 => {
                // Split into 64x64 blocks
                for i in 0..4 {
                    let x = base_x + (i % 2) * 64;
                    let y = base_y + (i / 2) * 64;
                    self.decode_coding_unit(parser, x, y, 64, output)?;
                }
            }
            _ => {
                // Other partition modes
                self.decode_coding_unit(parser, base_x, base_y, 64, output)?;
            }
        }

        Ok(())
    }

    /// Decode coding unit
    fn decode_coding_unit(&mut self, parser: &mut BitstreamParser, x: u32, y: u32, size: u32, output: &mut Frame) -> Result<(), CodecError> {
        let is_inter = parser.read_bits(1)?;

        if is_inter == 0 {
            self.decode_intra_cu(parser, x, y, size, output)?;
        } else {
            self.decode_inter_cu(parser, x, y, size, output)?;
        }

        Ok(())
    }

    fn decode_intra_cu(&mut self, parser: &mut BitstreamParser, x: u32, y: u32, size: u32, output: &mut Frame) -> Result<(), CodecError> {
        let _intra_mode = parser.read_bits(5)?;

        // Decode transform coefficients using symbol decoder
        let mut coeffs = [0i16; 16384];
        SymbolDecoder::decode_coeffs(parser, &mut coeffs[0..(size * size) as usize])?;

        // AV1 uses identity or DCT transform
        let tx_type = parser.read_bits(2)?;
        let mut residual = [0i16; 16384];

        if tx_type == 0 {
            // Identity transform
            residual[0..(size * size) as usize].copy_from_slice(&coeffs[0..(size * size) as usize]);
        } else {
            // DCT transform
            match size {
                4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
                8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
                16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
                32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
                64 => Transform::inverse_dct_64x64(&coeffs[0..4096], &mut residual[0..4096]),
                _ => return Err(CodecError::InvalidParameter),
            }
        }

        self.add_residual(x, y, size, &residual, output);

        Ok(())
    }

    fn decode_inter_cu(&mut self, parser: &mut BitstreamParser, x: u32, y: u32, size: u32, output: &mut Frame) -> Result<(), CodecError> {
        // Parse compound prediction mode (AV1 supports multiple reference frames)
        let ref_frame_0 = parser.read_bits(3)? as usize;
        let ref_frame_1 = parser.read_bits(3)? as usize;

        // Motion vectors
        let mvd_x = parser.read_se()?;
        let mvd_y = parser.read_se()?;

        let mv = MotionVector {
            x: mvd_x as i16,
            y: mvd_y as i16,
        };

        // Compound motion compensation (average two reference frames)
        if let Some(ref_frame) = &self.reference_frames[ref_frame_0] {
            MotionCompensation::compensate(ref_frame, output, x, y, size, mv);
        }

        // Decode residual
        let mut coeffs = [0i16; 16384];
        SymbolDecoder::decode_coeffs(parser, &mut coeffs[0..(size * size) as usize])?;

        let mut residual = [0i16; 16384];
        match size {
            4 => Transform::inverse_dct_4x4(&coeffs[0..16], &mut residual[0..16]),
            8 => Transform::inverse_dct_8x8(&coeffs[0..64], &mut residual[0..64]),
            16 => Transform::inverse_dct_16x16(&coeffs[0..256], &mut residual[0..256]),
            32 => Transform::inverse_dct_32x32(&coeffs[0..1024], &mut residual[0..1024]),
            64 => Transform::inverse_dct_64x64(&coeffs[0..4096], &mut residual[0..4096]),
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

impl VideoDecoder for AV1Decoder {
    fn decode(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        // Use hardware decoder if available
        if self.use_hardware {
            if let Some(hw_decoder) = &self.hw_decoder {
                return hw_decoder.decode(data, output);
            }
        }

        // Software decode
        let mut parser = BitstreamParser::new(data);

        loop {
            let obu_header = self.parse_obu_header(&mut parser)?;

            match obu_header.obu_type {
                1 => {
                    // OBU_SEQUENCE_HEADER
                    self.parse_sequence_header(&mut parser)?;
                }
                3 => {
                    // OBU_FRAME_HEADER
                    let _frame_header = self.parse_frame_header(&mut parser)?;
                }
                4 => {
                    // OBU_TILE_GROUP
                    self.decode_tile_group(&mut parser, output)?;
                }
                6 => {
                    // OBU_FRAME (header + tiles)
                    let _frame_header = self.parse_frame_header(&mut parser)?;
                    self.decode_tile_group(&mut parser, output)?;
                }
                _ => {
                    // Skip unknown OBU types
                    if obu_header.obu_size > 0 {
                        parser.skip_bytes(obu_header.obu_size as usize)?;
                    }
                }
            }

            if parser.bits_remaining() == 0 {
                break;
            }
        }

        // Apply CDEF (Constrained Directional Enhancement Filter)
        CDEF::apply(output);

        Ok(())
    }

    fn flush(&mut self) {
        for frame in &mut self.reference_frames {
            *frame = None;
        }
    }
}

/// AV1 encoder
pub struct AV1Encoder {
    config: EncoderConfig,
    sequence_header: SequenceHeader,
    reference_frames: [Option<Frame>; 8],
    frame_num: u32,
    hw_encoder: Option<HWEncoderHandle>,
    use_hardware: bool,
}

impl AV1Encoder {
    pub fn new(hw_accel: &HardwareAccelerator, config: EncoderConfig) -> Result<Self, CodecError> {
        let use_hardware = config.use_hardware && hw_accel.has_hw_encode(CodecType::AV1);
        let hw_encoder = if use_hardware {
            hw_accel.init_hw_encoder(CodecType::AV1, &config).ok()
        } else {
            None
        };

        let sequence_header = SequenceHeader {
            seq_profile: 0,
            still_picture: false,
            max_frame_width: config.width,
            max_frame_height: config.height,
            bit_depth: 8,
        };

        Ok(Self {
            config,
            sequence_header,
            reference_frames: Default::default(),
            frame_num: 0,
            hw_encoder,
            use_hardware,
        })
    }

    fn write_obu(&self, output: &mut [u8], offset: usize, obu_type: u8, payload: &[u8]) -> usize {
        // OBU header
        output[offset] = (obu_type << 3) | 0x02; // has_size_field = 1

        // Write size as LEB128
        let size = payload.len();
        let mut size_offset = offset + 1;

        if size < 128 {
            output[size_offset] = size as u8;
            size_offset += 1;
        } else {
            output[size_offset] = ((size & 0x7F) | 0x80) as u8;
            output[size_offset + 1] = ((size >> 7) & 0x7F) as u8;
            size_offset += 2;
        }

        // Write payload
        let len = payload.len().min(output.len() - size_offset);
        output[size_offset..size_offset + len].copy_from_slice(&payload[..len]);

        size_offset + len - offset
    }
}

impl VideoEncoder for AV1Encoder {
    fn encode(&mut self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        // Use hardware encoder if available
        if self.use_hardware {
            if let Some(hw_encoder) = &self.hw_encoder {
                return hw_encoder.encode(frame, output);
            }
        }

        // Software encode
        let mut offset = 0;

        // Write sequence header on first frame
        if self.frame_num == 0 {
            let seq_data = [0u8; 64]; // Simplified
            offset += self.write_obu(output, offset, 1, &seq_data);
        }

        // Write frame OBU
        let frame_data = [0u8; 2048]; // Simplified
        offset += self.write_obu(output, offset, 6, &frame_data);

        self.frame_num = self.frame_num.saturating_add(1);

        Ok(offset)
    }

    fn flush(&mut self) {
        self.frame_num = 0;
    }
}

// Internal structures

#[derive(Default)]
struct AV1FrameContext {
    // Symbol decoder context
}

struct OBUHeader {
    obu_type: u8,
    temporal_id: u8,
    spatial_id: u8,
    obu_size: u32,
}

struct SequenceHeader {
    seq_profile: u8,
    still_picture: bool,
    max_frame_width: u32,
    max_frame_height: u32,
    bit_depth: u8,
}

struct FrameHeaderAV1 {
    frame_type: FrameType,
    show_frame: bool,
    width: u32,
    height: u32,
    refresh_frame_flags: u8,
}
