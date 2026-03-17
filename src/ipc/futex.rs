use crate::sync::Mutex;
/// Futex — fast userspace mutex support
///
/// Provides Linux-compatible futex primitives for building efficient
/// userspace synchronization.  A futex is a 32-bit integer in shared
/// memory that processes can atomically test-and-wait on.
///
/// Operations supported:
///   FUTEX_WAIT          (0)   — sleep if *addr == expected
///   FUTEX_WAKE          (1)   — wake N waiters on addr
///   FUTEX_WAIT_PRIVATE  (128) — same as WAIT (no cross-process here)
///   FUTEX_WAKE_PRIVATE  (129) — same as WAKE
///   FUTEX_REQUEUE       (3)   — wake N, move M waiters to another addr
///
/// Implementation notes:
///   - Fixed-size waiter table (256 slots) — NO heap allocation.
///   - Waiter slots are identified by `(uaddr, pid, tid)`.
///   - Blocking is cooperative: the current task is removed from the
///     scheduler run-queue via `process::scheduler::SCHEDULER.lock().remove()`
///     and re-added by the waker via `wake_up()`.
///   - Timeout is expressed as nanoseconds from now; the TSC is polled
///     in the spin loop.  Pass `None` for an indefinite wait.
///
/// Rules: no_std, no heap, no float casts, saturating arithmetic.
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Operation codes
// ---------------------------------------------------------------------------

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_REQUEUE: u32 = 3;
pub const FUTEX_CMP_REQUEUE: u32 = 4;
pub const FUTEX_WAIT_BITSET: u32 = 9;
pub const FUTEX_WAKE_BITSET: u32 = 10;
pub const FUTEX_WAIT_PRIVATE: u32 = 128;
pub const FUTEX_WAKE_PRIVATE: u32 = 129;

pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xFFFF_FFFF;

// ---------------------------------------------------------------------------
// Errno mirrors (negative i32 convention used in return values)
// ---------------------------------------------------------------------------

const EAGAIN: i32 = -11;
const ETIMEDOUT: i32 = -110;
const EINVAL: i32 = -22;

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Maximum concurrent waiters across ALL futex addresses.
const MAX_WAITERS: usize = 256;

// ---------------------------------------------------------------------------
// Waiter entry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct FutexWaiter {
    /// Virtual address of the futex word this thread is sleeping on.
    uaddr: u64,
    /// Process ID of the waiting thread.
    pid: u32,
    /// Thread ID (same as pid for single-threaded processes).
    tid: u32,
    /// Bitmask for WAIT_BITSET / WAKE_BITSET (default: all-ones).
    bitset: u32,
    /// TSC value after which this waiter should be timed out.
    /// Zero means no timeout.
    deadline_tsc: u64,
    /// Set to `true` by the waker so the sleeping loop knows to exit.
    woken: bool,
    /// Slot is occupied.
    in_use: bool,
}

