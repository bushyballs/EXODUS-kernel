/// Log output sinks (serial, file, ring buffer)
///
/// Part of the AIOS logging infrastructure. Provides configurable
/// output sinks that receive formatted log records and write them
/// to serial port, file, or in-memory ring buffer.

use alloc::vec::Vec;
use crate::sync::Mutex;

/// Sink types for log output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinkType {
    Serial,
    File,
    RingBuffer,
}

/// Ring buffer for in-memory log storage
struct RingBuffer {
    data: Vec<u8>,
    capacity: usize,
    write_pos: usize,
    read_pos: usize,
    wrapped: bool,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        let cap = if capacity == 0 { 4096 } else { capacity };
        let mut data = Vec::with_capacity(cap);
        for _ in 0..cap {
            data.push(0);
        }
        Self {
            data,
            capacity: cap,
            write_pos: 0,
            read_pos: 0,
            wrapped: false,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.data[self.write_pos] = b;
            self.write_pos = (self.write_pos + 1) % self.capacity;
            if self.wrapped || self.write_pos == 0 {
                self.wrapped = true;
                self.read_pos = self.write_pos;
            }
        }
    }

    fn read_all(&self) -> Vec<u8> {
        let mut result = Vec::new();
        if self.wrapped {
            // Read from read_pos to end, then from start to write_pos
            for i in self.read_pos..self.capacity {
                result.push(self.data[i]);
            }
            for i in 0..self.write_pos {
                result.push(self.data[i]);
            }
        } else {
            for i in 0..self.write_pos {
                result.push(self.data[i]);
            }
        }
        result
    }

    fn used(&self) -> usize {
        if self.wrapped {
            self.capacity
        } else {
            self.write_pos
        }
    }

    fn clear(&mut self) {
        self.write_pos = 0;
        self.read_pos = 0;
        self.wrapped = false;
    }
}

/// A log output sink that receives formatted log records.
pub struct LogSink {
    sink_type: SinkType,
    buffer: Vec<u8>,
    ring: Option<RingBuffer>,
    enabled: bool,
    total_bytes_written: u64,
    total_records: u64,
    flush_count: u64,
    buffer_capacity: usize,
    auto_flush: bool,
}

impl LogSink {
    pub fn new(sink_type: SinkType) -> Self {
        let (ring, buffer_cap) = match sink_type {
            SinkType::RingBuffer => (Some(RingBuffer::new(65536)), 0), // 64KB ring
            SinkType::Serial => (None, 512),
            SinkType::File => (None, 4096),
        };
        crate::serial_println!("[log::sink] created {:?} sink", sink_type);
        Self {
            sink_type,
            buffer: Vec::with_capacity(buffer_cap),
            ring,
            enabled: true,
            total_bytes_written: 0,
            total_records: 0,
            flush_count: 0,
            buffer_capacity: buffer_cap,
            auto_flush: matches!(sink_type, SinkType::Serial),
        }
    }

    /// Write a log record to this sink.
    pub fn write(&mut self, data: &[u8]) {
        if !self.enabled || data.is_empty() {
            return;
        }

        self.total_records = self.total_records.saturating_add(1);
        self.total_bytes_written += data.len() as u64;

        match self.sink_type {
            SinkType::Serial => {
                // Write directly to serial output
                // In a real kernel, this writes to the UART
                for &b in data {
                    self.buffer.push(b);
                }
                if self.auto_flush || self.buffer.len() >= self.buffer_capacity {
                    self.flush();
                }
            }
            SinkType::File => {
                // Buffer for batch writing to file
                for &b in data {
                    self.buffer.push(b);
                }
                // Auto-flush when buffer is full
                if self.buffer.len() >= self.buffer_capacity {
                    self.flush();
                }
            }
            SinkType::RingBuffer => {
                // Write to ring buffer
                if let Some(ref mut ring) = self.ring {
                    ring.write(data);
                }
            }
        }
    }

    /// Flush buffered output.
    pub fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        self.flush_count = self.flush_count.saturating_add(1);

        match self.sink_type {
            SinkType::Serial => {
                // In real impl: write buffer to UART port
                // For now, use serial_println to output
                if let Ok(s) = core::str::from_utf8(&self.buffer) {
                    crate::serial_println!("[log::sink] serial: {}", s);
                }
                self.buffer.clear();
            }
            SinkType::File => {
                // In real impl: write buffer to filesystem
                let bytes_flushed = self.buffer.len();
                crate::serial_println!("[log::sink] file flush: {} bytes", bytes_flushed);
                self.buffer.clear();
            }
            SinkType::RingBuffer => {
                // Ring buffer doesn't need flushing
            }
        }
    }

    /// Enable or disable this sink
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Get the sink type
    pub fn sink_type(&self) -> SinkType {
        self.sink_type
    }

    /// Get total bytes written through this sink
    pub fn bytes_written(&self) -> u64 {
        self.total_bytes_written
    }

    /// Get total records written
    pub fn record_count(&self) -> u64 {
        self.total_records
    }

    /// Read the ring buffer contents (only for RingBuffer sinks)
    pub fn read_ring(&self) -> Option<Vec<u8>> {
        self.ring.as_ref().map(|r| r.read_all())
    }

    /// Clear the ring buffer
    pub fn clear_ring(&mut self) {
        if let Some(ref mut ring) = self.ring {
            ring.clear();
        }
    }

    /// Check if the buffer has pending data
    pub fn has_pending(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Set auto-flush mode
    pub fn set_auto_flush(&mut self, enabled: bool) {
        self.auto_flush = enabled;
    }
}

static SINKS: Mutex<Option<Vec<LogSink>>> = Mutex::new(None);

pub fn init() {
    let mut sinks = Vec::new();
    sinks.push(LogSink::new(SinkType::Serial));
    sinks.push(LogSink::new(SinkType::RingBuffer));
    let mut s = SINKS.lock();
    *s = Some(sinks);
    crate::serial_println!("[log::sink] sink subsystem initialized (serial + ring)");
}

/// Write to all active sinks
pub fn write_all(data: &[u8]) {
    let mut s = SINKS.lock();
    if let Some(ref mut sinks) = *s {
        for sink in sinks.iter_mut() {
            sink.write(data);
        }
    }
}

/// Flush all sinks
pub fn flush_all() {
    let mut s = SINKS.lock();
    if let Some(ref mut sinks) = *s {
        for sink in sinks.iter_mut() {
            sink.flush();
        }
    }
}
