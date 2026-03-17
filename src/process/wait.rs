/// wait4/waitpid implementation and zombie process reaping.
///
/// Part of the AIOS kernel.
///
/// Integration summary
/// -------------------
/// `do_wait4` is now fully wired to:
///   - `pcb::PROCESS_TABLE`       — scan children for zombie/stopped/continued
///   - `scheduler::SCHEDULER`     — read current PID
///   - `syscall::cleanup_process_fds` — release FD resources on reap
///
/// Blocking wait (WNOHANG not set and no child ready) logs and returns
/// EAGAIN rather than spinning, because the kernel has no sleep/wakeup
/// queue yet.  TODO(sched): when wait queues are added, block the caller
/// and wake on SIGCHLD instead.
use super::pcb::{ProcessState, PROCESS_TABLE};
use super::scheduler::SCHEDULER;
use crate::process::sched_core::{sleep_on, wake_up};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for the wait4 syscall.
pub struct WaitOptions {
    /// Return immediately (WNOHANG) if no child has changed state.
    pub nohang: bool,
    /// Also report children stopped by a signal (WUNTRACED).
    pub untraced: bool,
    /// Also report children that have been continued by SIGCONT (WCONTINUED).
    pub continued: bool,
}

impl WaitOptions {
    /// Construct from a raw wait4 flags integer (Linux ABI).
    pub fn from_flags(flags: i32) -> Self {
        WaitOptions {
            nohang: (flags & 1) != 0,    // WNOHANG
            untraced: (flags & 2) != 0,  // WUNTRACED
            continued: (flags & 8) != 0, // WCONTINUED
        }
    }
}

/// Outcome of a successful wait4 call.
pub struct WaitResult {
    /// PID of the child that changed state.
    pub pid: u32,
    /// Encoded wait status (use `wait_status::*` helpers to decode).
    pub status: i32,
    /// Child's accumulated user-mode CPU ticks.
    pub rusage_user_ticks: u64,
    /// Child's accumulated kernel-mode CPU ticks.
    pub rusage_kernel_ticks: u64,
}

// ---------------------------------------------------------------------------
// Wait-status encoding (Linux/POSIX conventions)
// ---------------------------------------------------------------------------

/// Helpers to encode/decode the `status` field returned by wait4.
pub mod wait_status {
    /// Normal exit: bits 15:8 = exit_code, bits 7:0 = 0.
    #[inline]
    pub fn exited(code: i32) -> i32 {
        (code & 0xff) << 8
    }

    /// Killed by signal: bits 6:0 = signal number, bit 7 = core-dump flag.
    #[inline]
    pub fn signaled(sig: i32, core_dump: bool) -> i32 {
        (sig & 0x7f) | if core_dump { 0x80 } else { 0 }
    }

    /// Stopped: bits 15:8 = stop signal, bits 7:0 = 0x7f.
    #[inline]
    pub fn stopped(sig: i32) -> i32 {
        ((sig & 0xff) << 8) | 0x7f
    }

    /// Continued (WCONTINUED): 0xffff.
    #[inline]
    pub fn continued() -> i32 {
        0xffff
    }

    /// Extract exit code from a normal-exit status.
    #[inline]
    pub fn exit_code(status: i32) -> i32 {
        (status >> 8) & 0xff
    }

    /// True if the process exited normally.
    #[inline]
    pub fn if_exited(status: i32) -> bool {
        (status & 0x7f) == 0
    }

    /// True if the process was killed by a signal.
    #[inline]
    pub fn if_signaled(status: i32) -> bool {
        let low7 = status & 0x7f;
        low7 != 0 && low7 != 0x7f
    }

    /// True if the process was stopped.
    #[inline]
    pub fn if_stopped(status: i32) -> bool {
        (status & 0xff) == 0x7f
    }
}

// ---------------------------------------------------------------------------
// Zombie reaping
// ---------------------------------------------------------------------------

