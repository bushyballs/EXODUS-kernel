// timerfd — Linux-compatible timer file descriptors
//
// Returns a file descriptor that becomes readable when a timer expires.
// Used by: poll/select/epoll, event-loop frameworks.
//
// Design:
//   - A global fixed-size table of TimerFd entries, each protected by the
//     table Mutex.  The table is indexed by an fd (u32).
//   - timerfd_tick() is called from the timer interrupt every elapsed_ns
//     nanoseconds; it walks all active entries, increments expirations on
//     any that fired, and rearms repeating timers.
//   - timerfd_read() drains and returns the expiration count (atomically
//     with an AtomicU64 CAS so no lock is needed for the read fast-path
//     once the fd index is known).
//
// Inspired by: Linux timerfd(2) (API contract, CLOCK_MONOTONIC semantics).
// All code is original.

use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Public constants (match Linux ABI)
// ---------------------------------------------------------------------------

pub const CLOCK_REALTIME: u8 = 0;
pub const CLOCK_MONOTONIC: u8 = 1;
pub const TFD_NONBLOCK: u32 = 0x800;
pub const TFD_CLOEXEC: u32 = 0x80000;

/// EAGAIN / EWOULDBLOCK — returned when a non-blocking read would block.
const EAGAIN: i32 = 11;
/// EINVAL — returned for an invalid argument.
const EINVAL: i32 = 22;
/// EMFILE — no more file descriptors available.
const EMFILE: i32 = 24;
/// EBADF  — bad file descriptor number.
const EBADF: i32 = 9;

/// Maximum number of simultaneously open timerfd instances.
const MAX_TIMERFDS: usize = 64;

// ---------------------------------------------------------------------------
// TimerFd entry
// ---------------------------------------------------------------------------

/// One timer file descriptor.
///
/// `expirations` is the only field accessed from the IRQ context
/// (timerfd_tick).  It is an `AtomicU64` so the hot-path increment
/// in the IRQ handler does not need to acquire the table lock.
/// All other fields are mutated only under the table lock.
pub struct TimerFd {
    /// File descriptor number (1-based; 0 = unused slot).
    pub fd: u32,
    /// Clock source: CLOCK_REALTIME or CLOCK_MONOTONIC.
    pub clock: u8,
    /// Flags supplied at creation time (TFD_NONBLOCK, TFD_CLOEXEC).
    pub flags: u32,
    /// Repeat interval in nanoseconds. 0 = one-shot.
    pub interval_ns: u64,
    /// Absolute timestamp (ns) at which the timer next fires. 0 = disarmed.
    pub next_expiry_ns: u64,
    /// Accumulated, unread expiration count. Accessed from IRQ + reader.
    pub expirations: AtomicU64,
    /// Whether this entry is active (armed or just created with a valid fd).
    pub active: bool,
}

impl TimerFd {
    fn new(fd: u32, clock: u8, flags: u32) -> Self {
        TimerFd {
            fd,
            clock,
            flags,
            interval_ns: 0,
            next_expiry_ns: 0,
            expirations: AtomicU64::new(0),
            active: true,
        }
    }

    fn is_nonblock(&self) -> bool {
        self.flags & TFD_NONBLOCK != 0
    }

    fn is_armed(&self) -> bool {
        self.next_expiry_ns != 0
    }
}

// ---------------------------------------------------------------------------
// Global timerfd table
// ---------------------------------------------------------------------------

/// Wrapper that can live in a static and be initialised lazily.
struct TimerFdTable {
    slots: [Option<TimerFd>; MAX_TIMERFDS],
    /// Next fd value to try allocating. Wraps at MAX_TIMERFDS.
    next_fd: u32,
}

impl TimerFdTable {
    const fn new() -> Self {
        // Cannot use [None; N] because TimerFd is not Copy. Use a manual
        // approach with a const initialiser that fills via array init macro.
        #[allow(clippy::declare_interior_mutable_const)]
        const NONE_SLOT: Option<TimerFd> = None;
        TimerFdTable {
            slots: [NONE_SLOT; MAX_TIMERFDS],
            next_fd: 1,
        }
    }

