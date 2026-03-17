use alloc::vec::Vec;

/// Video codec type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Raw,          // Uncompressed ARGB
    MotionJpeg,   // Per-frame compression
    GenesisVideo, // Our inter-frame codec
}

/// Frame type
#[derive(Debug, Clone, Copy)]
pub enum FrameType {
    I, // Keyframe (full frame)
    P, // Predicted (delta from previous)
    B, // Bi-predicted (not implemented yet)
}

/// Video frame
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub frame_type: FrameType,
    pub timestamp_ms: u64,
    pub data: Vec<u8>,
}

/// Encoder state
pub struct VideoEncoder {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
    pub keyframe_interval: u32,
    pub frame_count: u64,
    prev_frame: Vec<u8>,
    /// Quality (1-100)
    pub quality: u8,
}

impl VideoEncoder {
    pub fn new(codec: VideoCodec, width: u32, height: u32, fps: u8) -> Self {
        VideoEncoder {
            codec,
            width,
            height,
            fps,
            bitrate: width * height * fps as u32,
            keyframe_interval: fps as u32 * 2, // keyframe every 2 seconds
            frame_count: 0,
            prev_frame: Vec::new(),
            quality: 80,
        }
    }

    /// Encode a raw ARGB frame
    pub fn encode(&mut self, raw_pixels: &[u32]) -> VideoFrame {
        let is_keyframe =
            self.frame_count % self.keyframe_interval as u64 == 0 || self.prev_frame.is_empty();

        let frame_type = if is_keyframe {
            FrameType::I
        } else {
            FrameType::P
        };

        let data = match self.codec {
            VideoCodec::Raw => {
                // Just copy the raw pixels
                let mut bytes = Vec::with_capacity(raw_pixels.len() * 4);
                for &pixel in raw_pixels {
                    bytes.extend_from_slice(&pixel.to_le_bytes());
                }
                bytes
            }
            VideoCodec::GenesisVideo => {
                if is_keyframe {
                    // I-frame: simple RLE compression
                    self.encode_keyframe(raw_pixels)
                } else {
                    // P-frame: delta encoding from previous
                    self.encode_delta(raw_pixels)
                }
            }
            _ => Vec::new(),
        };

        // Store current frame as reference
        self.prev_frame = raw_pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        self.frame_count = self.frame_count.saturating_add(1);

        VideoFrame {
            width: self.width,
            height: self.height,
            frame_type,
            timestamp_ms: self.frame_count * 1000 / self.fps as u64,
            data,
        }
    }

    fn encode_keyframe(&self, pixels: &[u32]) -> Vec<u8> {
        // Simple RLE on pixels
        let mut output = Vec::new();
        output.push(b'I'); // frame type marker

        let mut run_pixel = pixels[0];
        let mut run_len: u16 = 1;

        for &pixel in &pixels[1..] {
            if pixel == run_pixel && run_len < 0xFFFF {
                run_len += 1;
            } else {
                output.extend_from_slice(&run_len.to_le_bytes());
                output.extend_from_slice(&run_pixel.to_le_bytes());
                run_pixel = pixel;
                run_len = 1;
            }
        }
        output.extend_from_slice(&run_len.to_le_bytes());
        output.extend_from_slice(&run_pixel.to_le_bytes());
        output
    }

