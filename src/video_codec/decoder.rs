// video_codec/decoder.rs - Video decoder trait

#![no_std]

use crate::video_codec::types::{Frame, CodecError};

/// Video decoder trait
pub trait VideoDecoder {
    /// Decode a compressed frame
    fn decode(&mut self, data: &[u8], output: &mut Frame) -> Result<(), CodecError>;

    /// Flush internal buffers
    fn flush(&mut self);
}
