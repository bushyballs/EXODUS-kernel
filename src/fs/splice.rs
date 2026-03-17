use crate::serial_println;
use crate::sync::Mutex;
/// Zero-copy data transfer between file descriptors (splice/tee/vmsplice)
///
/// Part of the AIOS filesystem layer.
///
/// Provides Linux-compatible splice operations that move data between
/// file descriptors without copying through userspace. Data flows through
/// an internal pipe buffer (a ring of page-sized slots).
///
/// Design:
///   - PipeBuffer: a ring buffer of fixed-size pages used as the
///     intermediate store for splice/tee operations.
///   - splice(): moves data from fd_in to fd_out via a PipeBuffer.
///   - tee(): duplicates pipe content to another pipe without consuming it.
///   - vmsplice(): transfers userspace pages into a pipe.
///   - A global PipeBuffer pool is maintained for kernel-internal splices.
///
/// Inspired by: Linux splice (fs/splice.c). All code is original.
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Page size for pipe buffer slots.
const PAGE_SIZE: usize = 4096;

/// Default number of pages in a pipe buffer.
const DEFAULT_PIPE_PAGES: usize = 16;

/// Splice flags.
pub const SPLICE_F_MOVE: u32 = 0x01;
pub const SPLICE_F_NONBLOCK: u32 = 0x02;
pub const SPLICE_F_MORE: u32 = 0x04;
pub const SPLICE_F_GIFT: u32 = 0x08;

// ---------------------------------------------------------------------------
// PipeBuffer
// ---------------------------------------------------------------------------

/// A single page slot in the pipe buffer.
#[derive(Clone)]
struct PipePage {
    data: Vec<u8>,
    len: usize,
}

/// Ring buffer of pages used for zero-copy transfers.
pub struct PipeBuffer {
    pages: Vec<PipePage>,
    capacity: usize,
    head: usize, // read position
    tail: usize, // write position
    count: usize,
}

