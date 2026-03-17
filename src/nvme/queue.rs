//! NVMe Queue Management
//!
//! Implements submission and completion queue ring buffers with doorbell signaling.

use super::commands::{CompletionQueueEntry, SubmissionQueueEntry};
use super::registers::NvmeRegisters;
use super::{NvmeError, Result};
use crate::dma::DmaRegion;
use core::ptr;
use core::sync::atomic::{AtomicU16, AtomicU8, Ordering};

/// Maximum queue depth
pub const MAX_QUEUE_ENTRIES: usize = 256;

/// NVMe Submission Queue
pub struct SubmissionQueue {
    pub queue_id: u16,
    pub entries: DmaRegion,
    pub size: u16,
    tail: AtomicU16,
    phase: AtomicU8,
}

impl SubmissionQueue {
    /// Create a new submission queue
    pub fn new(queue_id: u16, size: u16) -> Result<Self> {
        if size == 0 || size > MAX_QUEUE_ENTRIES as u16 {
            return Err(NvmeError::InvalidNamespace);
        }

        // Allocate DMA memory for queue entries
        let entry_size = core::mem::size_of::<SubmissionQueueEntry>();
        let total_size = entry_size * (size as usize);

        let mut entries = DmaRegion::allocate(total_size, 4096)
            .ok_or(NvmeError::InitializationFailed)?;

        // Zero the queue
        entries.zero();

        Ok(SubmissionQueue {
            queue_id,
            entries,
            size,
            tail: AtomicU16::new(0),
            phase: AtomicU8::new(1),
        })
    }

    /// Get physical address of queue
    pub fn phys_addr(&self) -> u64 {
        self.entries.phys_addr()
    }

    /// Submit a command to the queue
    pub fn submit(&self, cmd: &SubmissionQueueEntry, regs: &NvmeRegisters) -> Result<()> {
        let tail = self.tail.load(Ordering::Acquire);
        let next_tail = (tail + 1) % self.size;

        // Check if queue is full (would catch up to head)
        // In production, track head pointer from completion
        // For now, assume queue has space

        // Write command to queue
        unsafe {
            let queue_ptr = self.entries.as_ptr::<SubmissionQueueEntry>();
            ptr::write_volatile(queue_ptr.add(tail as usize), *cmd);
        }

        // Update tail pointer
        self.tail.store(next_tail, Ordering::Release);

        // Ring doorbell to notify controller
        regs.ring_submission_doorbell(self.queue_id, next_tail);

        Ok(())
    }

    /// Get current tail position
    pub fn tail(&self) -> u16 {
        self.tail.load(Ordering::Acquire)
    }
}

/// NVMe Completion Queue
pub struct CompletionQueue {
    pub queue_id: u16,
    pub entries: DmaRegion,
    pub size: u16,
    head: AtomicU16,
    phase: AtomicU8,
}

impl CompletionQueue {
    /// Create a new completion queue
    pub fn new(queue_id: u16, size: u16) -> Result<Self> {
        if size == 0 || size > MAX_QUEUE_ENTRIES as u16 {
            return Err(NvmeError::InvalidNamespace);
        }

        // Allocate DMA memory for queue entries
        let entry_size = core::mem::size_of::<CompletionQueueEntry>();
        let total_size = entry_size * (size as usize);

        let mut entries = DmaRegion::allocate(total_size, 4096)
            .ok_or(NvmeError::InitializationFailed)?;

        // Zero the queue
        entries.zero();

        Ok(CompletionQueue {
            queue_id,
            entries,
            size,
            head: AtomicU16::new(0),
            phase: AtomicU8::new(1),
        })
    }

    /// Get physical address of queue
    pub fn phys_addr(&self) -> u64 {
        self.entries.phys_addr()
    }

