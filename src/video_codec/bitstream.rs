// video_codec/bitstream.rs - Bitstream parsing utilities

#![no_std]

use crate::video_codec::types::CodecError;

/// Bitstream parser for reading variable-length codes
pub struct BitstreamParser<'a> {
    data: &'a [u8],
    byte_offset: usize,
    bit_offset: u8,
}

impl<'a> BitstreamParser<'a> {
    /// Create a new bitstream parser
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_offset: 0,
            bit_offset: 0,
        }
    }

    /// Read n bits from the bitstream
    pub fn read_bits(&mut self, n: usize) -> Result<u64, CodecError> {
        if n > 64 {
            return Err(CodecError::InvalidParameter);
        }

        let mut result = 0u64;

        for _ in 0..n {
            if self.byte_offset >= self.data.len() {
                return Err(CodecError::InvalidBitstream);
            }

            let bit = (self.data[self.byte_offset] >> (7 - self.bit_offset)) & 1;
            result = (result << 1) | (bit as u64);

            self.bit_offset += 1;
            if self.bit_offset == 8 {
                self.bit_offset = 0;
                self.byte_offset += 1;
            }
        }

        Ok(result)
    }

    /// Read unsigned exponential-Golomb code
    pub fn read_ue(&mut self) -> Result<u64, CodecError> {
        let mut leading_zeros = 0;

        // Count leading zeros
        while self.read_bits(1)? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return Err(CodecError::InvalidBitstream);
            }
        }

        if leading_zeros == 0 {
            return Ok(0);
        }

        // Read the remaining bits
        let value = self.read_bits(leading_zeros)?;
        Ok((1u64 << leading_zeros) - 1 + value)
    }

    /// Read signed exponential-Golomb code
    pub fn read_se(&mut self) -> Result<i64, CodecError> {
        let code = self.read_ue()?;

        if code == 0 {
            return Ok(0);
        }

        let sign = if code & 1 == 1 { 1 } else { -1 };
        let value = ((code + 1) >> 1) as i64;

        Ok(sign * value)
    }

    /// Read LEB128 (used in AV1)
    pub fn read_leb128(&mut self) -> Result<u64, CodecError> {
        let mut value = 0u64;
        let mut shift = 0;

        for _ in 0..8 {
            let byte = self.read_bits(8)?;
            value |= ((byte & 0x7F) as u64) << shift;

            if (byte & 0x80) == 0 {
                break;
            }

            shift += 7;
        }

        Ok(value)
    }

    /// Skip n bytes
    pub fn skip_bytes(&mut self, n: usize) -> Result<(), CodecError> {
        // Align to byte boundary
        if self.bit_offset != 0 {
            self.bit_offset = 0;
            self.byte_offset += 1;
        }

        if self.byte_offset + n > self.data.len() {
            return Err(CodecError::InvalidBitstream);
        }

        self.byte_offset += n;
        Ok(())
    }

    /// Get remaining bits in the bitstream
    pub fn bits_remaining(&self) -> usize {
        if self.byte_offset >= self.data.len() {
            return 0;
        }

        let bytes_remaining = self.data.len() - self.byte_offset;
        bytes_remaining * 8 - self.bit_offset as usize
    }

    /// Byte align the parser
    pub fn byte_align(&mut self) {
        if self.bit_offset != 0 {
            self.bit_offset = 0;
            self.byte_offset += 1;
        }
    }
}

/// Bitstream writer for encoding
pub struct BitstreamWriter<'a> {
    data: &'a mut [u8],
    byte_offset: usize,
    bit_offset: u8,
}

impl<'a> BitstreamWriter<'a> {
    pub fn new(data: &'a mut [u8]) -> Self {
        Self {
            data,
            byte_offset: 0,
            bit_offset: 0,
        }
    }

    /// Write n bits to the bitstream
    pub fn write_bits(&mut self, value: u64, n: usize) -> Result<(), CodecError> {
        if n > 64 {
            return Err(CodecError::InvalidParameter);
        }

        for i in (0..n).rev() {
            if self.byte_offset >= self.data.len() {
                return Err(CodecError::BufferTooSmall);
            }

            let bit = ((value >> i) & 1) as u8;

            if self.bit_offset == 0 {
                self.data[self.byte_offset] = 0;
            }

            self.data[self.byte_offset] |= bit << (7 - self.bit_offset);

            self.bit_offset += 1;
            if self.bit_offset == 8 {
                self.bit_offset = 0;
                self.byte_offset += 1;
            }
        }

        Ok(())
    }

    /// Write unsigned exponential-Golomb code
    pub fn write_ue(&mut self, value: u64) -> Result<(), CodecError> {
        let value_plus_1 = value + 1;
        let bit_length = 64 - value_plus_1.leading_zeros() as usize;

        // Write leading zeros
        for _ in 0..bit_length - 1 {
            self.write_bits(0, 1)?;
        }

        // Write the value
        self.write_bits(value_plus_1, bit_length)?;

        Ok(())
    }

    /// Write signed exponential-Golomb code
    pub fn write_se(&mut self, value: i64) -> Result<(), CodecError> {
        let code = if value <= 0 {
            (-value * 2) as u64
        } else {
            (value * 2 - 1) as u64
        };

        self.write_ue(code)
    }

    /// Byte align the writer
    pub fn byte_align(&mut self) {
        if self.bit_offset != 0 {
            self.bit_offset = 0;
            self.byte_offset += 1;
        }
    }

    /// Get number of bytes written
    pub fn bytes_written(&self) -> usize {
        self.byte_offset + if self.bit_offset > 0 { 1 } else { 0 }
    }
}