/// Reap a zombie process: remove its PCB from the table, release its kernel
/// stack, and accumulate its resource usage into the parent.
///
/// Must be called with the PROCESS_TABLE lock **not** held.
pub fn reap_zombie(zombie_pid: u32) {
    crate::serial_println!("[wait] reaping zombie PID {}", zombie_pid);

    // Pull out rusage and parent PID before dropping the PCB.
    let (parent_pid, user_ticks, kernel_ticks) = {
        let table = PROCESS_TABLE.lock();
        if let Some(proc) = table[zombie_pid as usize].as_ref() {
            (
                proc.parent_pid,
                proc.rusage.ticks_user,
                proc.rusage.ticks_kernel,
            )
        } else {
            crate::serial_println!("[wait] reap_zombie: PID {} not in table", zombie_pid);
            return;
        }
    };

    // Close any lingering file descriptors (idempotent if already closed).
    crate::syscall::cleanup_process_fds(zombie_pid);

    // Remove the PCB and explicitly free the kernel stack via the heap
    // allocator.  We extract the stack pointer before dropping the slot.
    let stack_ptr: u64 = {
        let table = PROCESS_TABLE.lock();
        table[zombie_pid as usize]
            .as_ref()
            .map(|p| p.kernel_stack.base as u64)
            .unwrap_or(0)
    };

    {
        let mut table = PROCESS_TABLE.lock();
        table[zombie_pid as usize] = None;
    }

    // Free the kernel stack now that the PCB slot has been cleared.
    free_kernel_stack(stack_ptr);

    // Accumulate child resource usage into parent.
    if parent_pid > 0 {
        let mut table = PROCESS_TABLE.lock();
        if let Some(parent) = table[parent_pid as usize].as_mut() {
            parent.children_rusage.ticks_user =
                parent.children_rusage.ticks_user.saturating_add(user_ticks);
            parent.children_rusage.ticks_kernel = parent
                .children_rusage
                .ticks_kernel
                .saturating_add(kernel_ticks);
            // Remove this child from the parent's children list.
            parent.children.retain(|&c| c != zombie_pid);
        }
    }

    crate::serial_println!("[wait] zombie PID {} reaped", zombie_pid);
}

// ---------------------------------------------------------------------------
// do_wait4
// ---------------------------------------------------------------------------

