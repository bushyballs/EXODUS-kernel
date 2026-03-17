// compositor/buffer_queue.rs - BufferQueue implementation (triple buffering)

use crate::compositor::types::{PixelFormat, BufferUsage};
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Maximum number of buffers in the queue
const MAX_BUFFERS: usize = 3;

/// Graphics buffer
///
/// The pixel data is owned by a heap-allocated `Vec<u8>`.  `data` is a raw
/// pointer into that Vec and is valid for the lifetime of the struct.
pub struct GraphicsBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub usage: BufferUsage,
    pub data: *mut u8,
    pub size: usize,
    pub fence: AtomicU32,  // Sync fence for GPU completion
    // Backing heap allocation — keeps the data pointer valid.
    _backing: alloc::vec::Vec<u8>,
}

impl GraphicsBuffer {
    /// Allocate a new graphics buffer backed by heap memory.
    ///
    /// Returns `Err` only when `width`, `height`, or BPP are zero (which
    /// would produce a zero-size allocation whose pointer is meaningless).
    pub fn allocate(width: u32, height: u32, format: PixelFormat, usage: BufferUsage) -> Result<Self, &'static str> {
        let bpp = format.bytes_per_pixel() as u32;
        if width == 0 || height == 0 || bpp == 0 {
            return Err("GraphicsBuffer: zero-dimension allocation");
        }
        let stride = width.saturating_mul(bpp);
        let size = (stride as usize).saturating_mul(height as usize);
        if size == 0 {
            return Err("GraphicsBuffer: computed size is zero");
        }

        // Heap-allocate pixel storage (zeroed).
        let mut backing = alloc::vec![0u8; size];
        let data = backing.as_mut_ptr();

        Ok(Self {
            width,
            height,
            stride,
            format,
            usage,
            data,
            size,
            fence: AtomicU32::new(0),
            _backing: backing,
        })
    }

    /// Map buffer for CPU access
    pub fn map(&mut self) -> Result<&mut [u8], &'static str> {
        if self.data.is_null() {
            return Err("Buffer not allocated");
        }

        // Wait for GPU fence
        while self.fence.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }

        unsafe {
            Ok(core::slice::from_raw_parts_mut(self.data, self.size))
        }
    }

    /// Unmap buffer
    pub fn unmap(&mut self) {
        // Flush CPU caches if needed
    }

    /// Set fence value (signaled when GPU is done)
    pub fn set_fence(&self, fence: u32) {
        self.fence.store(fence, Ordering::Release);
    }

    /// Wait for fence to be signaled
    pub fn wait_fence(&self) {
        while self.fence.load(Ordering::Acquire) != 0 {
            core::hint::spin_loop();
        }
    }
}

impl Drop for GraphicsBuffer {
    fn drop(&mut self) {
        // `_backing` (Vec<u8>) is dropped automatically; no manual free needed.
        // Null out the raw pointer defensively so use-after-free is obvious.
        self.data = core::ptr::null_mut();
    }
}

/// Buffer state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferState {
    Free,       // Available for dequeue
    Dequeued,   // Currently being written by producer
    Queued,     // Ready to be acquired by consumer
    Acquired,   // Currently being read by consumer
}

/// Buffer slot
struct BufferSlot {
    buffer: Option<GraphicsBuffer>,
    state: BufferState,
    frame_number: u64,
}

impl BufferSlot {
    fn new() -> Self {
        Self {
            buffer: None,
            state: BufferState::Free,
            frame_number: 0,
        }
    }
}

/// BufferQueue - Producer/Consumer buffer management
///
/// Implements Android's BufferQueue pattern with triple buffering
pub struct BufferQueue {
    slots: [BufferSlot; MAX_BUFFERS],
    width: u32,
    height: u32,
    format: PixelFormat,
    usage: BufferUsage,
    frame_counter: AtomicUsize,
    max_acquired_buffers: usize,
    current_acquired: AtomicUsize,
}

impl BufferQueue {
    /// Create a new BufferQueue
    pub fn new(width: u32, height: u32, format: PixelFormat, usage: BufferUsage) -> Self {
        Self {
            slots: [
                BufferSlot::new(),
                BufferSlot::new(),
                BufferSlot::new(),
            ],
            width,
            height,
            format,
            usage,
            frame_counter: AtomicUsize::new(0),
            max_acquired_buffers: 1,
            current_acquired: AtomicUsize::new(0),
        }
    }

