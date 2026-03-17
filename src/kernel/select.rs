/// select / pselect6 — synchronous I/O multiplexing
///
/// Implements POSIX select(2) semantics using a spin-poll loop with
/// rdtsc-based timeout.
///
/// ABI (Linux x86_64 compatible):
///   select(nfds, readfds*, writefds*, exceptfds*, timeval*)
///   pselect6(nfds, readfds*, writefds*, exceptfds*, timespec*, sigset*)
///
/// This implementation:
///   - No alloc (fixed-size FdSet on the stack)
///   - No float casts
///   - No panic
///   - Saturating arithmetic throughout
///   - Spin-poll with ~1000-TSC-cycle pauses between iterations
///
/// Inspired by: POSIX select(2), Linux fs/select.c.  All code is original.
use crate::kernel::fd_poll;

// ---------------------------------------------------------------------------
// FdSet — bitmask of up to 1024 file descriptors
// ---------------------------------------------------------------------------

/// A bitmask of up to 1024 file descriptors, matching the Linux fd_set ABI.
///
/// Layout: 16 × u64 words, each word covers 64 consecutive fd numbers.
/// Word `i` covers fds `64*i .. 64*i+63`.
#[derive(Clone, Copy)]
pub struct FdSet {
    pub bits: [u64; 16],
}

impl FdSet {
    /// Return an empty (all-clear) FdSet.
    #[inline]
    pub const fn zero() -> Self {
        FdSet { bits: [0u64; 16] }
    }

    /// Set the bit for `fd`.  Out-of-range fds are silently ignored.
    #[inline]
    pub fn set(&mut self, fd: i32) {
        if fd < 0 || fd >= 1024 {
            return;
        }
        let word = (fd as usize) / 64;
        let bit = (fd as usize) % 64;
        self.bits[word] |= 1u64 << bit;
    }

