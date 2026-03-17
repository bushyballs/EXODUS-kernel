/// POSIX Real-Time Signals — per-process queued RT signal delivery.
///
/// Real-time signals (SIGRTMIN..SIGRTMAX, Linux 34-64) differ from standard
/// signals in three key ways:
///
///   1. **Queuing** — multiple identical RT signals queue rather than
///      collapsing to a single pending bit.
///   2. **Priority** — lowest signal number is delivered first (POSIX).
///   3. **siginfo_t** — RT signals carry a full `Siginfo` payload with
///      sender PID/UID and a user-supplied value (`sival_int` / `sival_ptr`).
///
/// This module owns:
///   - `Siginfo`      — kernel-internal siginfo_t
///   - `SigrtQueue`   — per-process circular buffer for each RT signal
///   - `RT_QUEUES`    — static table, one `SigrtQueue` per tracked process
///   - Public API functions: `sigrt_alloc_queue`, `sigrt_free_queue`,
///     `sigrt_send`, `sigrt_dequeue`, `sigrt_block`, `sigrt_unblock`,
///     `sigrt_get_pending`, `sigrt_deliver_pending`
///
/// RULES (no violations or the kernel panics):
///   - No heap (`Vec`, `Box`, `String`, `alloc::*`)
///   - No float casts (`as f32`, `as f64`)
///   - No `unwrap()`, `expect()`, `panic!()`
///   - All counters: `saturating_add` / `saturating_sub`
///   - All sequence numbers: `wrapping_add`
///   - MMIO: `read_volatile` / `write_volatile` only
///   - Every struct inside a `static Mutex` must be `Copy` with `const fn empty()`
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// RT signal range constants
// ---------------------------------------------------------------------------

/// First real-time signal number (matches Linux).
pub const SIGRTMIN: u32 = 34;
/// Last real-time signal number (matches Linux).
pub const SIGRTMAX: u32 = 64;
/// Number of distinct real-time signals.
pub const SIGRT_COUNT: usize = (SIGRTMAX - SIGRTMIN + 1) as usize; // 31

/// Per-signal queue depth.  POSIX minimum is 8; we use 32.
pub const SIGRT_QUEUE_DEPTH: usize = 32;

/// Maximum number of processes whose RT queues this module tracks simultaneously.
pub const SIGRT_MAX_PROCS: usize = 64;

// ---------------------------------------------------------------------------
// SI_CODE values used in Siginfo::si_code
// ---------------------------------------------------------------------------

pub const SI_USER: i32 = 0; // kill(2) / raise(3)
pub const SI_QUEUE: i32 = -1; // sigqueue(3)
pub const SI_TIMER: i32 = -2; // POSIX timer expiration
pub const SI_MESGQ: i32 = -3; // Message-queue notification
pub const SI_ASYNCIO: i32 = -4; // AIO completion

// ---------------------------------------------------------------------------
// Siginfo — kernel-internal siginfo_t
// ---------------------------------------------------------------------------

/// Full POSIX siginfo_t equivalent carried with every RT signal.
///
/// Stored inside `SigrtQueue` circular buffers.  Must be `Copy` (no heap).
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Siginfo {
    /// Signal number (`si_signo`).
    pub si_signo: u32,
    /// Errno associated with the signal, or 0 (`si_errno`).
    pub si_errno: i32,
    /// Signal code — `SI_USER`, `SI_QUEUE`, `SI_TIMER`, … (`si_code`).
    pub si_code: i32,
    /// PID of the sending process (`si_pid`).
    pub si_pid: u32,
    /// UID of the sending process (`si_uid`).
    pub si_uid: u32,
    /// Integer payload from `sigqueue` / `sigev_value.sival_int` (`si_value.sival_int`).
    pub si_value_int: i32,
    /// Pointer payload from `sigqueue` / `sigev_value.sival_ptr` (`si_value.sival_ptr`).
    pub si_value_ptr: u64,
    /// Exit status or signal that caused child to stop / exit (`SIGCHLD`).
    pub si_status: i32,
    /// Fault address for memory-fault signals (`SIGSEGV`, `SIGBUS`).
    pub si_addr: u64,
    /// POSIX timer ID for `SI_TIMER` signals (`si_timerid`).
    pub si_timerid: i32,
    /// Timer overrun count for `SI_TIMER` signals (`si_overrun`).
    pub si_overrun: i32,
}

