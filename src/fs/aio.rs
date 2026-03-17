use crate::serial_println;
use crate::sync::Mutex;
/// Asynchronous I/O (io_uring-style) -- submission/completion ring buffers
///
/// Part of the AIOS filesystem layer.
///
/// Provides an async I/O interface modelled after Linux io_uring. Userspace
/// submits I/O operations into a submission queue; the kernel processes them
/// and posts results into a completion queue.
///
/// Design:
///   - Fixed-size submission queue (SQ) and completion queue (CQ) per instance.
///   - Opcodes: Read, Write, Fsync, Nop, Close.
///   - Each submission carries an fd, offset, length, buffer pointer, and a
///     user_data cookie returned in the completion.
///   - The kernel drains the SQ, dispatches operations, and fills the CQ.
///   - A global table maps ring IDs to instances.
///
/// Inspired by: Linux io_uring (io_uring.c). All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default queue depth (must be power of two)
const DEFAULT_QUEUE_DEPTH: usize = 128;

// ---------------------------------------------------------------------------
// Opcodes
// ---------------------------------------------------------------------------

/// I/O operation opcodes.
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum Opcode {
    Nop = 0,
    Read = 1,
    Write = 2,
    Fsync = 3,
    Close = 4,
    ReadFixed = 5,
    WriteFixed = 6,
}

impl Opcode {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Opcode::Nop),
            1 => Some(Opcode::Read),
            2 => Some(Opcode::Write),
            3 => Some(Opcode::Fsync),
            4 => Some(Opcode::Close),
            5 => Some(Opcode::ReadFixed),
            6 => Some(Opcode::WriteFixed),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Submission / Completion entries
// ---------------------------------------------------------------------------

/// A submission queue entry (SQE).
#[derive(Clone)]
pub struct SubmissionEntry {
    pub opcode: u8,
    pub flags: u8,
    pub fd: i32,
    pub offset: u64,
    pub len: u32,
    pub user_data: u64,
}

/// A completion queue entry (CQE).
#[derive(Clone, Copy)]
pub struct CompletionEntry {
    /// Echoed from the submission's user_data.
    pub user_data: u64,
    /// Result: bytes transferred on success, negative errno on error.
    pub result: i64,
    pub flags: u32,
}

// ---------------------------------------------------------------------------
// Ring instance
// ---------------------------------------------------------------------------

/// A single io_uring instance.
struct IoUringInner {
    sq: Vec<SubmissionEntry>,
    cq: Vec<CompletionEntry>,
    sq_capacity: usize,
    cq_capacity: usize,
    /// How many SQEs have been submitted but not yet processed
    sq_pending: usize,
    /// Statistics
    submitted: u64,
    completed: u64,
}

impl IoUringInner {
    fn new(depth: usize) -> Self {
        let sq_cap = depth.next_power_of_two().max(8);
        let cq_cap = sq_cap * 2; // CQ is typically 2x SQ
        IoUringInner {
            sq: Vec::new(),
            cq: Vec::new(),
            sq_capacity: sq_cap,
            cq_capacity: cq_cap,
            sq_pending: 0,
            submitted: 0,
            completed: 0,
        }
    }

    /// Submit an I/O operation into the submission queue.
    fn submit(&mut self, entry: SubmissionEntry) -> Result<(), i32> {
        if self.sq.len() >= self.sq_capacity {
            return Err(-1); // SQ full
        }
        // Validate opcode
        if Opcode::from_u8(entry.opcode).is_none() {
            return Err(-2); // Invalid opcode
        }
        self.sq.push(entry);
        self.sq_pending = self.sq_pending.saturating_add(1);
        self.submitted = self.submitted.saturating_add(1);
        Ok(())
    }

    /// Process all pending submissions, generating completions.
    /// In a real kernel this would dispatch actual I/O; here we simulate
    /// the pipeline by generating completion entries.
    fn process(&mut self) {
        while let Some(sqe) = self.sq.pop() {
            let result = self.dispatch(&sqe);
            if self.cq.len() < self.cq_capacity {
                self.cq.push(CompletionEntry {
                    user_data: sqe.user_data,
                    result,
                    flags: 0,
                });
                self.completed = self.completed.saturating_add(1);
            }
            self.sq_pending = self.sq_pending.saturating_sub(1);
        }
    }

