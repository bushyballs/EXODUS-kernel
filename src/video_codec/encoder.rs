// video_codec/encoder.rs - Video encoder trait

#![no_std]

use crate::video_codec::types::{Frame, CodecError};

/// Video encoder trait
pub trait VideoEncoder {
    /// Encode a frame to compressed bitstream
    /// Returns the number of bytes written to output buffer
    fn encode(&mut self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError>;

    /// Flush internal buffers
    fn flush(&mut self);
}