    /// Find a free slot and allocate a new fd. Returns EMFILE if full.
    fn alloc(&mut self, clock: u8, flags: u32) -> Result<u32, i32> {
        // Linear scan from next_fd, wrapping.
        for _ in 0..MAX_TIMERFDS {
            let idx = (self.next_fd as usize).saturating_sub(1) % MAX_TIMERFDS;
            self.next_fd = (self.next_fd % MAX_TIMERFDS as u32).saturating_add(1);
            if self.slots[idx].is_none() {
                let fd = (idx as u32).saturating_add(1); // 1-based
                self.slots[idx] = Some(TimerFd::new(fd, clock, flags));
                return Ok(fd);
            }
        }
        Err(EMFILE)
    }

    fn slot_mut(&mut self, fd: u32) -> Option<&mut TimerFd> {
        if fd == 0 {
            return None;
        }
        let idx = (fd as usize).saturating_sub(1);
        if idx >= MAX_TIMERFDS {
            return None;
        }
        self.slots[idx].as_mut().filter(|t| t.fd == fd && t.active)
    }

    fn slot_ref(&self, fd: u32) -> Option<&TimerFd> {
        if fd == 0 {
            return None;
        }
        let idx = (fd as usize).saturating_sub(1);
        if idx >= MAX_TIMERFDS {
            return None;
        }
        self.slots[idx].as_ref().filter(|t| t.fd == fd && t.active)
    }

    fn free(&mut self, fd: u32) {
        if fd == 0 {
            return;
        }
        let idx = (fd as usize).saturating_sub(1);
        if idx < MAX_TIMERFDS {
            self.slots[idx] = None;
        }
    }
}

// SAFETY: TimerFdTable is only accessed through the Mutex guard.
unsafe impl Send for TimerFdTable {}