    /// Clear the bit for `fd`.  Out-of-range fds are silently ignored.
    #[inline]
    pub fn clear(&mut self, fd: i32) {
        if fd < 0 || fd >= 1024 {
            return;
        }
        let word = (fd as usize) / 64;
        let bit = (fd as usize) % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// Returns `true` if the bit for `fd` is set.
    #[inline]
    pub fn is_set(&self, fd: i32) -> bool {
        if fd < 0 || fd >= 1024 {
            return false;
        }
        let word = (fd as usize) / 64;
        let bit = (fd as usize) % 64;
        self.bits[word] & (1u64 << bit) != 0
    }

    /// Count the number of set bits.
    pub fn count(&self) -> usize {
        let mut n = 0usize;
        for &word in &self.bits {
            n = n.saturating_add(word.count_ones() as usize);
        }
        n
    }
}

impl Default for FdSet {
    #[inline]
    fn default() -> Self {
        FdSet::zero()
    }
}

// ---------------------------------------------------------------------------
// sys_select
// ---------------------------------------------------------------------------

/// Maximum iterations in the spin-poll loop before declaring timeout.
///
/// Each iteration pauses ~1000 TSC cycles.  At 3 GHz that is ~333 ns per
/// iteration.  1_000_000 iterations ≈ 333 ms per "tick unit" — but we use
/// the TSC-based elapsed-ns check as the real gate; this constant is only
/// a safety backstop.
const MAX_SPIN_ITERS: u64 = 10_000_000;

/// Pause ~1000 TSC cycles between poll iterations to avoid hammering the bus.
#[inline(always)]
fn spin_pause() {
    // rep nop (pause instruction) hints to the CPU that we are spinning.
    // Execute ~16 pause instructions to approximate 1000 TSC cycles.
    for _ in 0u32..16 {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }
}

/// Multiplexed I/O readiness check.
///
/// # Parameters
/// - `nfds`      — one more than the highest fd to check (capped to 1024).
/// - `readfds`   — in/out: fds to check for readability.
/// - `writefds`  — in/out: fds to check for writability.
/// - `exceptfds` — in/out: fds to check for exception conditions.
/// - `timeout_us` — `None` = block forever, `Some(0)` = non-blocking poll,
///                  `Some(n)` = wait up to `n` microseconds.
///
/// # Returns
/// - `>0`  — number of ready fds; the sets are modified to contain only
///           those fds.
/// - `0`   — timeout expired; all sets cleared.
/// - `-4`  — EINTR (not currently generated; reserved for future signal wakeup).
pub fn sys_select(
    nfds: i32,
    readfds: &mut FdSet,
    writefds: &mut FdSet,
    exceptfds: &mut FdSet,
    timeout_us: Option<u64>,
) -> i32 {
    // Cap nfds to the allowed maximum.
    let nfds = if nfds > 1024 {
        1024
    } else if nfds < 0 {
        return -22;
    } else {
        nfds
    };

    // Save a snapshot of the requested sets so we can clear on timeout.
    let orig_read = *readfds;
    let orig_write = *writefds;
    let orig_except = *exceptfds;

    // Record start time for timeout tracking.
    let start_ns = fd_poll::read_tsc_ns();

    // Convert timeout_us to nanoseconds (saturating to avoid overflow).
    let timeout_ns: Option<u64> = timeout_us.map(|us| us.saturating_mul(1000));

    let mut iter: u64 = 0;

    loop {
        // ── Probe every requested fd ──────────────────────────────────────
        let mut result_read = FdSet::zero();
        let mut result_write = FdSet::zero();
        let mut result_except = FdSet::zero();
        let mut ready: usize = 0;

        for word_idx in 0..16 {
            let base_fd = (word_idx as i32) * 64;

            // Mask out fds >= nfds for the last partial word.
            let read_word = orig_read.bits[word_idx];
            let write_word = orig_write.bits[word_idx];
            let except_word = orig_except.bits[word_idx];

            // Fast skip when the word is entirely zero.
            if read_word == 0 && write_word == 0 && except_word == 0 {
                continue;
            }

            for bit in 0u32..64 {
                let fd = base_fd + bit as i32;
                if fd >= nfds {
                    break;
                }

                let mask = 1u64 << bit;

                if read_word & mask != 0 {
                    if fd_poll::fd_can_read(fd) {
                        result_read.bits[word_idx] |= mask;
                        ready = ready.saturating_add(1);
                    }
                }

                if write_word & mask != 0 {
                    if fd_poll::fd_can_write(fd) {
                        result_write.bits[word_idx] |= mask;
                        ready = ready.saturating_add(1);
                    }
                }

                if except_word & mask != 0 {
                    if fd_poll::fd_has_error(fd) {
                        result_except.bits[word_idx] |= mask;
                        ready = ready.saturating_add(1);
                    }
                }
            }
        }

        // ── Return immediately if anything is ready ───────────────────────
        if ready > 0 {
            *readfds = result_read;
            *writefds = result_write;
            *exceptfds = result_except;
            return ready.min(i32::MAX as usize) as i32;
        }

        // ── Check timeout ─────────────────────────────────────────────────
        match timeout_ns {
            Some(0) => {
                // Non-blocking: clear sets and return 0 immediately.
                *readfds = FdSet::zero();
                *writefds = FdSet::zero();
                *exceptfds = FdSet::zero();
                return 0;
            }
            Some(limit_ns) => {
                let elapsed = fd_poll::read_tsc_ns().saturating_sub(start_ns);
                if elapsed >= limit_ns {
                    *readfds = FdSet::zero();
                    *writefds = FdSet::zero();
                    *exceptfds = FdSet::zero();
                    return 0;
                }
            }
            None => {
                // Block forever — check safety backstop only.
                iter = iter.saturating_add(1);
                if iter >= MAX_SPIN_ITERS {
                    // Should not happen in practice; avoid infinite spin.
                    crate::serial_println!(
                        "[select] WARNING: MAX_SPIN_ITERS reached (infinite block?)"
                    );
                    iter = 0; // reset and keep waiting
                }
            }
        }

        spin_pause();
    }
}
