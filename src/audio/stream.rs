//! Audio streaming and buffer management
//!
//! Provides buffered audio streaming with automatic codec handling.
//! Uses static buffers — no heap allocation.

use super::codecs::{get_decoder, get_encoder, StaticDecoder, StaticEncoder};
use super::error::*;
use super::types::*;

/// Maximum ring buffer size for a stream (16 KiB).
const MAX_RING_SIZE: usize = 16384;

/// Audio stream handle
pub struct AudioStream {
    codec: CodecId,
    config: AudioConfig,
    direction: StreamDirection,
    encoder: Option<StaticEncoder>,
    decoder: Option<StaticDecoder>,
    buffer: RingBuffer,
    state: StreamState,
}

/// Stream direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    Encode,
    Decode,
}

/// Stream state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Active,
    Draining,
    Error,
}

/// Ring buffer for audio samples (static, no heap)
pub struct RingBuffer {
    data: [u8; MAX_RING_SIZE],
    read_pos: usize,
    write_pos: usize,
    size: usize,
}

/// Stream statistics
pub struct StreamStats {
    pub bytes_processed: u64,
    pub frames_processed: u64,
    pub underruns: u32,
    pub overruns: u32,
}

impl AudioStream {
    /// Create a new encoding stream
    pub fn new_encoder(codec: CodecId, config: AudioConfig) -> Result<Self> {
        let encoder = get_encoder(codec)?;

        Ok(Self {
            codec,
            config,
            direction: StreamDirection::Encode,
            encoder: Some(encoder),
            decoder: None,
            buffer: RingBuffer::new(config.buffer_size.min(MAX_RING_SIZE / 4) * 4),
            state: StreamState::Idle,
        })
    }

    /// Create a new decoding stream
    pub fn new_decoder(codec: CodecId, config: AudioConfig) -> Result<Self> {
        let decoder = get_decoder(codec)?;

        Ok(Self {
            codec,
            config,
            direction: StreamDirection::Decode,
            encoder: None,
            decoder: Some(decoder),
            buffer: RingBuffer::new(config.buffer_size.min(MAX_RING_SIZE / 4) * 4),
            state: StreamState::Idle,
        })
    }

    /// Initialize the stream
    pub fn init(&mut self) -> Result<()> {
        match self.direction {
            StreamDirection::Encode => {
                if let Some(encoder) = &mut self.encoder {
                    encoder.init(&self.config)?;
                }
            }
            StreamDirection::Decode => {
                if let Some(decoder) = &mut self.decoder {
                    decoder.init(&self.config)?;
                }
            }
        }

        self.state = StreamState::Active;
        Ok(())
    }

    /// Encode audio frame
    pub fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        if self.direction != StreamDirection::Encode {
            return Err(AudioError::StreamError);
        }

        if let Some(encoder) = &mut self.encoder {
            encoder.encode(frame, output)
        } else {
            Err(AudioError::StreamError)
        }
    }

    /// Decode audio packet
    pub fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize> {
        if self.direction != StreamDirection::Decode {
            return Err(AudioError::StreamError);
        }

        if let Some(decoder) = &mut self.decoder {
            decoder.decode(packet, output)
        } else {
            Err(AudioError::StreamError)
        }
    }

    /// Write data to stream buffer
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.buffer.write(data)
    }

    /// Read data from stream buffer
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        self.buffer.read(output)
    }

    /// Get available bytes in buffer
    pub fn available(&self) -> usize {
        self.buffer.available()
    }

    /// Get free space in buffer
    pub fn free_space(&self) -> usize {
        self.buffer.free_space()
    }

    /// Drain the stream
    pub fn drain(&mut self) -> Result<()> {
        self.state = StreamState::Draining;
        self.buffer.clear();
        Ok(())
    }

    /// Reset the stream
    pub fn reset(&mut self) -> Result<()> {
        if let Some(decoder) = &mut self.decoder {
            decoder.reset();
        }

        self.buffer.clear();
        self.state = StreamState::Idle;
        Ok(())
    }

    /// Get stream state
    pub fn get_state(&self) -> StreamState {
        self.state
    }

    /// Get stream configuration
    pub fn get_config(&self) -> &AudioConfig {
        &self.config
    }
}

impl RingBuffer {
    pub fn new(size: usize) -> Self {
        Self {
            data: [0u8; MAX_RING_SIZE],
            read_pos: 0,
            write_pos: 0,
            size: size.min(MAX_RING_SIZE),
        }
    }

    /// Write data to ring buffer
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        let available = self.free_space();
        if data.len() > available {
            return Err(AudioError::BufferOverflow);
        }

        let mut bytes_written = 0;

        while bytes_written < data.len() {
            let chunk_size = (data.len() - bytes_written).min(self.size - self.write_pos);

            self.data[self.write_pos..self.write_pos + chunk_size]
                .copy_from_slice(&data[bytes_written..bytes_written + chunk_size]);

            self.write_pos = (self.write_pos + chunk_size) % self.size;
            bytes_written += chunk_size;
        }

        Ok(bytes_written)
    }

    /// Read data from ring buffer
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize> {
        let available = self.available();
        let to_read = output.len().min(available);

        if to_read == 0 {
            return Ok(0);
        }

        let mut bytes_read = 0;

        while bytes_read < to_read {
            let chunk_size = (to_read - bytes_read).min(self.size - self.read_pos);

            output[bytes_read..bytes_read + chunk_size]
                .copy_from_slice(&self.data[self.read_pos..self.read_pos + chunk_size]);

            self.read_pos = (self.read_pos + chunk_size) % self.size;
            bytes_read += chunk_size;
        }

        Ok(bytes_read)
    }

    /// Get number of bytes available to read
    pub fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.size - self.read_pos + self.write_pos
        }
    }

    /// Get free space for writing
    pub fn free_space(&self) -> usize {
        self.size.saturating_sub(self.available()).saturating_sub(1)
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.read_pos == self.write_pos
    }

    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        self.free_space() == 0
    }
}

/// Audio stream builder for easy configuration
pub struct StreamBuilder {
    codec: CodecId,
    sample_rate: u32,
    channels: u8,
    bit_depth: u8,
    buffer_size: usize,
}

impl StreamBuilder {
    pub fn new(codec: CodecId) -> Self {
        Self {
            codec,
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
            buffer_size: 4096,
        }
    }

    pub fn sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate = rate;
        self
    }

    pub fn channels(mut self, channels: u8) -> Self {
        self.channels = channels;
        self
    }

    pub fn bit_depth(mut self, depth: u8) -> Self {
        self.bit_depth = depth;
        self
    }

    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    pub fn build_encoder(self) -> Result<AudioStream> {
        let config = AudioConfig {
            sample_rate: self.sample_rate,
            channels: self.channels,
            bit_depth: self.bit_depth,
            buffer_size: self.buffer_size,
        };

        AudioStream::new_encoder(self.codec, config)
    }

    pub fn build_decoder(self) -> Result<AudioStream> {
        let config = AudioConfig {
            sample_rate: self.sample_rate,
            channels: self.channels,
            bit_depth: self.bit_depth,
            buffer_size: self.buffer_size,
        };

        AudioStream::new_decoder(self.codec, config)
    }
}
