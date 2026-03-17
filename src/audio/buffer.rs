/// Audio ring buffer — streaming playback and capture
///
/// Double-buffered ring buffer for DMA transfers.
/// Supports multiple sample formats and rates.
use alloc::vec::Vec;

const DEFAULT_BUFFER_SIZE: usize = 4096; // samples per buffer
const DEFAULT_SAMPLE_RATE: u32 = 48000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    S16Le, // 16-bit signed little-endian (CD quality)
    S24Le, // 24-bit signed little-endian
    S32Le, // 32-bit signed little-endian
    F32Le, // 32-bit float little-endian
}

impl SampleFormat {
    pub fn bytes_per_sample(&self) -> usize {
        match self {
            SampleFormat::S16Le => 2,
            SampleFormat::S24Le => 3,
            SampleFormat::S32Le | SampleFormat::F32Le => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferState {
    Empty,
    Playing,
    Recording,
    Paused,
    Full,
}

pub struct AudioBuffer {
    pub data: Vec<u8>,
    pub read_pos: usize,
    pub write_pos: usize,
    pub capacity: usize,
    pub state: BufferState,
    pub format: SampleFormat,
    pub sample_rate: u32,
    pub channels: u8,
    pub underruns: u32,
    pub overruns: u32,
}

impl AudioBuffer {
    pub fn new(format: SampleFormat, sample_rate: u32, channels: u8) -> Self {
        let frame_size = format.bytes_per_sample() * channels as usize;
        let capacity = DEFAULT_BUFFER_SIZE * frame_size;
        let mut data = Vec::new();
        data.resize(capacity, 0u8);

        AudioBuffer {
            data,
            read_pos: 0,
            write_pos: 0,
            capacity,
            state: BufferState::Empty,
            format,
            sample_rate,
            channels,
            underruns: 0,
            overruns: 0,
        }
    }

    /// How many bytes are available to read
    pub fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            self.capacity - self.read_pos + self.write_pos
        }
    }

    /// How many bytes of free space
    pub fn free_space(&self) -> usize {
        self.capacity - self.available() - 1
    }

    /// Write audio data into the buffer (from application)
    pub fn write(&mut self, data: &[u8]) -> usize {
        let space = self.free_space();
        let to_write = data.len().min(space);

        if to_write == 0 {
            self.overruns = self.overruns.saturating_add(1);
            return 0;
        }

        for i in 0..to_write {
            self.data[self.write_pos] = data[i];
            self.write_pos = (self.write_pos + 1) % self.capacity;
        }

        if self.state == BufferState::Empty {
            self.state = BufferState::Playing;
        }
        to_write
    }

    /// Read audio data from the buffer (for DMA/hardware)
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let avail = self.available();
        let to_read = buf.len().min(avail);

        if to_read == 0 {
            self.underruns = self.underruns.saturating_add(1);
            // Fill with silence
            for b in buf.iter_mut() {
                *b = 0;
            }
            return 0;
        }

        for i in 0..to_read {
            buf[i] = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % self.capacity;
        }

        if self.available() == 0 {
            self.state = BufferState::Empty;
        }
        to_read
    }

    /// Flush the buffer
    pub fn flush(&mut self) {
        self.read_pos = 0;
        self.write_pos = 0;
        self.state = BufferState::Empty;
    }

    /// Buffer latency in milliseconds
    pub fn latency_ms(&self) -> u32 {
        let frame_size = self.format.bytes_per_sample() * self.channels as usize;
        let frames = self.available() / frame_size;
        (frames as u32 * 1000) / self.sample_rate
    }
}