impl Siginfo {
    /// Construct a zeroed `Siginfo`.
    pub const fn empty() -> Self {
        Siginfo {
            si_signo: 0,
            si_errno: 0,
            si_code: SI_USER,
            si_pid: 0,
            si_uid: 0,
            si_value_int: 0,
            si_value_ptr: 0,
            si_status: 0,
            si_addr: 0,
            si_timerid: 0,
            si_overrun: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// SigrtQueue — per-process RT signal queues
// ---------------------------------------------------------------------------

/// Per-process state for real-time signals.
///
/// `queues[n]` holds a circular buffer for RT signal `SIGRTMIN + n`.
/// `heads[n]` / `tails[n]` are indices into that buffer (mod SIGRT_QUEUE_DEPTH).
/// A signal `SIGRTMIN + n` is "pending" when `heads[n] != tails[n]`.
#[derive(Copy, Clone)]
pub struct SigrtQueue {
    /// Owner process PID (0 = slot free).
    pub pid: u32,
    /// Slot is in use.
    pub active: bool,
    /// Circular buffers — one per real-time signal.
    pub queues: [[Siginfo; SIGRT_QUEUE_DEPTH]; SIGRT_COUNT],
    /// Read pointers (next entry to dequeue) for each signal.
    pub heads: [u8; SIGRT_COUNT],
    /// Write pointers (next entry to enqueue) for each signal.
    pub tails: [u8; SIGRT_COUNT],
    /// Bitmask of blocked RT signals.  Bit n=1 means `SIGRTMIN + n` is blocked.
    pub rt_blocked: u32,
    /// Bitmask of pending RT signals (set when enqueued, cleared when queue drains).
    pub rt_pending: u32,
}

impl SigrtQueue {
    /// Construct an empty / inactive `SigrtQueue`.
    pub const fn empty() -> Self {
        SigrtQueue {
            pid: 0,
            active: false,
            queues: [[Siginfo::empty(); SIGRT_QUEUE_DEPTH]; SIGRT_COUNT],
            heads: [0u8; SIGRT_COUNT],
            tails: [0u8; SIGRT_COUNT],
            rt_blocked: 0,
            rt_pending: 0,
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Number of entries currently in the queue for signal index `idx`.
    #[inline]
    fn queue_len(&self, idx: usize) -> usize {
        let h = self.heads[idx] as usize;
        let t = self.tails[idx] as usize;
        if t >= h {
            t - h
        } else {
            SIGRT_QUEUE_DEPTH - h + t
        }
    }

    /// Enqueue one `Siginfo` for signal index `idx`.
    /// Returns `false` if the queue is full.
    fn push(&mut self, idx: usize, info: Siginfo) -> bool {
        if self.queue_len(idx) >= SIGRT_QUEUE_DEPTH.saturating_sub(1) {
            return false; // full
        }
        let tail = self.tails[idx] as usize;
        self.queues[idx][tail] = info;
        self.tails[idx] = ((tail.wrapping_add(1)) % SIGRT_QUEUE_DEPTH) as u8;
        // Mark signal pending in bitmask.
        if idx < 32 {
            self.rt_pending |= 1u32 << idx;
        }
        true
    }

    /// Dequeue the oldest `Siginfo` for signal index `idx`.
    /// Clears the pending bit if the queue is now empty.
    fn pop(&mut self, idx: usize) -> Option<Siginfo> {
        if self.heads[idx] == self.tails[idx] {
            return None; // empty
        }
        let head = self.heads[idx] as usize;
        let info = self.queues[idx][head];
        self.heads[idx] = ((head.wrapping_add(1)) % SIGRT_QUEUE_DEPTH) as u8;
        // Clear pending bit if queue is now empty.
        if self.heads[idx] == self.tails[idx] {
            if idx < 32 {
                self.rt_pending &= !(1u32 << idx);
            }
        }
        Some(info)
    }

    /// Return `true` if signal index `idx` has at least one queued entry.
    #[inline]
    fn has_entry(&self, idx: usize) -> bool {
        self.heads[idx] != self.tails[idx]
    }
}

// ---------------------------------------------------------------------------
// Global table
// ---------------------------------------------------------------------------

static RT_QUEUES: Mutex<[SigrtQueue; SIGRT_MAX_PROCS]> =
    Mutex::new([const { SigrtQueue::empty() }; SIGRT_MAX_PROCS]);

// ---------------------------------------------------------------------------
// Internal table helpers
// ---------------------------------------------------------------------------

/// Find the slot index for `pid`.  Returns `None` if not found.
fn find_slot(table: &[SigrtQueue; SIGRT_MAX_PROCS], pid: u32) -> Option<usize> {
    table.iter().position(|q| q.active && q.pid == pid)
}

/// Find or create a slot for `pid`.  Returns `None` if the table is full.
fn find_or_alloc(table: &mut [SigrtQueue; SIGRT_MAX_PROCS], pid: u32) -> Option<usize> {
    // Try existing slot first.
    if let Some(i) = table.iter().position(|q| q.active && q.pid == pid) {
        return Some(i);
    }
    // Allocate a free slot.
    if let Some(i) = table.iter().position(|q| !q.active) {
        table[i] = SigrtQueue::empty();
        table[i].pid = pid;
        table[i].active = true;
        return Some(i);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Allocate a real-time signal queue for `pid`.
///
/// Idempotent: returns `true` even if a queue already exists for `pid`.
/// Returns `false` only when the global table is full.
pub fn sigrt_alloc_queue(pid: u32) -> bool {
    let mut table = RT_QUEUES.lock();
    find_or_alloc(&mut table, pid).is_some()
}

/// Release the RT signal queue belonging to `pid`.
///
/// The slot is zeroed and marked inactive so it can be reused.
pub fn sigrt_free_queue(pid: u32) {
    let mut table = RT_QUEUES.lock();
    if let Some(i) = find_slot(&table, pid) {
        table[i] = SigrtQueue::empty();
    }
}

/// Send real-time signal `signo` to `target_pid`, carrying `info`.
///
/// # Returns
/// *  `0`   — signal enqueued successfully.
/// * `-11`  — `EAGAIN`: queue is full (POSIX-defined overrun behaviour).
/// * `-22`  — `EINVAL`: signal number out of `[SIGRTMIN, SIGRTMAX]`.
/// * `-3`   — `ESRCH`: process not found and table is full.
pub fn sigrt_send(target_pid: u32, signo: u32, info: Siginfo) -> i32 {
    if signo < SIGRTMIN || signo > SIGRTMAX {
        return -22; // EINVAL
    }
    let idx = (signo - SIGRTMIN) as usize;

    let mut table = RT_QUEUES.lock();
    match find_or_alloc(&mut table, target_pid) {
        None => -3, // ESRCH — table full
        Some(slot) => {
            if table[slot].push(idx, info) {
                0
            } else {
                -11 // EAGAIN — queue full
            }
        }
    }
}

/// Return `true` if `signo` is pending (queued and not blocked) for `pid`.
pub fn sigrt_is_pending(pid: u32, signo: u32) -> bool {
    if signo < SIGRTMIN || signo > SIGRTMAX {
        return false;
    }
    let idx = (signo - SIGRTMIN) as usize;
    let table = RT_QUEUES.lock();
    match find_slot(&table, pid) {
        None => false,
        Some(slot) => {
            let q = &table[slot];
            let blocked = if idx < 32 {
                (q.rt_blocked >> idx) & 1 != 0
            } else {
                false
            };
            !blocked && q.has_entry(idx)
        }
    }
}

/// Dequeue the next `Siginfo` for `signo` from `pid`'s queue.
///
/// Clears the pending bitmask bit for `signo` if the queue drains.
/// Returns `None` if the queue is empty or `signo` is invalid.
pub fn sigrt_dequeue(pid: u32, signo: u32) -> Option<Siginfo> {
    if signo < SIGRTMIN || signo > SIGRTMAX {
        return None;
    }
    let idx = (signo - SIGRTMIN) as usize;
    let mut table = RT_QUEUES.lock();
    let slot = find_slot(&table, pid)?;
    table[slot].pop(idx)
}

/// Add signals in `mask` to the blocked set for `pid`.
///
/// `mask` is a bitmask over the RT signal range: bit `n` corresponds to
/// `SIGRTMIN + n`.  Signals already blocked are unaffected.
pub fn sigrt_block(pid: u32, mask: u32) {
    let mut table = RT_QUEUES.lock();
    if let Some(slot) = find_slot(&table, pid) {
        table[slot].rt_blocked |= mask;
    }
}

/// Remove signals in `mask` from the blocked set for `pid`.
///
/// After unblocking, any newly deliverable signals remain in the queue and
/// will be picked up on the next call to `sigrt_deliver_pending`.
pub fn sigrt_unblock(pid: u32, mask: u32) {
    let mut table = RT_QUEUES.lock();
    if let Some(slot) = find_slot(&table, pid) {
        table[slot].rt_blocked &= !mask;
    }
}

/// Return the bitmask of pending, **unblocked** RT signals for `pid`.
///
/// Bit `n` set means signal `SIGRTMIN + n` has at least one queued entry
/// and is not blocked.
pub fn sigrt_get_pending(pid: u32) -> u32 {
    let table = RT_QUEUES.lock();
    match find_slot(&table, pid) {
        None => 0,
        Some(slot) => {
            let q = &table[slot];
            q.rt_pending & !q.rt_blocked
        }
    }
}

/// Deliver the lowest-numbered pending, unblocked RT signal to `pid`.
///
/// POSIX specifies that the lowest-numbered pending RT signal is delivered
/// first.  This function dequeues that signal and calls the registered
/// signal handler via the standard process signal delivery path.
///
/// Currently this routes through `crate::process::signal::send_signal_to`
/// (which enqueues the signal into the process's standard pending bitmask
/// so that `deliver_pending_signals` will pick it up on the next scheduler
/// window).  A more complete implementation would push the `Siginfo` onto
/// the process's SA_SIGINFO stack frame directly.
pub fn sigrt_deliver_pending(pid: u32) {
    // Find lowest-numbered pending unblocked signal.
    let deliverable = sigrt_get_pending(pid);
    if deliverable == 0 {
        return;
    }

    // Lowest-set-bit gives the lowest-numbered signal.
    let idx = deliverable.trailing_zeros() as usize;
    let signo = SIGRTMIN + idx as u32;

    // Dequeue the siginfo.
    let info_opt = sigrt_dequeue(pid, signo);
    let _info = match info_opt {
        Some(i) => i,
        None => return,
    };

    // Route through the standard signal path so deliver_pending_signals
    // handles the handler lookup / signal frame setup.
    // RT signals use numbers 34-64; cast to u8 fits (max 64 < 255).
    let _ = crate::process::signal::send_signal_to(pid, signo as u8);

    crate::serial_println!(
        "[sigrt] delivered sig={} (SIGRT+{}) to PID {}",
        signo,
        idx,
        pid
    );
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the real-time signal subsystem.
pub fn init() {
    crate::serial_println!(
        "  sigrt: real-time signal subsystem ready ({} slots)",
        SIGRT_MAX_PROCS
    );
}