/// Wait for a child process to change state (POSIX wait4 semantics).
///
/// `pid` semantics:
///   >  0 : wait for exactly that child PID
///   == -1 : wait for any child of the caller
///   ==  0 : wait for any child in the caller's process group
///   < -1  : wait for any child whose pgid == |pid|
///
/// Returns:
///   `Ok(WaitResult)` when a child is found in the requested state.
///   `Err("ECHILD")`  when no matching children exist at all.
///   `Err("EAGAIN")`  when WNOHANG is set and no child is ready yet.
///   `Err("ENOSYS")`  when a blocking wait is requested but the scheduler
///                     sleep/wakeup path is not yet integrated.
pub fn do_wait4(target_pid: i32, options: WaitOptions) -> Result<WaitResult, &'static str> {
    let caller_pid = SCHEDULER.lock().current();
    crate::serial_println!(
        "[wait] do_wait4: caller={} target={} nohang={}",
        caller_pid,
        target_pid,
        options.nohang
    );

    // Determine caller's pgid for pid==0 case.
    let caller_pgid = {
        let table = PROCESS_TABLE.lock();
        table[caller_pid as usize]
            .as_ref()
            .map(|p| p.pgid)
            .unwrap_or(0)
    };

    // Collect the set of child PIDs we care about.
    let children: alloc::vec::Vec<u32> = {
        let table = PROCESS_TABLE.lock();
        table[caller_pid as usize]
            .as_ref()
            .map(|p| p.children.clone())
            .unwrap_or_default()
    };

    if children.is_empty() {
        crate::serial_println!("[wait] caller {} has no children — ECHILD", caller_pid);
        return Err("ECHILD");
    }

    // Scan children for a matching state change.
    let found = {
        let table = PROCESS_TABLE.lock();
        let mut result: Option<WaitResult> = None;

        for &cpid in &children {
            // --- pid filter ---
            let matches = if target_pid > 0 {
                cpid == target_pid as u32
            } else if target_pid == -1 {
                true // any child
            } else if target_pid == 0 {
                table[cpid as usize]
                    .as_ref()
                    .map(|c| c.pgid == caller_pgid)
                    .unwrap_or(false)
            } else {
                // target_pid < -1 : match pgid == |target_pid|
                let want_pgid = ((-target_pid) as u32);
                table[cpid as usize]
                    .as_ref()
                    .map(|c| c.pgid == want_pgid)
                    .unwrap_or(false)
            };

            if !matches {
                continue;
            }

            let child = match table[cpid as usize].as_ref() {
                Some(c) => c,
                None => continue,
            };

            // --- state check ---
            if child.state == ProcessState::Dead {
                // Zombie: encode exit status.
                let status = if child.exit_code < 0 {
                    wait_status::signaled(-child.exit_code, false)
                } else {
                    wait_status::exited(child.exit_code)
                };
                result = Some(WaitResult {
                    pid: cpid,
                    status,
                    rusage_user_ticks: child.rusage.ticks_user,
                    rusage_kernel_ticks: child.rusage.ticks_kernel,
                });
                break;
            }

            if options.untraced && child.state == ProcessState::Stopped {
                // Report stopped child.
                use super::pcb::signal::SIGTSTP;
                result = Some(WaitResult {
                    pid: cpid,
                    status: wait_status::stopped(SIGTSTP as i32),
                    rusage_user_ticks: child.rusage.ticks_user,
                    rusage_kernel_ticks: child.rusage.ticks_kernel,
                });
                break;
            }

            if options.continued && !child.stopped && child.state == ProcessState::Ready {
                // Report continued child (SIGCONT was delivered).
                result = Some(WaitResult {
                    pid: cpid,
                    status: wait_status::continued(),
                    rusage_user_ticks: child.rusage.ticks_user,
                    rusage_kernel_ticks: child.rusage.ticks_kernel,
                });
                break;
            }
        }

        result
    }; // PROCESS_TABLE lock released here

    match found {
        Some(wr) => {
            // Reap zombie if it was a dead child.
            if wait_status::if_exited(wr.status) || wait_status::if_signaled(wr.status) {
                reap_zombie(wr.pid);
            }
            crate::serial_println!(
                "[wait] do_wait4: returning pid={} status=0x{:x}",
                wr.pid,
                wr.status
            );
            Ok(wr)
        }
        None => {
            if options.nohang {
                crate::serial_println!("[wait] do_wait4: WNOHANG, no child ready — EAGAIN");
                Err("EAGAIN")
            } else {
                // Blocking wait: suspend the caller on a channel derived from
                // its PID and re-examine state once woken by SIGCHLD delivery.
                crate::serial_println!(
                    "[wait] do_wait4: no child ready — sleeping on channel pid={}",
                    caller_pid
                );
                // Use `caller_pid as u64` as the wait channel.  The signal
                // delivery path (send_signal SIGCHLD) should call
                // `wake_up(parent_pid as u64)` to unblock the waiter.
                sleep_on(caller_pid as u64, caller_pid);
                // After wakeup, return EAGAIN so the syscall layer retries.
                Err("EAGAIN")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Kernel stack allocator / free
// ---------------------------------------------------------------------------

/// Free a kernel stack that was allocated with `alloc_zeroed`.
///
/// `stack_ptr` must be the base address returned by the allocator (i.e. the
/// lowest address of the 16 KiB allocation, *not* the stack top).  Passing 0
/// is a no-op.
pub fn free_kernel_stack(stack_ptr: u64) {
    if stack_ptr == 0 {
        return;
    }
    use alloc::alloc::{dealloc, Layout};
    // SAFETY: `stack_ptr` is the base of a 16 KiB allocation that was
    // created with `alloc_zeroed(Layout::from_size_align_unchecked(16384, 16))`.
    // Re-creating the identical layout and calling `dealloc` is sound because
    // the pointer came from our own allocator and is no longer reachable (the
    // PCB slot was set to `None` before this call).
    unsafe {
        let layout = Layout::from_size_align_unchecked(16384, 16);
        dealloc(stack_ptr as *mut u8, layout);
    }
    crate::serial_println!("[wait] kernel stack at 0x{:x} freed", stack_ptr);
}

// ---------------------------------------------------------------------------
// Per-process wait queue table
// ---------------------------------------------------------------------------

use crate::sync::Mutex as WqMutex;

/// Maximum number of concurrently tracked per-process wait queues (one per PID).
const WAIT_QUEUE_TABLE_SIZE: usize = 1024;

/// A simple per-process wait queue entry (channel + registered PID).
pub struct WaitQueue {
    pub owner_pid: u32,
    /// Channel value used with `sleep_on` / `wake_up`.
    pub channel: u64,
}

/// Global table of per-process wait queues indexed by PID.
static WAIT_QUEUES: WqMutex<[Option<WaitQueue>; WAIT_QUEUE_TABLE_SIZE]> =
    WqMutex::new([const { None }; WAIT_QUEUE_TABLE_SIZE]);

/// Register a wait queue for `pid`.
///
/// The channel is `pid as u64` so that `wake_up(pid as u64)` can unblock
/// the process.  Callers should use `sleep_on(pid as u64, pid)` to block
/// and `wake_up(pid as u64)` to resume.
pub fn init_wait_queue(pid: u32) {
    if (pid as usize) >= WAIT_QUEUE_TABLE_SIZE {
        crate::serial_println!("[wait] init_wait_queue: PID {} out of bounds", pid);
        return;
    }
    let mut table = WAIT_QUEUES.lock();
    table[pid as usize] = Some(WaitQueue {
        owner_pid: pid,
        channel: pid as u64,
    });
    crate::serial_println!(
        "[wait] init_wait_queue: registered wait queue for PID {}",
        pid
    );
}

// ---------------------------------------------------------------------------
// SIGCHLD wakeup helper
// ---------------------------------------------------------------------------

/// Wake up a process that is sleeping in `do_wait4` waiting for a child.
///
/// Called by the signal delivery path when SIGCHLD is sent to `parent_pid`
/// so that the parent's blocking `do_wait4` can re-examine child states.
pub fn sigchld_wake(parent_pid: u32) {
    wake_up(parent_pid as u64);
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the wait subsystem.
///
/// Registers a wait queue for PID 0 (the idle process) and PID 1 (init) so
/// that the early boot paths have valid queue entries.
pub fn init() {
    init_wait_queue(0);
    init_wait_queue(1);
    crate::serial_println!("  wait: subsystem ready (wait queues initialised)");
}