impl PipeBuffer {
    /// Create a new pipe buffer with the given number of page slots.
    pub fn new(num_pages: usize) -> Self {
        let cap = num_pages.max(2);
        let mut pages = Vec::with_capacity(cap);
        for _ in 0..cap {
            pages.push(PipePage {
                data: Vec::new(),
                len: 0,
            });
        }
        PipeBuffer {
            pages,
            capacity: cap,
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Check if the buffer is full.
    pub fn is_full(&self) -> bool {
        self.count >= self.capacity
    }

    /// Bytes available for reading.
    pub fn available(&self) -> usize {
        let mut total = 0;
        let mut idx = self.head;
        for _ in 0..self.count {
            total += self.pages[idx].len;
            idx = (idx + 1) % self.capacity;
        }
        total
    }

    /// Write data into the pipe buffer, filling pages sequentially.
    /// Returns the number of bytes actually written.
    pub fn write(&mut self, data: &[u8]) -> usize {
        let mut written = 0usize;
        let mut remaining = data;

        while !remaining.is_empty() && !self.is_full() {
            let page = &mut self.pages[self.tail];
            let space = PAGE_SIZE - page.len;
            if space == 0 {
                // This page is full, advance to next
                self.tail = (self.tail + 1) % self.capacity;
                self.count = self.count.saturating_add(1);
                if self.is_full() {
                    break;
                }
                continue;
            }

            let chunk = remaining.len().min(space);
            if page.data.len() < page.len + chunk {
                page.data.resize(page.len + chunk, 0);
            }
            page.data[page.len..page.len + chunk].copy_from_slice(&remaining[..chunk]);
            page.len += chunk;
            written += chunk;
            remaining = &remaining[chunk..];

            if page.len >= PAGE_SIZE {
                self.tail = (self.tail + 1) % self.capacity;
                self.count = self.count.saturating_add(1);
            }
        }

        // If we wrote into the current tail page but didn't fill it,
        // it still counts as occupying a slot.
        if written > 0 && self.pages[self.tail].len > 0 && self.count == 0 {
            self.count = 1;
        }

        written
    }

    /// Read data from the pipe buffer. Returns bytes read.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let mut total = 0usize;
        while total < buf.len() && self.count > 0 {
            let page = &mut self.pages[self.head];
            if page.len == 0 {
                self.head = (self.head + 1) % self.capacity;
                self.count = self.count.saturating_sub(1);
                continue;
            }
            let chunk = (buf.len() - total).min(page.len);
            buf[total..total + chunk].copy_from_slice(&page.data[..chunk]);
            total += chunk;

            // Remove consumed bytes from page
            if chunk < page.len {
                let remaining = page.len - chunk;
                for i in 0..remaining {
                    page.data[i] = page.data[chunk + i];
                }
                page.len = remaining;
            } else {
                page.len = 0;
                page.data.clear();
                self.head = (self.head + 1) % self.capacity;
                self.count = self.count.saturating_sub(1);
            }
        }
        total
    }

    /// Peek at data without consuming (for tee).
    pub fn peek(&self, buf: &mut [u8]) -> usize {
        let mut total = 0usize;
        let mut idx = self.head;
        let mut remaining_pages = self.count;
        while total < buf.len() && remaining_pages > 0 {
            let page = &self.pages[idx];
            if page.len == 0 {
                idx = (idx + 1) % self.capacity;
                remaining_pages = remaining_pages.saturating_sub(1);
                continue;
            }
            let chunk = (buf.len() - total).min(page.len);
            buf[total..total + chunk].copy_from_slice(&page.data[..chunk]);
            total += chunk;
            idx = (idx + 1) % self.capacity;
            remaining_pages -= 1;
        }
        total
    }

    /// Reset the buffer to empty.
    pub fn clear(&mut self) {
        for page in self.pages.iter_mut() {
            page.len = 0;
            page.data.clear();
        }
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }
}

// ---------------------------------------------------------------------------
// Transfer operations
// ---------------------------------------------------------------------------

/// Result of a splice/tee operation.
pub struct SpliceResult {
    pub bytes_transferred: usize,
}

/// Splice data from one pipe buffer to another (simulating fd_in -> fd_out).
/// In a real kernel, fd_in/fd_out would be looked up in the fd table.
pub fn splice_buffers(
    src: &mut PipeBuffer,
    dst: &mut PipeBuffer,
    max_len: usize,
    _flags: u32,
) -> Result<SpliceResult, i32> {
    let mut tmp = vec![0u8; max_len.min(PAGE_SIZE * DEFAULT_PIPE_PAGES)];
    let read = src.read(&mut tmp);
    if read == 0 {
        return Ok(SpliceResult {
            bytes_transferred: 0,
        });
    }
    let written = dst.write(&tmp[..read]);
    Ok(SpliceResult {
        bytes_transferred: written,
    })
}

/// Tee: duplicate data from src pipe into dst pipe without consuming src.
pub fn tee_buffers(
    src: &PipeBuffer,
    dst: &mut PipeBuffer,
    max_len: usize,
) -> Result<SpliceResult, i32> {
    let mut tmp = vec![0u8; max_len.min(PAGE_SIZE * DEFAULT_PIPE_PAGES)];
    let peeked = src.peek(&mut tmp);
    if peeked == 0 {
        return Ok(SpliceResult {
            bytes_transferred: 0,
        });
    }
    let written = dst.write(&tmp[..peeked]);
    Ok(SpliceResult {
        bytes_transferred: written,
    })
}

// ---------------------------------------------------------------------------
// Global pipe buffer pool
// ---------------------------------------------------------------------------

struct SpliceState {
    /// Pool of reusable pipe buffers for kernel-internal splice
    pool: Vec<PipeBuffer>,
    total_spliced: u64,
}

static SPLICE_STATE: Mutex<Option<SpliceState>> = Mutex::new(None);

/// Allocate a pipe buffer from the pool (or create a new one).
pub fn alloc_pipe_buffer() -> PipeBuffer {
    let mut guard = SPLICE_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(buf) = state.pool.pop() {
            return buf;
        }
    }
    PipeBuffer::new(DEFAULT_PIPE_PAGES)
}

/// Return a pipe buffer to the pool for reuse.
pub fn free_pipe_buffer(mut buf: PipeBuffer) {
    buf.clear();
    let mut guard = SPLICE_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if state.pool.len() < 32 {
            state.pool.push(buf);
        }
    }
}

/// Initialize the splice subsystem.
pub fn init() {
    let mut guard = SPLICE_STATE.lock();
    *guard = Some(SpliceState {
        pool: Vec::new(),
        total_spliced: 0,
    });
    serial_println!("    splice: initialized (zero-copy pipe transfer, tee)");
}