static TIMERFD_TABLE: Mutex<TimerFdTable> = Mutex::new(TimerFdTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new timerfd.
///
/// `clockid` must be CLOCK_REALTIME (0) or CLOCK_MONOTONIC (1).
/// `flags` may include TFD_NONBLOCK and/or TFD_CLOEXEC.
///
/// Returns the allocated fd on success, or a negative errno on failure.
pub fn timerfd_create(clockid: u8, flags: u32) -> Result<u32, i32> {
    if clockid != CLOCK_REALTIME && clockid != CLOCK_MONOTONIC {
        return Err(EINVAL);
    }
    TIMERFD_TABLE.lock().alloc(clockid, flags)
}

/// Arm or disarm a timerfd.
///
/// `value_ns`    — initial expiry relative to now (nanoseconds).
///                 Pass 0 to disarm the timer.
/// `interval_ns` — repeat interval.  Pass 0 for a one-shot timer.
/// `current_ns`  — current monotonic time in nanoseconds (from TSC / HPET).
///
/// Returns Ok(()) on success, Err(errno) on failure.
pub fn timerfd_settime(
    fd: u32,
    interval_ns: u64,
    value_ns: u64,
    current_ns: u64,
) -> Result<(), i32> {
    let mut tbl = TIMERFD_TABLE.lock();
    let tfd = tbl.slot_mut(fd).ok_or(EBADF)?;

    if value_ns == 0 {
        // Disarm
        tfd.interval_ns = 0;
        tfd.next_expiry_ns = 0;
    } else {
        tfd.interval_ns = interval_ns;
        tfd.next_expiry_ns = current_ns.saturating_add(value_ns);
        // Reset stale expiration count when rearming.
        tfd.expirations.store(0, Ordering::Relaxed);
    }
    Ok(())
}

/// Query the current timer setting.
///
/// Returns `(remaining_ns, interval_ns)` where `remaining_ns` is the time
/// until the next expiry (0 if disarmed) and `interval_ns` is the period.
///
/// `current_ns` is the current monotonic timestamp.
pub fn timerfd_gettime(fd: u32, current_ns: u64) -> Result<(u64, u64), i32> {
    let tbl = TIMERFD_TABLE.lock();
    let tfd = tbl.slot_ref(fd).ok_or(EBADF)?;

    let remaining = if tfd.next_expiry_ns == 0 {
        0
    } else {
        tfd.next_expiry_ns.saturating_sub(current_ns)
    };
    Ok((remaining, tfd.interval_ns))
}

/// Read (and clear) the expiration count from a timerfd.
///
/// Returns the number of times the timer has expired since the last read,
/// or Err(EAGAIN) if no expirations have occurred and the fd is
/// TFD_NONBLOCK.  Blocking semantics are indicated by Err(EAGAIN) in
/// both cases; a real scheduler would put the caller to sleep.
pub fn timerfd_read(fd: u32) -> Result<u64, i32> {
    let tbl = TIMERFD_TABLE.lock();
    let tfd = tbl.slot_ref(fd).ok_or(EBADF)?;

    // Atomically drain the expiration counter.
    let count = tfd.expirations.swap(0, Ordering::AcqRel);
    if count == 0 {
        // No expirations yet.
        if tfd.is_nonblock() {
            return Err(EAGAIN); // EAGAIN — caller should poll/epoll
        }
        // Blocking mode: return EAGAIN as a signal to the scheduler that
        // the thread should be put to sleep until timerfd_tick() fires.
        return Err(EAGAIN);
    }
    Ok(count)
}

/// Timer interrupt hook — advance all active timerfd instances.
///
/// Called from the timer IRQ handler with the current monotonic timestamp
/// (nanoseconds).  Must be async-signal-safe: only uses atomics and the
/// Mutex spinlock.
pub fn timerfd_tick(current_ns: u64) {
    let mut tbl = TIMERFD_TABLE.lock();
    for slot in tbl.slots.iter_mut() {
        let tfd = match slot {
            Some(t) if t.active && t.is_armed() => t,
            _ => continue,
        };

        if current_ns < tfd.next_expiry_ns {
            continue; // not yet expired
        }

        // Count how many intervals have elapsed (at least 1).
        let elapsed_intervals = if tfd.interval_ns > 0 {
            let overdue = current_ns.saturating_sub(tfd.next_expiry_ns);
            overdue.saturating_div(tfd.interval_ns).saturating_add(1)
        } else {
            1
        };

        tfd.expirations
            .fetch_add(elapsed_intervals, Ordering::AcqRel);

        if tfd.interval_ns > 0 {
            // Rearm: advance next_expiry by the number of intervals that fired.
            tfd.next_expiry_ns = tfd
                .next_expiry_ns
                .saturating_add(tfd.interval_ns.saturating_mul(elapsed_intervals));
        } else {
            // One-shot: disarm after firing.
            tfd.next_expiry_ns = 0;
        }
    }
}

/// Close a timerfd and free its slot.
pub fn timerfd_close(fd: u32) -> Result<(), i32> {
    let mut tbl = TIMERFD_TABLE.lock();
    // Verify it exists first.
    if tbl.slot_ref(fd).is_none() {
        return Err(EBADF);
    }
    tbl.free(fd);
    Ok(())
}

/// Close all timerfds opened by the given process PID.
///
/// Called from process exit / exec clean-up.  Since we do not currently
/// track owner PIDs in `TimerFd` we provide a targeted-close variant for
/// callers that manage fd tracking themselves.  The fd-level clean-up path
/// in `syscall::cleanup_process_fds` calls `timerfd_close` per-fd.
pub fn timerfd_close_all(_pid: u32, fds: &[u32]) {
    let mut tbl = TIMERFD_TABLE.lock();
    for &fd in fds {
        tbl.free(fd);
    }
}

/// Initialise the timerfd subsystem.
///
/// Called once from `ipc::init()`.  The global table is already zero-
/// initialised at compile time so this is a no-op beyond logging.
pub fn init() {
    crate::serial_println!(
        "    [timerfd] timer file descriptor subsystem ready (max {} fds)",
        MAX_TIMERFDS
    );
}
