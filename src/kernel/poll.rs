/// poll / ppoll — wait for events on file descriptors
///
/// Implements POSIX poll(2) / ppoll(2) semantics using a spin-poll loop with
/// rdtsc-based timeout.
///
/// ABI (Linux x86_64 compatible):
///   poll(fds*, nfds, timeout_ms)
///   ppoll(fds*, nfds, timespec*, sigset*)
///
/// Rules followed:
///   - No alloc (slice of PollFd passed directly from caller)
///   - No float casts
///   - No panic — early returns on error conditions
///   - Saturating arithmetic throughout
///   - Spin-poll with pause instructions between iterations
///
/// Inspired by: POSIX poll(2), Linux fs/select.c.  All code is original.
use crate::kernel::fd_poll;

// ---------------------------------------------------------------------------
// pollfd  — Linux ABI struct
// ---------------------------------------------------------------------------

/// The `pollfd` structure as defined by the Linux x86_64 ABI.
///
/// Size: 8 bytes.  Fields must be at their natural offsets.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PollFd {
    /// File descriptor to watch.
    pub fd: i32,
    /// Requested events (caller sets this before calling sys_poll).
    pub events: i16,
    /// Returned events (kernel fills this in; caller reads after return).
    pub revents: i16,
}

impl PollFd {
    /// A const-evaluable zeroed sentinel used for static/stack array initialization.
    pub const EMPTY: PollFd = PollFd {
        fd: -1,
        events: 0,
        revents: 0,
    };
}

// ---------------------------------------------------------------------------
// Event flag constants  — match Linux ABI
// ---------------------------------------------------------------------------

/// Data may be read without blocking.
pub const POLLIN: i16 = 0x0001;
/// Urgent / out-of-band data available.
pub const POLLPRI: i16 = 0x0002;
/// Writing will not block.
pub const POLLOUT: i16 = 0x0004;
/// Error condition (output only; always monitored).
pub const POLLERR: i16 = 0x0008;
/// Hang-up on the fd (output only; always monitored).
pub const POLLHUP: i16 = 0x0010;
/// Invalid request: fd is not open (output only).
pub const POLLNVAL: i16 = 0x0020;

// ---------------------------------------------------------------------------
// sys_poll
// ---------------------------------------------------------------------------

/// POSIX poll(2) — wait for events on a set of file descriptors.
///
/// # Parameters
/// - `fds`        — slice of `PollFd` entries.  `fd < 0` entries are ignored
///                  with `revents = 0`.
/// - `nfds`       — number of entries in `fds` to process (capped to `fds.len()`).
/// - `timeout_ms` — milliseconds to wait:
///                  * `< 0`  block indefinitely (until at least one fd is ready).
///                  * `= 0`  return immediately after one pass.
///                  * `> 0`  wait at most `timeout_ms` milliseconds.
///
/// # Returns
/// - `>0` — count of entries with non-zero `revents`.
/// - `0`  — timeout expired with no ready fds.
/// - `<0` — negated errno (currently only `-22` EINVAL for `nfds > 1024`).
pub fn sys_poll(fds: &mut [PollFd], nfds: usize, timeout_ms: i32) -> i32 {
    // Guard: cap nfds, reject obviously bad values.
    let nfds = if nfds > 1024 {
        return -22; // EINVAL
    } else {
        nfds.min(fds.len())
    };

    // Convert timeout_ms to nanoseconds for TSC comparison.
    // Saturating mul: 0x7fff_ffff ms * 1_000_000 would overflow u64 → cap.
    let timeout_ns: Option<u64> = if timeout_ms < 0 {
        None // block forever
    } else {
        Some((timeout_ms as u64).saturating_mul(1_000_000))
    };

    let start_ns = fd_poll::read_tsc_ns();

    loop {
        // ── Single pass: probe all fds ────────────────────────────────────
        let ready = poll_once(&mut fds[..nfds]);

        if ready > 0 {
            return ready;
        }

        // ── Timeout check ─────────────────────────────────────────────────
        match timeout_ns {
            Some(0) => {
                // Non-blocking: return immediately after first pass.
                return 0;
            }
            Some(limit_ns) => {
                let elapsed = fd_poll::read_tsc_ns().saturating_sub(start_ns);
                if elapsed >= limit_ns {
                    return 0;
                }
            }
            None => {
                // Block indefinitely — no timeout check.
            }
        }

        spin_pause();
    }
}

// ---------------------------------------------------------------------------
// sys_ppoll
// ---------------------------------------------------------------------------

/// POSIX ppoll(2) — poll with nanosecond timeout and optional signal mask.
///
/// The signal mask argument is accepted for ABI compatibility but is not
/// applied (no signal delivery mechanism is wired into the poll path yet).
///
/// # Parameters
/// - `fds`        — slice of `PollFd` entries.
/// - `nfds`       — number of entries to process (capped to `fds.len()`).
/// - `timeout_ns` — `None` = block forever; `Some(0)` = non-blocking;
///                  `Some(n)` = wait up to `n` nanoseconds.
///
/// # Returns
/// Same as `sys_poll`.
pub fn sys_ppoll(fds: &mut [PollFd], nfds: usize, timeout_ns: Option<u64>) -> i32 {
    let nfds = if nfds > 1024 {
        return -22; // EINVAL
    } else {
        nfds.min(fds.len())
    };

    let start_ns = fd_poll::read_tsc_ns();

    loop {
        let ready = poll_once(&mut fds[..nfds]);

        if ready > 0 {
            return ready;
        }

        match timeout_ns {
            Some(0) => {
                return 0;
            }
            Some(limit_ns) => {
                let elapsed = fd_poll::read_tsc_ns().saturating_sub(start_ns);
                if elapsed >= limit_ns {
                    return 0;
                }
            }
            None => {
                // Block indefinitely.
            }
        }

        spin_pause();
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Perform one synchronous pass over all entries in `fds`.
///
/// Fills `revents` for each entry and returns the count of entries where
/// `revents != 0` (POSIX semantics: each fd counted at most once).
///
/// Never blocks or loops internally.
fn poll_once(fds: &mut [PollFd]) -> i32 {
    let mut count: i32 = 0;

    for entry in fds.iter_mut() {
        // Clear revents so previous loop results don't bleed through.
        entry.revents = 0;

        // Negative fd: ignored silently (POSIX requirement).
        if entry.fd < 0 {
            continue;
        }

        // Invalid fd → POLLNVAL (always set, never masked by events).
        if !fd_poll::fd_is_valid(entry.fd) {
            entry.revents = POLLNVAL;
            count = count.saturating_add(1);
            continue;
        }

        // Error/hangup: reported unconditionally regardless of events mask.
        if fd_poll::fd_has_error(entry.fd) {
            entry.revents |= POLLERR;
        }

        // Readable data.
        if entry.events & POLLIN != 0 && fd_poll::fd_can_read(entry.fd) {
            entry.revents |= POLLIN;
        }

        // Writable space.
        if entry.events & POLLOUT != 0 && fd_poll::fd_can_write(entry.fd) {
            entry.revents |= POLLOUT;
        }

        // Count this fd once if any event fired.
        if entry.revents != 0 {
            count = count.saturating_add(1);
        }
    }

    count
}

/// Pause ~1000 TSC cycles between poll iterations.
#[inline(always)]
fn spin_pause() {
    for _ in 0u32..16 {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }
}