impl FutexWaiter {
    const fn empty() -> Self {
        FutexWaiter {
            uaddr: 0,
            pid: 0,
            tid: 0,
            bitset: FUTEX_BITSET_MATCH_ANY,
            deadline_tsc: 0,
            woken: false,
            in_use: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global waiter table
// ---------------------------------------------------------------------------

struct FutexTable {
    waiters: [FutexWaiter; MAX_WAITERS],
    total_waits: u64,
    total_wakes: u64,
    total_requeues: u64,
    total_timeouts: u64,
}

impl FutexTable {
    const fn new() -> Self {
        FutexTable {
            waiters: [FutexWaiter::empty(); MAX_WAITERS],
            total_waits: 0,
            total_wakes: 0,
            total_requeues: 0,
            total_timeouts: 0,
        }
    }

    fn find_free(&self) -> Option<usize> {
        for i in 0..MAX_WAITERS {
            if !self.waiters[i].in_use {
                return Some(i);
            }
        }
        None
    }

    /// Internal: wake up to `count` waiters matching `(uaddr, bitset)`.
    /// Returns the number actually woken.
    fn wake_inner(&mut self, uaddr: u64, count: u32, bitset: u32) -> u32 {
        let mut woken = 0u32;
        for i in 0..MAX_WAITERS {
            if woken >= count {
                break;
            }
            let w = &mut self.waiters[i];
            if w.in_use && w.uaddr == uaddr && w.bitset & bitset != 0 {
                w.woken = true;
                w.in_use = false;
                // Resume the sleeping process via the scheduler.
                crate::process::scheduler::wake_up(w.pid);
                woken = woken.saturating_add(1);
            }
        }
        self.total_wakes = self.total_wakes.saturating_add(woken as u64);
        woken
    }
}

static FUTEX_TABLE: Mutex<FutexTable> = Mutex::new(FutexTable::new());

// ---------------------------------------------------------------------------
// TSC helpers (no float casts)
// ---------------------------------------------------------------------------

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Convert nanoseconds to TSC ticks using the kernel's MHz estimate.
///
/// ticks = ns * MHz / 1000  — entirely integer arithmetic, no floats.
fn ns_to_tsc(ns: u64) -> u64 {
    let mhz = crate::time::clock::tsc_freq_mhz();
    if mhz == 0 {
        return u64::MAX;
    }
    // ns * mhz / 1000  — divide last to preserve precision
    ns.saturating_mul(mhz) / 1000
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the futex subsystem.
pub fn init() {
    // Table is const-initialised; nothing to allocate.
    serial_println!(
        "    [futex] futex subsystem ready (max {} waiters)",
        MAX_WAITERS
    );
}

// ---------------------------------------------------------------------------
// FUTEX_WAIT
// ---------------------------------------------------------------------------

/// Block the calling thread on `uaddr` if `*uaddr == expected`.
///
/// `uaddr` must already have been validated by the syscall dispatch before
/// this function is called.
///
/// Returns:
///   0          — woken by futex_wake
///   EAGAIN     — *uaddr != expected at call time (retry immediately)
///   ETIMEDOUT  — timeout expired before a wake arrived
///   EINVAL     — too many waiters / bad args
pub fn futex_wait(uaddr: u64, val: u32, timeout_ns: Option<u64>) -> i32 {
    if uaddr == 0 {
        return EINVAL;
    }

    // Read the futex word from user-space and compare.
    let current = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if current != val {
        return EAGAIN;
    }

    let pid = crate::process::getpid();
    let tid = pid; // Genesis currently maps 1 thread per process

    // Compute deadline TSC (0 = no timeout)
    let deadline_tsc = match timeout_ns {
        Some(ns) => rdtsc().saturating_add(ns_to_tsc(ns)),
        None => 0,
    };

    // Allocate a waiter slot
    let slot = {
        let mut table = FUTEX_TABLE.lock();

        // Re-read *uaddr under the table lock so no race with a concurrent wake.
        let current2 = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
        if current2 != val {
            return EAGAIN;
        }

        let slot = match table.find_free() {
            Some(s) => s,
            None => return EINVAL,
        };
        table.waiters[slot] = FutexWaiter {
            uaddr,
            pid,
            tid,
            bitset: FUTEX_BITSET_MATCH_ANY,
            deadline_tsc,
            woken: false,
            in_use: true,
        };
        table.total_waits = table.total_waits.saturating_add(1);
        slot
    };

    // Remove self from the run-queue so the scheduler does not spin on us.
    {
        let mut sched = crate::process::scheduler::SCHEDULER.lock();
        sched.remove(pid);
    }

    // Spin-wait: re-check every iteration whether we were woken or timed out.
    // The waker calls wake_up(pid) after clearing in_use, which re-adds us to
    // the run-queue.  We detect that via the `woken` flag.
    loop {
        core::hint::spin_loop();

        let woken = {
            let table = FUTEX_TABLE.lock();
            // Slot freed by waker means woken = true
            !table.waiters[slot].in_use || table.waiters[slot].woken
        };

        if woken {
            // Clean up (idempotent if waker already did it)
            let mut table = FUTEX_TABLE.lock();
            table.waiters[slot].in_use = false;
            return 0;
        }

        // Check timeout
        if deadline_tsc != 0 && rdtsc() >= deadline_tsc {
            let mut table = FUTEX_TABLE.lock();
            table.waiters[slot].in_use = false;
            table.total_timeouts = table.total_timeouts.saturating_add(1);
            // Re-add to run-queue so the process continues
            crate::process::scheduler::wake_up(pid);
            return ETIMEDOUT;
        }

        // Yield to let other tasks run
        crate::process::yield_now();
    }
}

// ---------------------------------------------------------------------------
// FUTEX_WAKE
// ---------------------------------------------------------------------------

/// Wake up to `count` waiters sleeping on `uaddr`.
///
/// Returns the number of processes that were woken.
pub fn futex_wake(uaddr: u64, count: u32) -> i32 {
    if uaddr == 0 || count == 0 {
        return 0;
    }
    let woken = FUTEX_TABLE
        .lock()
        .wake_inner(uaddr, count, FUTEX_BITSET_MATCH_ANY);
    woken as i32
}

// ---------------------------------------------------------------------------
// FUTEX_REQUEUE
// ---------------------------------------------------------------------------

/// Wake `count` waiters on `uaddr`, then move up to `count2` remaining
/// waiters from `uaddr` to `uaddr2`.
///
/// Returns `woken + requeued` (matches Linux convention for sys_futex).
pub fn futex_requeue(uaddr: u64, count: u32, uaddr2: u64, count2: u32) -> i32 {
    if uaddr == 0 {
        return EINVAL;
    }

    let mut table = FUTEX_TABLE.lock();

    // Phase 1: wake `count` waiters on uaddr
    let woken = table.wake_inner(uaddr, count, FUTEX_BITSET_MATCH_ANY);

    // Phase 2: move up to `count2` remaining waiters from uaddr to uaddr2
    let mut requeued = 0u32;
    for i in 0..MAX_WAITERS {
        if requeued >= count2 {
            break;
        }
        let w = &mut table.waiters[i];
        if w.in_use && w.uaddr == uaddr {
            w.uaddr = uaddr2;
            requeued = requeued.saturating_add(1);
        }
    }
    table.total_requeues = table.total_requeues.saturating_add(requeued as u64);

    (woken as i32).saturating_add(requeued as i32)
}

// ---------------------------------------------------------------------------
// FUTEX_CMP_REQUEUE
// ---------------------------------------------------------------------------

/// Conditional requeue: same as `futex_requeue` but first validates
/// that `*uaddr == expected`.  Returns EAGAIN if it does not.
pub fn futex_cmp_requeue(uaddr: u64, count: u32, uaddr2: u64, count2: u32, expected: u32) -> i32 {
    if uaddr == 0 {
        return EINVAL;
    }
    let current = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if current != expected {
        return EAGAIN;
    }
    futex_requeue(uaddr, count, uaddr2, count2)
}

// ---------------------------------------------------------------------------
// FUTEX_WAIT_BITSET / FUTEX_WAKE_BITSET
// ---------------------------------------------------------------------------

/// FUTEX_WAIT_BITSET — like futex_wait but only woken by wakers whose
/// bitset overlaps with `bitset`.
pub fn futex_wait_bitset(uaddr: u64, val: u32, timeout_ns: Option<u64>, bitset: u32) -> i32 {
    if bitset == 0 {
        return EINVAL;
    }
    // Reuse common path, then patch the slot's bitset before releasing the lock.
    // We shadow the FUTEX_BITSET_MATCH_ANY default by inserting directly.
    if uaddr == 0 {
        return EINVAL;
    }

    let current = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    if current != val {
        return EAGAIN;
    }

    let pid = crate::process::getpid();
    let tid = pid;
    let deadline_tsc = match timeout_ns {
        Some(ns) => rdtsc().saturating_add(ns_to_tsc(ns)),
        None => 0,
    };

    let slot = {
        let mut table = FUTEX_TABLE.lock();
        let current2 = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
        if current2 != val {
            return EAGAIN;
        }
        let slot = match table.find_free() {
            Some(s) => s,
            None => return EINVAL,
        };
        table.waiters[slot] = FutexWaiter {
            uaddr,
            pid,
            tid,
            bitset,
            deadline_tsc,
            woken: false,
            in_use: true,
        };
        table.total_waits = table.total_waits.saturating_add(1);
        slot
    };

    {
        let mut sched = crate::process::scheduler::SCHEDULER.lock();
        sched.remove(pid);
    }

    loop {
        core::hint::spin_loop();
        let woken = {
            let table = FUTEX_TABLE.lock();
            !table.waiters[slot].in_use || table.waiters[slot].woken
        };
        if woken {
            let mut table = FUTEX_TABLE.lock();
            table.waiters[slot].in_use = false;
            return 0;
        }
        if deadline_tsc != 0 && rdtsc() >= deadline_tsc {
            let mut table = FUTEX_TABLE.lock();
            table.waiters[slot].in_use = false;
            table.total_timeouts = table.total_timeouts.saturating_add(1);
            crate::process::scheduler::wake_up(pid);
            return ETIMEDOUT;
        }
        crate::process::yield_now();
    }
}

/// FUTEX_WAKE_BITSET — wake up to `count` waiters whose bitset overlaps.
pub fn futex_wake_bitset(uaddr: u64, count: u32, bitset: u32) -> i32 {
    if bitset == 0 {
        return EINVAL;
    }
    let woken = FUTEX_TABLE.lock().wake_inner(uaddr, count, bitset);
    woken as i32
}

// ---------------------------------------------------------------------------
// Cleanup helpers
// ---------------------------------------------------------------------------

/// Remove all waiters belonging to a process that is exiting.
/// Each such waiter has its slot freed and the count is not incremented.
pub fn process_exit(pid: u32) {
    let mut table = FUTEX_TABLE.lock();
    for i in 0..MAX_WAITERS {
        if table.waiters[i].in_use && table.waiters[i].pid == pid {
            table.waiters[i].in_use = false;
        }
    }
}

/// Cancel a specific thread's wait (used on signal delivery or timeout).
/// Returns `true` if a slot was freed.
pub fn cancel_wait(uaddr: u64, pid: u32, tid: u32) -> bool {
    let mut table = FUTEX_TABLE.lock();
    for i in 0..MAX_WAITERS {
        let w = &mut table.waiters[i];
        if w.in_use && w.uaddr == uaddr && w.pid == pid && w.tid == tid {
            w.in_use = false;
            table.total_timeouts = table.total_timeouts.saturating_add(1);
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct FutexStats {
    pub active_waiters: u32,
    pub total_waits: u64,
    pub total_wakes: u64,
    pub total_requeues: u64,
    pub total_timeouts: u64,
}

pub fn stats() -> FutexStats {
    let table = FUTEX_TABLE.lock();
    let active = table.waiters.iter().filter(|w| w.in_use).count() as u32;
    FutexStats {
        active_waiters: active,
        total_waits: table.total_waits,
        total_wakes: table.total_wakes,
        total_requeues: table.total_requeues,
        total_timeouts: table.total_timeouts,
    }
}

/// How many threads are currently waiting on `uaddr`.
pub fn waiter_count(uaddr: u64) -> u32 {
    let table = FUTEX_TABLE.lock();
    table
        .waiters
        .iter()
        .filter(|w| w.in_use && w.uaddr == uaddr)
        .count() as u32
}

// ---------------------------------------------------------------------------
// Syscall dispatch entry point
// ---------------------------------------------------------------------------

/// Handle `SYS_FUTEX` from the syscall dispatch table.
///
/// `uaddr`      — user-space futex address (validated by dispatch)
/// `op`         — FUTEX_* operation code
/// `val`        — expected value (for WAIT) or wake count (for WAKE)
/// `timeout_ns` — optional timeout in nanoseconds
/// `uaddr2`     — second address (REQUEUE only)
/// `val3`       — second count or expected value (REQUEUE / CMP_REQUEUE)
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout_ns: Option<u64>,
    uaddr2: u64,
    val3: u32,
) -> i64 {
    // Strip the private flag — our single-process-group kernel treats
    // private and shared the same way.
    let op_base = op & !(FUTEX_WAIT_PRIVATE ^ FUTEX_WAIT);

    match op_base {
        FUTEX_WAIT | FUTEX_WAIT_PRIVATE => futex_wait(uaddr, val, timeout_ns) as i64,

        FUTEX_WAKE | FUTEX_WAKE_PRIVATE => futex_wake(uaddr, val) as i64,

        FUTEX_REQUEUE => futex_requeue(uaddr, val, uaddr2, val3) as i64,

        FUTEX_CMP_REQUEUE => futex_cmp_requeue(uaddr, val, uaddr2, val3, val3) as i64,

        FUTEX_WAIT_BITSET => futex_wait_bitset(uaddr, val, timeout_ns, val3) as i64,

        FUTEX_WAKE_BITSET => futex_wake_bitset(uaddr, val, val3) as i64,

        _ => EINVAL as i64,
    }
}
