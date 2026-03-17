use crate::process::realtime_signal::{sigrt_send, Siginfo, SIGRTMAX, SIGRTMIN, SI_TIMER};
use crate::sync::Mutex;
/// POSIX Interval Timers — `timer_create`, `timer_settime`, `timer_gettime`,
/// `timer_delete`, `timer_getoverrun`.
///
/// Implements POSIX.1-2008 §14 interval timers entirely in static storage so
/// that no heap allocation is required.  All state lives in
/// `POSIX_TIMERS: Mutex<[PosixTimer; MAX_PROC_TIMERS]>`.
///
/// # Clocks supported
///
/// | Constant                  | ID | Behaviour                         |
/// |---------------------------|----|-----------------------------------|
/// | `CLOCK_REALTIME`          |  0 | Monotonic wall-clock (uptime_ms)  |
/// | `CLOCK_MONOTONIC`         |  1 | Same as CLOCK_REALTIME here       |
/// | `CLOCK_PROCESS_CPUTIME_ID`|  2 | Stubbed — treated as monotonic    |
/// | `CLOCK_THREAD_CPUTIME_ID` |  3 | Stubbed — treated as monotonic    |
///
/// # Notification (sigev_notify)
///
/// | Constant        | ID | Behaviour                                    |
/// |-----------------|----|----------------------------------------------|
/// | `SIGEV_SIGNAL`  |  0 | Deliver `sigev_signo` via RT signal queue    |
/// | `SIGEV_NONE`    |  1 | No notification (timer fires silently)       |
/// | `SIGEV_THREAD`  |  2 | Stub — logged, not yet implemented           |
/// | `SIGEV_THREAD_ID`| 4 | Stub — logged, not yet implemented           |
///
/// # timer_settime flags
///
/// | Constant       | Value | Meaning                                    |
/// |----------------|-------|--------------------------------------------|
/// | `TIMER_ABSTIME`|     1 | `value_ms` is an absolute time             |
/// |                |     0 | `value_ms` is relative to current time     |
///
/// RULES (no violations or the kernel panics):
///   - No heap (`Vec`, `Box`, `String`, `alloc::*`)
///   - No float casts (`as f32`, `as f64`)
///   - No `unwrap()`, `expect()`, `panic!()`
///   - All counters: `saturating_add` / `saturating_sub`
///   - All sequence numbers: `wrapping_add`
///   - MMIO: `read_volatile` / `write_volatile` only
///   - Every struct inside a `static Mutex` must be `Copy` with `const fn empty()`
use core::sync::atomic::{AtomicI32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum simultaneous timers per process.
pub const MAX_TIMERS_PER_PROC: usize = 32;
/// Maximum timers across all processes.
pub const MAX_PROC_TIMERS: usize = 256;

// Clock IDs (POSIX).
pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;
pub const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
pub const CLOCK_THREAD_CPUTIME_ID: i32 = 3;

// Notification types (POSIX sigevent).
pub const SIGEV_SIGNAL: i32 = 0;
pub const SIGEV_NONE: i32 = 1;
pub const SIGEV_THREAD: i32 = 2;
pub const SIGEV_THREAD_ID: i32 = 4;

/// `timer_settime` flag: treat `value_ms` as absolute time.
pub const TIMER_ABSTIME: i32 = 1;

// Error codes returned as negative integers.
const EINVAL: i32 = -22;
const ENOMEM: i32 = -12;
const ESRCH: i32 = -3;

// ---------------------------------------------------------------------------
// PosixTimer
// ---------------------------------------------------------------------------

/// A single POSIX interval timer entry.
///
/// Must be `Copy` so it can live in a `static Mutex<[PosixTimer; N]>`.
#[derive(Copy, Clone)]
pub struct PosixTimer {
    /// Unique timer identifier returned to user space.
    pub timer_id: i32,
    /// Owning process PID.
    pub pid: u32,
    /// Clock source (CLOCK_REALTIME, CLOCK_MONOTONIC, …).
    pub clock_id: i32,
    /// Notification type (SIGEV_SIGNAL, SIGEV_NONE, …).
    pub sigev_notify: i32,
    /// Signal number to deliver on expiration (for `SIGEV_SIGNAL`).
    pub sigev_signo: u32,
    /// `sival_int` payload embedded in the delivered `Siginfo`.
    pub sigev_value_int: i32,
    /// Timer is armed (will fire).
    pub armed: bool,
    /// Slot is in use.
    pub active: bool,
    /// Absolute time (ms from boot) of next expiration.
    pub next_exp_ms: u64,
    /// Repeating interval in ms.  0 = one-shot.
    pub interval_ms: u64,
    /// Number of expirations missed since the last delivery.
    pub overrun: i32,
    /// Total number of times this timer has fired (for diagnostics).
    pub fire_count: u64,
}

impl PosixTimer {
    /// Construct an empty / inactive timer slot.
    pub const fn empty() -> Self {
        PosixTimer {
            timer_id: -1,
            pid: 0,
            clock_id: CLOCK_REALTIME,
            sigev_notify: SIGEV_NONE,
            sigev_signo: 0,
            sigev_value_int: 0,
            armed: false,
            active: false,
            next_exp_ms: 0,
            interval_ms: 0,
            overrun: 0,
            fire_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static POSIX_TIMERS: Mutex<[PosixTimer; MAX_PROC_TIMERS]> =
    Mutex::new([const { PosixTimer::empty() }; MAX_PROC_TIMERS]);

/// Monotonically increasing timer ID generator.  Wraps via `wrapping_add`.
static NEXT_TIMER_ID: AtomicI32 = AtomicI32::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate the next unique timer ID (wrapping, skip 0 and negatives).
fn next_id() -> i32 {
    let mut id = NEXT_TIMER_ID
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    // Ensure non-zero and positive to avoid confusion with error codes.
    if id <= 0 {
        id = 1;
        NEXT_TIMER_ID.store(2, Ordering::Relaxed);
    }
    id
}

/// Find a free slot in the timer table.
fn find_free(table: &[PosixTimer; MAX_PROC_TIMERS]) -> Option<usize> {
    table.iter().position(|t| !t.active)
}

/// Find the slot for `timer_id`.
fn find_by_id(table: &[PosixTimer; MAX_PROC_TIMERS], timer_id: i32) -> Option<usize> {
    table
        .iter()
        .position(|t| t.active && t.timer_id == timer_id)
}

/// Count how many active timers the given `pid` already owns.
fn count_for_pid(table: &[PosixTimer; MAX_PROC_TIMERS], pid: u32) -> usize {
    table.iter().filter(|t| t.active && t.pid == pid).count()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Allocate a new POSIX timer for `pid`.
///
/// # Arguments
/// * `pid`            — owning process.
/// * `clock_id`       — `CLOCK_REALTIME`, `CLOCK_MONOTONIC`, …
/// * `sigev_notify`   — notification type (`SIGEV_SIGNAL`, `SIGEV_NONE`, …).
/// * `sigev_signo`    — signal number for `SIGEV_SIGNAL` (ignored otherwise).
/// * `sigev_value`    — `sival_int` embedded in the delivered `Siginfo`.
///
/// # Returns
/// Positive timer ID on success, or a negative errno on failure:
/// * `-22` (`EINVAL`) — unknown `clock_id` or `sigev_signo` out of range.
/// * `-12` (`ENOMEM`) — timer table full or per-process limit reached.
pub fn timer_create(
    pid: u32,
    clock_id: i32,
    sigev_notify: i32,
    sigev_signo: u32,
    sigev_value: i32,
) -> i32 {
    // Validate clock_id.
    match clock_id {
        CLOCK_REALTIME | CLOCK_MONOTONIC | CLOCK_PROCESS_CPUTIME_ID | CLOCK_THREAD_CPUTIME_ID => {}
        _ => return EINVAL,
    }

    // Validate sigev_signo when SIGEV_SIGNAL is requested.
    if sigev_notify == SIGEV_SIGNAL {
        if sigev_signo < SIGRTMIN || sigev_signo > SIGRTMAX {
            // Also allow standard signals 1-31.
            if sigev_signo == 0 || sigev_signo > 64 {
                return EINVAL;
            }
        }
    }

    let mut table = POSIX_TIMERS.lock();

    // Enforce per-process limit.
    if count_for_pid(&table, pid) >= MAX_TIMERS_PER_PROC {
        return ENOMEM;
    }

    let slot = match find_free(&table) {
        Some(s) => s,
        None => return ENOMEM,
    };

    let id = next_id();

    table[slot] = PosixTimer {
        timer_id: id,
        pid,
        clock_id,
        sigev_notify,
        sigev_signo,
        sigev_value_int: sigev_value,
        armed: false,
        active: true,
        next_exp_ms: 0,
        interval_ms: 0,
        overrun: 0,
        fire_count: 0,
    };

    crate::serial_println!(
        "[posix_timer] timer_create: pid={} id={} clock={} notify={} sig={}",
        pid,
        id,
        clock_id,
        sigev_notify,
        sigev_signo
    );

    id
}

/// Arm or disarm a timer.
///
/// # Arguments
/// * `timer_id`    — timer returned by `timer_create`.
/// * `flags`       — 0 = relative, `TIMER_ABSTIME` = absolute.
/// * `interval_ms` — repeat interval in ms (0 = one-shot).
/// * `value_ms`    — initial delay in ms (or absolute expiry if `TIMER_ABSTIME`).
///                   Passing 0 with no interval disarms the timer.
///
/// # Returns
/// `0` on success, or a negative errno:
/// * `-3`  (`ESRCH`)  — timer not found.
/// * `-22` (`EINVAL`) — invalid flags.
pub fn timer_settime(timer_id: i32, flags: i32, interval_ms: u64, value_ms: u64) -> i32 {
    // Only flag 0 (relative) and TIMER_ABSTIME=1 are defined.
    if flags != 0 && flags != TIMER_ABSTIME {
        return EINVAL;
    }

    let current_ms = crate::time::clock::uptime_ms();

    let mut table = POSIX_TIMERS.lock();
    let slot = match find_by_id(&table, timer_id) {
        Some(s) => s,
        None => return ESRCH,
    };

    // value_ms == 0 && interval_ms == 0 → disarm.
    if value_ms == 0 && interval_ms == 0 {
        table[slot].armed = false;
        table[slot].next_exp_ms = 0;
        table[slot].interval_ms = 0;
        return 0;
    }

    let next_exp = if flags == TIMER_ABSTIME {
        value_ms // caller supplies absolute time
    } else {
        current_ms.saturating_add(value_ms) // relative to now
    };

    table[slot].next_exp_ms = next_exp;
    table[slot].interval_ms = interval_ms;
    table[slot].armed = true;
    table[slot].overrun = 0;

    crate::serial_println!(
        "[posix_timer] timer_settime: id={} next_exp={}ms interval={}ms",
        timer_id,
        next_exp,
        interval_ms
    );

    0
}

/// Query remaining time and interval for a timer.
///
/// # Returns
/// `Some((remaining_ms, interval_ms))` if the timer exists, `None` otherwise.
/// `remaining_ms` is 0 for an unarmed or already-expired one-shot timer.
pub fn timer_gettime(timer_id: i32) -> Option<(u64, u64)> {
    let current_ms = crate::time::clock::uptime_ms();
    let table = POSIX_TIMERS.lock();
    let slot = find_by_id(&table, timer_id)?;
    let t = &table[slot];
    let remaining = if t.armed && t.next_exp_ms > current_ms {
        t.next_exp_ms - current_ms
    } else {
        0
    };
    Some((remaining, t.interval_ms))
}

/// Delete a timer.
///
/// Disarms the timer and frees its slot.
///
/// # Returns
/// `0` on success, `-3` (`ESRCH`) if the timer does not exist.
pub fn timer_delete(timer_id: i32) -> i32 {
    let mut table = POSIX_TIMERS.lock();
    match find_by_id(&table, timer_id) {
        None => ESRCH,
        Some(slot) => {
            table[slot] = PosixTimer::empty();
            crate::serial_println!("[posix_timer] timer_delete: id={}", timer_id);
            0
        }
    }
}

/// Return the overrun count for a timer and reset it to zero.
///
/// The overrun count is the number of additional expirations that occurred
/// between successive signal deliveries.
///
/// # Returns
/// Non-negative overrun count, or `EINVAL` (`-22`) if the timer does not exist.
pub fn timer_getoverrun(timer_id: i32) -> i32 {
    let mut table = POSIX_TIMERS.lock();
    match find_by_id(&table, timer_id) {
        None => EINVAL,
        Some(slot) => {
            let ov = table[slot].overrun;
            table[slot].overrun = 0;
            ov
        }
    }
}

/// Process all armed timers.
///
/// Called from the timer interrupt handler (or the scheduler tick) with the
/// current uptime in milliseconds.  For each expired timer:
///
/// 1. Increment `overrun` if a previous delivery is still pending (i.e. the
///    signal has not yet been dequeued from the RT queue).
/// 2. Build a `Siginfo` with `si_code = SI_TIMER`, `si_timerid`, and
///    `si_overrun`, then call `sigrt_send` (for RT signals) or the standard
///    `send_signal_to` path (for standard signals).
/// 3. Rearm the timer if `interval_ms > 0`, otherwise disarm.
///
/// This function holds `POSIX_TIMERS` for the duration of the scan.
/// It must not call any function that would attempt to lock `POSIX_TIMERS`
/// again (no re-entrant locking).
pub fn posix_timer_tick(current_ms: u64) {
    // Collect expired timers without holding the lock across signal delivery.
    // We copy the minimal data needed, release the lock, then send signals.

    // Fixed-size staging array: (timer_id, pid, sigev_notify, sigev_signo,
    //                            sigev_value_int, overrun)
    const BATCH: usize = 32;
    let mut batch: [(i32, u32, i32, u32, i32, i32); BATCH] = [(-1, 0, SIGEV_NONE, 0, 0, 0); BATCH];
    let mut batch_len = 0usize;

    {
        let mut table = POSIX_TIMERS.lock();
        for t in table.iter_mut() {
            if !t.active || !t.armed {
                continue;
            }
            if t.next_exp_ms > current_ms {
                continue; // not yet expired
            }

            // Compute how many intervals were missed.
            let missed = if t.interval_ms > 0 {
                let elapsed = current_ms.saturating_sub(t.next_exp_ms);
                (elapsed / t.interval_ms) as i32
            } else {
                0
            };

            // Accumulate overrun.
            t.overrun = t.overrun.saturating_add(missed);
            t.fire_count = t.fire_count.saturating_add(1).saturating_add(missed as u64);

            if batch_len < BATCH {
                batch[batch_len] = (
                    t.timer_id,
                    t.pid,
                    t.sigev_notify,
                    t.sigev_signo,
                    t.sigev_value_int,
                    t.overrun,
                );
                batch_len = batch_len.saturating_add(1);
                // Reset overrun after capturing it for delivery.
                t.overrun = 0;
            }

            // Rearm or disarm.
            if t.interval_ms > 0 {
                // Advance to the next expiry, skipping missed intervals.
                let advance = t.interval_ms.saturating_add(if missed > 0 {
                    t.interval_ms.saturating_mul(missed as u64)
                } else {
                    0
                });
                t.next_exp_ms = t.next_exp_ms.wrapping_add(advance);
            } else {
                t.armed = false;
            }
        }
    } // Release POSIX_TIMERS lock.

    // Deliver signals outside the lock.
    for i in 0..batch_len {
        let (timer_id, pid, notify, signo, sival, overrun) = batch[i];
        if notify == SIGEV_NONE {
            continue;
        }
        if notify == SIGEV_SIGNAL && signo >= SIGRTMIN && signo <= SIGRTMAX {
            let mut info = Siginfo::empty();
            info.si_signo = signo;
            info.si_code = SI_TIMER;
            info.si_timerid = timer_id;
            info.si_overrun = overrun;
            info.si_value_int = sival;
            info.si_pid = 0; // kernel-generated
            let _ = sigrt_send(pid, signo, info);
        } else if notify == SIGEV_SIGNAL && signo > 0 && signo <= 31 {
            // Standard signal (1-31): route through the simple send path.
            let _ = crate::process::signal::send_signal_to(pid, signo as u8);
        } else if notify == SIGEV_THREAD || notify == SIGEV_THREAD_ID {
            crate::serial_println!(
                "[posix_timer] SIGEV_THREAD not yet implemented (timer_id={})",
                timer_id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the POSIX timer subsystem.
pub fn init() {
    crate::serial_println!(
        "  posix_timer: subsystem ready (max {} timers)",
        MAX_PROC_TIMERS
    );
}