    fn encode_delta(&self, pixels: &[u32]) -> Vec<u8> {
        let mut output = Vec::new();
        output.push(b'P'); // frame type marker

        // Encode only changed 8x8 blocks
        let blocks_x = (self.width + 7) / 8;
        let blocks_y = (self.height + 7) / 8;

        let mut changed_blocks = Vec::new();

        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let mut changed = false;
                'block_check: for dy in 0..8 {
                    let y = by * 8 + dy;
                    if y >= self.height {
                        break;
                    }
                    for dx in 0..8 {
                        let x = bx * 8 + dx;
                        if x >= self.width {
                            break;
                        }
                        let idx = (y * self.width + x) as usize;
                        if idx < pixels.len() {
                            let prev_idx = idx * 4;
                            if prev_idx + 3 < self.prev_frame.len() {
                                let prev_pixel = u32::from_le_bytes([
                                    self.prev_frame[prev_idx],
                                    self.prev_frame[prev_idx + 1],
                                    self.prev_frame[prev_idx + 2],
                                    self.prev_frame[prev_idx + 3],
                                ]);
                                if pixels[idx] != prev_pixel {
                                    changed = true;
                                    break 'block_check;
                                }
                            }
                        }
                    }
                }
                if changed {
                    changed_blocks.push((bx, by));
                }
            }
        }

        // Write number of changed blocks
        let num_blocks = changed_blocks.len() as u16;
        output.extend_from_slice(&num_blocks.to_le_bytes());

        // Write each changed block
        for (bx, by) in changed_blocks {
            output.extend_from_slice(&(bx as u16).to_le_bytes());
            output.extend_from_slice(&(by as u16).to_le_bytes());
            for dy in 0..8 {
                let y = by * 8 + dy;
                if y >= self.height {
                    continue;
                }
                for dx in 0..8 {
                    let x = bx * 8 + dx;
                    if x >= self.width {
                        continue;
                    }
                    let idx = (y * self.width + x) as usize;
                    if idx < pixels.len() {
                        output.extend_from_slice(&pixels[idx].to_le_bytes());
                    }
                }
            }
        }
        output
    }
}

/// Decoder state
pub struct VideoDecoder {
    pub width: u32,
    pub height: u32,
    pub frame_buffer: Vec<u32>,
    pub frames_decoded: u64,
}

impl VideoDecoder {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        VideoDecoder {
            width,
            height,
            frame_buffer: alloc::vec![0u32; size],
            frames_decoded: 0,
        }
    }

    /// Decode a frame and return pixel buffer
    pub fn decode(&mut self, frame: &VideoFrame) -> &[u32] {
        match frame.data.first() {
            Some(b'I') => self.decode_keyframe(&frame.data[1..]),
            Some(b'P') => self.decode_delta(&frame.data[1..]),
            _ => {}
        }
        self.frames_decoded = self.frames_decoded.saturating_add(1);
        &self.frame_buffer
    }

    fn decode_keyframe(&mut self, data: &[u8]) {
        let mut i = 0;
        let mut pixel_idx = 0;
        while i + 5 < data.len() && pixel_idx < self.frame_buffer.len() {
            let run_len = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
            let pixel = u32::from_le_bytes([data[i + 2], data[i + 3], data[i + 4], data[i + 5]]);
            for _ in 0..run_len {
                if pixel_idx < self.frame_buffer.len() {
                    self.frame_buffer[pixel_idx] = pixel;
                    pixel_idx += 1;
                }
            }
            i += 6;
        }
    }

    fn decode_delta(&mut self, data: &[u8]) {
        if data.len() < 2 {
            return;
        }
        let num_blocks = u16::from_le_bytes([data[0], data[1]]) as usize;
        let mut i = 2;

        for _ in 0..num_blocks {
            if i + 3 >= data.len() {
                break;
            }
            let bx = u16::from_le_bytes([data[i], data[i + 1]]) as u32;
            let by = u16::from_le_bytes([data[i + 2], data[i + 3]]) as u32;
            i += 4;

            for dy in 0..8u32 {
                let y = by * 8 + dy;
                if y >= self.height {
                    continue;
                }
                for dx in 0..8u32 {
                    let x = bx * 8 + dx;
                    if x >= self.width {
                        continue;
                    }
                    if i + 3 < data.len() {
                        let pixel =
                            u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                        let idx = (y * self.width + x) as usize;
                        if idx < self.frame_buffer.len() {
                            self.frame_buffer[idx] = pixel;
                        }
                        i += 4;
                    }
                }
            }
        }
    }
}

pub fn init() {
    crate::serial_println!("  [video] Video codec initialized (Raw, GenesisVideo)");
}