    /// Dequeue a buffer for writing (producer side)
    pub fn dequeue_buffer(&mut self) -> Result<(usize, &mut GraphicsBuffer), &'static str> {
        // Find a free buffer
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == BufferState::Free {
                // Allocate buffer if needed
                if slot.buffer.is_none() {
                    slot.buffer = Some(GraphicsBuffer::allocate(
                        self.width,
                        self.height,
                        self.format,
                        self.usage,
                    )?);
                }

                slot.state = BufferState::Dequeued;
                let buffer = slot.buffer.as_mut().unwrap();
                return Ok((idx, buffer));
            }
        }

        Err("No free buffers available")
    }

    /// Queue a buffer for reading (producer done writing)
    pub fn queue_buffer(&mut self, slot_idx: usize, fence: u32) -> Result<(), &'static str> {
        if slot_idx >= MAX_BUFFERS {
            return Err("Invalid slot index");
        }

        let slot = &mut self.slots[slot_idx];
        if slot.state != BufferState::Dequeued {
            return Err("Buffer not in dequeued state");
        }

        // Set fence and update state
        if let Some(ref buffer) = slot.buffer {
            buffer.set_fence(fence);
        }

        let frame = self.frame_counter.fetch_add(1, Ordering::SeqCst) as u64;
        slot.frame_number = frame;
        slot.state = BufferState::Queued;

        Ok(())
    }

    /// Acquire a buffer for reading (consumer side)
    pub fn acquire_buffer(&mut self) -> Result<(usize, &GraphicsBuffer), &'static str> {
        // Check if we've hit the max acquired limit
        if self.current_acquired.load(Ordering::Acquire) >= self.max_acquired_buffers {
            return Err("Too many acquired buffers");
        }

        // Find the queued buffer with the highest frame number
        let mut best_idx = None;
        let mut best_frame = 0u64;

        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.state == BufferState::Queued && slot.frame_number > best_frame {
                best_idx = Some(idx);
                best_frame = slot.frame_number;
            }
        }

        if let Some(idx) = best_idx {
            let slot = &mut self.slots[idx];
            slot.state = BufferState::Acquired;
            self.current_acquired.fetch_add(1, Ordering::SeqCst);

            // Wait for fence before returning
            if let Some(ref buffer) = slot.buffer {
                buffer.wait_fence();
                return Ok((idx, buffer));
            }
        }

        Err("No queued buffers available")
    }

    /// Release a buffer (consumer done reading)
    pub fn release_buffer(&mut self, slot_idx: usize) -> Result<(), &'static str> {
        if slot_idx >= MAX_BUFFERS {
            return Err("Invalid slot index");
        }

        let slot = &mut self.slots[slot_idx];
        if slot.state != BufferState::Acquired {
            return Err("Buffer not in acquired state");
        }

        slot.state = BufferState::Free;
        self.current_acquired.fetch_sub(1, Ordering::SeqCst);

        Ok(())
    }

    /// Cancel a dequeued buffer (producer changed its mind)
    pub fn cancel_buffer(&mut self, slot_idx: usize) -> Result<(), &'static str> {
        if slot_idx >= MAX_BUFFERS {
            return Err("Invalid slot index");
        }

        let slot = &mut self.slots[slot_idx];
        if slot.state != BufferState::Dequeued {
            return Err("Buffer not in dequeued state");
        }

        slot.state = BufferState::Free;
        Ok(())
    }

    /// Resize the buffer queue
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        // Free all existing buffers
        for slot in &mut self.slots {
            slot.buffer = None;
            slot.state = BufferState::Free;
        }

        self.width = width;
        self.height = height;
    }

    /// Get current buffer dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get pixel format
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Borrow the `GraphicsBuffer` stored in the given slot (read-only).
    ///
    /// Returns `None` when the slot index is out of range or the slot has no
    /// allocated buffer.
    pub fn slot_buffer(&self, slot_idx: usize) -> Option<&GraphicsBuffer> {
        if slot_idx >= MAX_BUFFERS {
            return None;
        }
        self.slots[slot_idx].buffer.as_ref()
    }
}
