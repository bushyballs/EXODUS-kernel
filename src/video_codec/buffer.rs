// video_codec/buffer.rs - Video buffer management

#![no_std]

use crate::video_codec::types::{Frame, PixelFormat, CodecError};

/// Video buffer pool for efficient memory management
pub struct BufferPool {
    buffers: [Option<VideoBuffer>; 16],
    next_index: usize,
}

impl BufferPool {
    /// Create a new buffer pool
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
            next_index: 0,
        }
    }

    /// Allocate a buffer from the pool
    pub fn allocate(&mut self, width: u32, height: u32, format: PixelFormat) -> Result<&mut VideoBuffer, CodecError> {
        // Find first available buffer
        for i in 0..16 {
            let idx = (self.next_index + i) % 16;
            if self.buffers[idx].is_none() {
                self.buffers[idx] = Some(VideoBuffer::new(width, height, format)?);
                self.next_index = (idx + 1) % 16;
                return Ok(self.buffers[idx].as_mut().unwrap());
            }
        }

        Err(CodecError::OutOfMemory)
    }

    /// Release a buffer back to the pool
    pub fn release(&mut self, index: usize) {
        if index < 16 {
            self.buffers[index] = None;
        }
    }

    /// Get buffer by index
    pub fn get(&self, index: usize) -> Option<&VideoBuffer> {
        if index < 16 {
            self.buffers[index].as_ref()
        } else {
            None
        }
    }

    /// Get mutable buffer by index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut VideoBuffer> {
        if index < 16 {
            self.buffers[index].as_mut()
        } else {
            None
        }
    }
}

/// Video buffer with allocated memory
pub struct VideoBuffer {
    frame: Frame,
    y_data: [u8; 4096 * 4096],      // Max 4K resolution
    u_data: [u8; 2048 * 2048],      // Chroma planes
    v_data: [u8; 2048 * 2048],
    allocated: bool,
}

impl VideoBuffer {
    /// Create a new video buffer
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Result<Self, CodecError> {
        if width > 4096 || height > 4096 {
            return Err(CodecError::InvalidParameter);
        }

        let mut buffer = Self {
            frame: Frame::new(width, height, format),
            y_data: [0; 4096 * 4096],
            u_data: [0; 2048 * 2048],
            v_data: [0; 2048 * 2048],
            allocated: true,
        };

        // Set frame pointers
        buffer.frame.y_plane = buffer.y_data.as_mut_ptr();
        buffer.frame.u_plane = buffer.u_data.as_mut_ptr();
        buffer.frame.v_plane = buffer.v_data.as_mut_ptr();

        Ok(buffer)
    }

    /// Get frame reference
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Get mutable frame reference
    pub fn frame_mut(&mut self) -> &mut Frame {
        &mut self.frame
    }

    /// Clear buffer to black
    pub fn clear(&mut self) {
        // Y = 16 (black in YUV)
        for i in 0..(self.frame.width * self.frame.height) as usize {
            self.y_data[i] = 16;
        }

        // U, V = 128 (neutral chroma)
        let chroma_size = ((self.frame.width / 2) * (self.frame.height / 2)) as usize;
        for i in 0..chroma_size {
            self.u_data[i] = 128;
            self.v_data[i] = 128;
        }
    }

    /// Copy from another buffer
    pub fn copy_from(&mut self, src: &VideoBuffer) -> Result<(), CodecError> {
        if src.frame.width != self.frame.width || src.frame.height != self.frame.height {
            return Err(CodecError::InvalidParameter);
        }

        let y_size = (self.frame.width * self.frame.height) as usize;
        let chroma_size = ((self.frame.width / 2) * (self.frame.height / 2)) as usize;

        self.y_data[..y_size].copy_from_slice(&src.y_data[..y_size]);
        self.u_data[..chroma_size].copy_from_slice(&src.u_data[..chroma_size]);
        self.v_data[..chroma_size].copy_from_slice(&src.v_data[..chroma_size]);

        self.frame.timestamp = src.frame.timestamp;
        self.frame.frame_type = src.frame.frame_type;

        Ok(())
    }
}

/// Reference counted buffer
pub struct RcBuffer {
    buffer: VideoBuffer,
    ref_count: u32,
}

impl RcBuffer {
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Result<Self, CodecError> {
        Ok(Self {
            buffer: VideoBuffer::new(width, height, format)?,
            ref_count: 1,
        })
    }

    pub fn add_ref(&mut self) {
        self.ref_count = self.ref_count.saturating_add(1);
    }

    pub fn release(&mut self) -> bool {
        if self.ref_count > 0 {
            self.ref_count -= 1;
        }
        self.ref_count == 0
    }

    pub fn buffer(&self) -> &VideoBuffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut VideoBuffer {
        &mut self.buffer
    }
}

/// Decoded Picture Buffer (DPB) for reference frame management
pub struct DecodedPictureBuffer {
    buffers: [Option<RcBuffer>; 16],
}

impl DecodedPictureBuffer {
    pub fn new() -> Self {
        Self {
            buffers: Default::default(),
        }
    }

    /// Add a decoded frame to DPB
    pub fn add_frame(&mut self, buffer: RcBuffer) -> Result<usize, CodecError> {
        for i in 0..16 {
            if self.buffers[i].is_none() {
                self.buffers[i] = Some(buffer);
                return Ok(i);
            }
        }

        Err(CodecError::OutOfMemory)
    }

    /// Get reference frame
    pub fn get_ref(&self, index: usize) -> Option<&RcBuffer> {
        if index < 16 {
            self.buffers[index].as_ref()
        } else {
            None
        }
    }

    /// Remove oldest frame
    pub fn remove_oldest(&mut self) {
        // Simplified: remove first available
        for i in 0..16 {
            if let Some(buffer) = &mut self.buffers[i] {
                if buffer.release() {
                    self.buffers[i] = None;
                    break;
                }
            }
        }
    }

    /// Clear all frames
    pub fn clear(&mut self) {
        for buffer in &mut self.buffers {
            *buffer = None;
        }
    }
}
