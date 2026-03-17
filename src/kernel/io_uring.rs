/// io_uring — Linux io_uring-compatible async I/O interface
///
/// Implements the core submission/completion ring buffer model:
///   - Submission Queue (SQ): producer is userspace, consumer is kernel
///   - Completion Queue (CQ): producer is kernel, consumer is userspace
///   - Fixed-size ring buffers (SQ_SIZE / CQ_SIZE entries)
///   - Operations: read, write, fsync, nop, timeout, close
///
/// In this kernel stub, "userspace" = any internal caller submitting SQEs.
/// io_uring_enter() processes all pending SQEs and posts CQEs.
///
/// Inspired by: Linux io_uring (fs/io_uring.c, io_uring/). All code original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SQ_SIZE: usize = 256; // must be power of two
const CQ_SIZE: usize = 512; // must be >= 2×SQ_SIZE
const SQ_MASK: usize = SQ_SIZE - 1;
const CQ_MASK: usize = CQ_SIZE - 1;

const MAX_RINGS: usize = 8; // simultaneously open io_uring instances

// io_uring opcodes (subset matching Linux IORING_OP_*)
pub const IORING_OP_NOP: u8 = 0;
pub const IORING_OP_READV: u8 = 1;
pub const IORING_OP_WRITEV: u8 = 2;
pub const IORING_OP_FSYNC: u8 = 3;
pub const IORING_OP_READ: u8 = 22;
pub const IORING_OP_WRITE: u8 = 23;
pub const IORING_OP_CLOSE: u8 = 19;
pub const IORING_OP_TIMEOUT: u8 = 11;

// io_uring setup flags
pub const IORING_SETUP_SQPOLL: u32 = 0x0002;
pub const IORING_SETUP_IOPOLL: u32 = 0x0001;

// Completion result codes (negative errno on error)
pub const IORING_CQE_OK: i32 = 0;
pub const IORING_CQE_ERR: i32 = -1;

// ---------------------------------------------------------------------------
// Submission Queue Entry (SQE)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct IoUringSqe {
    pub opcode: u8,
    pub flags: u8,
    pub ioprio: u16,
    pub fd: i32,
    pub off: u64,  // file offset or timeout (ns)
    pub addr: u64, // buffer address (kernel pointer)
    pub len: u32,  // buffer length
    pub rw_flags: u32,
    pub user_data: u64, // opaque tag returned in CQE
    pub pad: [u64; 3],
}

impl IoUringSqe {
    pub const fn zero() -> Self {
        IoUringSqe {
            opcode: 0,
            flags: 0,
            ioprio: 0,
            fd: -1,
            off: 0,
            addr: 0,
            len: 0,
            rw_flags: 0,
            user_data: 0,
            pad: [0u64; 3],
        }
    }
}

// ---------------------------------------------------------------------------
// Completion Queue Entry (CQE)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct IoUringCqe {
    pub user_data: u64,
    pub res: i32, // result (bytes transferred, or negative errno)
    pub flags: u32,
}