    /// Dispatch a single SQE and return the result.
    fn dispatch(&self, sqe: &SubmissionEntry) -> i64 {
        match Opcode::from_u8(sqe.opcode) {
            Some(Opcode::Nop) => 0,
            Some(Opcode::Read) => {
                // Simulated: return the requested length as "bytes read"
                // A real implementation would call into the VFS read path.
                if sqe.fd < 0 {
                    -9 // EBADF
                } else {
                    sqe.len as i64
                }
            }
            Some(Opcode::Write) => {
                if sqe.fd < 0 {
                    -9 // EBADF
                } else {
                    sqe.len as i64
                }
            }
            Some(Opcode::Fsync) => {
                if sqe.fd < 0 {
                    -9
                } else {
                    0
                }
            }
            Some(Opcode::Close) => {
                if sqe.fd < 0 {
                    -9
                } else {
                    0
                }
            }
            Some(Opcode::ReadFixed) | Some(Opcode::WriteFixed) => {
                if sqe.fd < 0 {
                    -9
                } else {
                    sqe.len as i64
                }
            }
            None => -22, // EINVAL
        }
    }

    /// Poll completions from the completion queue (up to max).
    fn poll_completions(&mut self, max: usize) -> Vec<CompletionEntry> {
        let take = max.min(self.cq.len());
        self.cq.drain(..take).collect()
    }

    /// Peek at how many completions are available without consuming them.
    fn cq_ready(&self) -> usize {
        self.cq.len()
    }

    /// Return how many SQ slots are available.
    fn sq_space(&self) -> usize {
        self.sq_capacity.saturating_sub(self.sq.len())
    }
}

// ---------------------------------------------------------------------------
// Global table
// ---------------------------------------------------------------------------

struct AioTable {
    rings: Vec<Option<IoUringInner>>,
    next_id: usize,
}

impl AioTable {
    fn new() -> Self {
        AioTable {
            rings: Vec::new(),
            next_id: 0,
        }
    }

    fn create(&mut self, depth: usize) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        if id < self.rings.len() {
            self.rings[id] = Some(IoUringInner::new(depth));
        } else {
            self.rings.push(Some(IoUringInner::new(depth)));
        }
        id
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut IoUringInner> {
        self.rings.get_mut(id).and_then(|s| s.as_mut())
    }

    fn destroy(&mut self, id: usize) {
        if id < self.rings.len() {
            self.rings[id] = None;
        }
    }
}

static AIO_TABLE: Mutex<Option<AioTable>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new io_uring instance with the given queue depth.
pub fn io_uring_setup(queue_depth: usize) -> Result<usize, i32> {
    let mut guard = AIO_TABLE.lock();
    match guard.as_mut() {
        Some(table) => Ok(table.create(queue_depth)),
        None => Err(-1),
    }
}

/// Submit an I/O operation.
pub fn io_uring_submit(ring_id: usize, entry: SubmissionEntry) -> Result<(), i32> {
    let mut guard = AIO_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let ring = table.get_mut(ring_id).ok_or(-2)?;
    ring.submit(entry)
}

/// Process pending submissions (called by kernel I/O worker or on enter).
pub fn io_uring_enter(ring_id: usize) -> Result<(), i32> {
    let mut guard = AIO_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let ring = table.get_mut(ring_id).ok_or(-2)?;
    ring.process();
    Ok(())
}

/// Poll for completed operations.
pub fn io_uring_poll(ring_id: usize, max: usize) -> Result<Vec<CompletionEntry>, i32> {
    let mut guard = AIO_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let ring = table.get_mut(ring_id).ok_or(-2)?;
    Ok(ring.poll_completions(max))
}

/// Check how many completions are ready.
pub fn io_uring_cq_ready(ring_id: usize) -> usize {
    let mut guard = AIO_TABLE.lock();
    guard
        .as_mut()
        .and_then(|t| t.get_mut(ring_id))
        .map_or(0, |r| r.cq_ready())
}

/// Check available SQ space.
pub fn io_uring_sq_space(ring_id: usize) -> usize {
    let mut guard = AIO_TABLE.lock();
    guard
        .as_mut()
        .and_then(|t| t.get_mut(ring_id))
        .map_or(0, |r| r.sq_space())
}

/// Destroy an io_uring instance.
pub fn io_uring_close(ring_id: usize) {
    let mut guard = AIO_TABLE.lock();
    if let Some(table) = guard.as_mut() {
        table.destroy(ring_id);
    }
}

/// Initialize the async I/O subsystem.
pub fn init() {
    let mut guard = AIO_TABLE.lock();
    *guard = Some(AioTable::new());
    serial_println!("    aio: initialized (io_uring-style submission/completion rings)");
}