    /// Poll for a completion entry
    pub fn poll(&self, regs: &NvmeRegisters) -> Option<CompletionQueueEntry> {
        let head = self.head.load(Ordering::Acquire);
        let phase = self.phase.load(Ordering::Acquire);

        // Read completion entry
        let entry = unsafe {
            let queue_ptr = self.entries.as_ptr::<CompletionQueueEntry>();
            ptr::read_volatile(queue_ptr.add(head as usize))
        };

        // Check if entry is new (phase bit matches expected phase)
        if entry.phase() != (phase != 0) {
            return None; // No new completion
        }

        // Advance head pointer
        let next_head = (head + 1) % self.size;
        self.head.store(next_head, Ordering::Release);

        // Toggle phase when wrapping around
        if next_head == 0 {
            self.phase.store(1 - phase, Ordering::Release);
        }

        // Ring doorbell to notify controller of consumed entry
        regs.ring_completion_doorbell(self.queue_id, next_head);

        Some(entry)
    }

    /// Wait for a specific command to complete
    pub fn wait_for_completion(
        &self,
        regs: &NvmeRegisters,
        command_id: u16,
        timeout_ms: u32,
    ) -> Result<CompletionQueueEntry> {
        // Simple spin-wait with timeout
        // In production, this would use interrupts or a proper scheduler
        let iterations = timeout_ms * 1000; // Approximate busy-wait iterations

        for _ in 0..iterations {
            if let Some(entry) = self.poll(regs) {
                if entry.command_id() == command_id {
                    if entry.success() {
                        return Ok(entry);
                    } else {
                        return Err(NvmeError::CommandFailed);
                    }
                }
            }

            // Brief pause to avoid hammering the bus
            unsafe {
                core::arch::asm!("pause");
            }
        }

        Err(NvmeError::Timeout)
    }

    /// Get current head position
    pub fn head(&self) -> u16 {
        self.head.load(Ordering::Acquire)
    }
}

/// Queue Pair (Submission + Completion)
pub struct QueuePair {
    pub submission: SubmissionQueue,
    pub completion: CompletionQueue,
    pub queue_id: u16,
}

impl QueuePair {
    /// Create a new queue pair
    pub fn new(queue_id: u16, size: u16) -> Result<Self> {
        let submission = SubmissionQueue::new(queue_id, size)?;
        let completion = CompletionQueue::new(queue_id, size)?;

        Ok(QueuePair {
            submission,
            completion,
            queue_id,
        })
    }

    /// Submit a command and wait for completion
    pub fn submit_and_wait(
        &self,
        cmd: &SubmissionQueueEntry,
        regs: &NvmeRegisters,
        timeout_ms: u32,
    ) -> Result<CompletionQueueEntry> {
        // Extract command ID from command
        let command_id = ((cmd.cdw0 >> 16) & 0xFFFF) as u16;

        // Submit command
        self.submission.submit(cmd, regs)?;

        // Wait for completion
        self.completion.wait_for_completion(regs, command_id, timeout_ms)
    }

    /// Submit a command without waiting (async)
    pub fn submit_async(&self, cmd: &SubmissionQueueEntry, regs: &NvmeRegisters) -> Result<u16> {
        let command_id = ((cmd.cdw0 >> 16) & 0xFFFF) as u16;
        self.submission.submit(cmd, regs)?;
        Ok(command_id)
    }

    /// Poll for any completion
    pub fn poll(&self, regs: &NvmeRegisters) -> Option<CompletionQueueEntry> {
        self.completion.poll(regs)
    }
}

/// Command ID allocator
pub struct CommandIdAllocator {
    next_id: AtomicU16,
}

impl CommandIdAllocator {
    pub fn new() -> Self {
        CommandIdAllocator {
            next_id: AtomicU16::new(1),
        }
    }

    /// Allocate a new command ID
    pub fn allocate(&self) -> u16 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            // Skip 0, use 1 instead
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            id
        }
    }

    /// Reset allocator
    pub fn reset(&self) {
        self.next_id.store(1, Ordering::Relaxed);
    }
}