impl IoUringCqe {
    pub const fn zero() -> Self {
        IoUringCqe {
            user_data: 0,
            res: 0,
            flags: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring instance
// ---------------------------------------------------------------------------

struct IoUringRing {
    pub id: u32,
    pub flags: u32,
    // Submission queue
    sq: [IoUringSqe; SQ_SIZE],
    sq_head: u32, // kernel read pointer
    sq_tail: u32, // userspace write pointer
    // Completion queue
    cq: [IoUringCqe; CQ_SIZE],
    cq_head: u32, // userspace read pointer
    cq_tail: u32, // kernel write pointer
    pub active: bool,
    pub submitted: u64,
    pub completed: u64,
}

impl IoUringRing {
    const fn new() -> Self {
        const ZERO_SQE: IoUringSqe = IoUringSqe::zero();
        const ZERO_CQE: IoUringCqe = IoUringCqe::zero();
        IoUringRing {
            id: 0,
            flags: 0,
            sq: [ZERO_SQE; SQ_SIZE],
            sq_head: 0,
            sq_tail: 0,
            cq: [ZERO_CQE; CQ_SIZE],
            cq_head: 0,
            cq_tail: 0,
            active: false,
            submitted: 0,
            completed: 0,
        }
    }

    fn sq_pending(&self) -> u32 {
        self.sq_tail.wrapping_sub(self.sq_head)
    }

    fn cq_space(&self) -> u32 {
        (CQ_SIZE as u32).saturating_sub(self.cq_tail.wrapping_sub(self.cq_head))
    }

    fn post_cqe(&mut self, user_data: u64, res: i32) {
        if self.cq_space() == 0 {
            return;
        } // CQ overflow — drop entry
        let idx = self.cq_tail as usize & CQ_MASK;
        self.cq[idx] = IoUringCqe {
            user_data,
            res,
            flags: 0,
        };
        self.cq_tail = self.cq_tail.wrapping_add(1);
        self.completed = self.completed.saturating_add(1);
    }

    /// Process all pending SQEs. Returns number processed.
    fn process_sqes(&mut self) -> u32 {
        let mut count = 0u32;
        while self.sq_head != self.sq_tail {
            let idx = self.sq_head as usize & SQ_MASK;
            let sqe = self.sq[idx];
            self.sq_head = self.sq_head.wrapping_add(1);
            self.submitted = self.submitted.saturating_add(1);

            // Dispatch operation
            let res = match sqe.opcode {
                IORING_OP_NOP => IORING_CQE_OK,
                IORING_OP_CLOSE => {
                    // Would call fd::close(sqe.fd) — stub returns ok
                    IORING_CQE_OK
                }
                IORING_OP_FSYNC => {
                    // Would flush dirty pages — stub ok
                    IORING_CQE_OK
                }
                IORING_OP_READ | IORING_OP_READV => {
                    // Would dispatch to VFS read — return sqe.len as "bytes read"
                    sqe.len as i32
                }
                IORING_OP_WRITE | IORING_OP_WRITEV => {
                    // Would dispatch to VFS write — return sqe.len as "bytes written"
                    sqe.len as i32
                }
                IORING_OP_TIMEOUT => {
                    // Would arm a timerfd — stub ok
                    IORING_CQE_OK
                }
                _ => -22, // EINVAL
            };

            self.post_cqe(sqe.user_data, res);
            count = count.saturating_add(1);
        }
        count
    }
}

// unsafe impl because Ring contains raw pointers (simulated)
unsafe impl Send for IoUringRing {}

// ---------------------------------------------------------------------------
// Global ring table
// ---------------------------------------------------------------------------

const fn ring_array() -> [IoUringRing; MAX_RINGS] {
    // Can't use [IoUringRing::new(); MAX_RINGS] because IoUringRing is not Copy.
    // Manually expand.
    [
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
        IoUringRing::new(),
    ]
}

static RINGS: Mutex<[IoUringRing; MAX_RINGS]> = Mutex::new(ring_array());
static RING_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new io_uring instance. Returns ring id, or 0 on failure.
pub fn io_uring_setup(flags: u32) -> u32 {
    let id = RING_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if !rings[i].active {
            rings[i] = IoUringRing::new();
            rings[i].id = id;
            rings[i].flags = flags;
            rings[i].active = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Submit an SQE to a ring. Returns true if queued.
pub fn io_uring_submit_sqe(ring_id: u32, sqe: IoUringSqe) -> bool {
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            let pending = rings[i].sq_pending();
            if pending as usize >= SQ_SIZE {
                return false;
            } // SQ full
            let idx = rings[i].sq_tail as usize & SQ_MASK;
            rings[i].sq[idx] = sqe;
            rings[i].sq_tail = rings[i].sq_tail.wrapping_add(1);
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Process all pending SQEs for a ring. Returns number of completions posted.
pub fn io_uring_enter(ring_id: u32) -> u32 {
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            return rings[i].process_sqes();
        }
        i = i.saturating_add(1);
    }
    0
}

/// Peek at the next CQE without consuming it. Returns Some(cqe) or None.
pub fn io_uring_peek_cqe(ring_id: u32) -> Option<IoUringCqe> {
    let rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            if rings[i].cq_head == rings[i].cq_tail {
                return None;
            }
            let idx = rings[i].cq_head as usize & CQ_MASK;
            return Some(rings[i].cq[idx]);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Consume (advance past) the head CQE.
pub fn io_uring_seen_cqe(ring_id: u32) {
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            if rings[i].cq_head != rings[i].cq_tail {
                rings[i].cq_head = rings[i].cq_head.wrapping_add(1);
            }
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Drain up to 8 CQEs at once.
pub fn io_uring_drain_cqes(ring_id: u32, out: &mut [IoUringCqe; 8]) -> usize {
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            let avail = rings[i].cq_tail.wrapping_sub(rings[i].cq_head);
            let count = (avail as usize).min(8);
            let mut k = 0usize;
            while k < count {
                let idx = rings[i].cq_head as usize & CQ_MASK;
                out[k] = rings[i].cq[idx];
                rings[i].cq_head = rings[i].cq_head.wrapping_add(1);
                k = k.saturating_add(1);
            }
            return count;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Close a ring and free its slot.
pub fn io_uring_close(ring_id: u32) -> bool {
    let mut rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            rings[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Query pending SQE / available CQE counts.
pub fn io_uring_stats(ring_id: u32) -> Option<(u32, u32, u64, u64)> {
    let rings = RINGS.lock();
    let mut i = 0usize;
    while i < MAX_RINGS {
        if rings[i].active && rings[i].id == ring_id {
            return Some((
                rings[i].sq_pending(),
                rings[i].cq_tail.wrapping_sub(rings[i].cq_head),
                rings[i].submitted,
                rings[i].completed,
            ));
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn init() {
    serial_println!(
        "[io_uring] io_uring async I/O subsystem ready (SQ={}, CQ={}, max {} rings)",
        SQ_SIZE,
        CQ_SIZE,
        MAX_RINGS
    );
}
