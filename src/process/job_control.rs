use super::pcb::{ProcessState, PROCESS_TABLE};
use super::scheduler::SCHEDULER;
use super::MAX_PROCESSES;
/// POSIX Job Control Signal Helpers
///
/// Provides the high-level send/stop/continue primitives used by the shell
/// and the terminal driver for job control.
///
/// Relationship with signal constants:
///   - Signal constants here are `u32` to match the `send_signal_to_pgroup`
///     interface; they are cast to `u8` when calling into `process::send_signal`.
///   - The same numeric values are defined as `u8` constants in
///     `process::pcb::signal` — both sets must stay in sync.
///
/// No heap is used.  All functions operate on the global PROCESS_TABLE
/// (through `process::send_signal`, `process::getpid`, etc.) and on the
/// scheduler run-queue (through `process::pcb::PROCESS_TABLE` directly for
/// state inspection).
use crate::serial_println;

// ---------------------------------------------------------------------------
// Signal number constants (u32 — matches send_signal_to_pgroup interface)
// ---------------------------------------------------------------------------

/// SIGHUP (1): Hangup — sent to a session's processes when the controlling
/// terminal is closed.
pub const SIGHUP: u32 = 1;

/// SIGCONT (18): Continue — resumes a stopped process.
pub const SIGCONT: u32 = 18;

/// SIGTSTP (20): Terminal stop — the interactive stop signal (Ctrl+Z).
pub const SIGTSTP: u32 = 20;

/// SIGTTIN (21): Background read — sent to a background process that attempts
/// to read from the terminal.
pub const SIGTTIN: u32 = 21;

/// SIGTTOU (22): Background write — sent to a background process that attempts
/// to write to the terminal (when TOSTOP is set).
pub const SIGTTOU: u32 = 22;

// ---------------------------------------------------------------------------
// Public job control API
// ---------------------------------------------------------------------------

/// Send SIGTSTP (20) to `pid`, stopping it.
///
/// This is the signal delivered when the user presses Ctrl+Z.  After calling
/// this the process enters `ProcessState::Stopped` and is removed from the
/// scheduler run-queue.  The parent should be notified via SIGCHLD (that
/// notification is handled inside `process::send_signal`).
pub fn raise_sigtstp(pid: u32) {
    let _ = super::send_signal(pid, SIGTSTP as u8);
}

/// Send SIGCONT (18) to `pid`, resuming it if it was stopped.
///
/// If the process is not stopped the signal is still delivered but has no
/// visible effect (POSIX specifies SIGCONT is ignored by a running process
/// unless a custom handler is installed).
pub fn raise_sigcont(pid: u32) {
    let _ = super::send_signal(pid, SIGCONT as u8);
}

/// Send SIGHUP (1) to every live process in session `sid`.
///
/// This is called when the session leader exits or the controlling terminal
/// is closed.  It gives all processes in the session the opportunity to
/// clean up before being terminated.
pub fn raise_sighup_to_session(sid: u32) {
    // Collect PIDs without holding the lock during signal delivery.
    let mut pids = [0u32; 64];
    let mut count = 0usize;

    {
        let table = PROCESS_TABLE.lock();
        for slot in table.iter() {
            if let Some(proc) = slot.as_ref() {
                if proc.sid == sid && proc.state != ProcessState::Dead && count < 64 {
                    pids[count] = proc.pid;
                    count = count.saturating_add(1);
                }
            }
        }
    }

    for i in 0..count {
        let _ = super::send_signal(pids[i], SIGHUP as u8);
    }

    serial_println!(
        "  JobControl: SIGHUP sent to {} process(es) in session {}",
        count,
        sid
    );
}

/// Send SIGTTIN (21) to `pid`.
///
/// Delivered when a background process group member tries to read from the
/// controlling terminal.  Stops the process until it is moved to the
/// foreground or receives SIGCONT.
pub fn raise_sigttin(pid: u32) {
    let _ = super::send_signal(pid, SIGTTIN as u8);
}

/// Send SIGTTOU (22) to `pid`.
///
/// Delivered when a background process group member tries to write to the
/// controlling terminal and the TOSTOP terminal flag is set.
pub fn raise_sigttou(pid: u32) {
    let _ = super::send_signal(pid, SIGTTOU as u8);
}

// ---------------------------------------------------------------------------
// Process stop / resume state primitives
// ---------------------------------------------------------------------------

/// Forcibly stop `pid`: set its state to `Stopped`, mark `stopped = true`,
/// and remove it from the scheduler run-queue.
///
/// Safe to call from any context; uses the spinlock on PROCESS_TABLE.
/// If `pid` is already stopped or dead the call is a no-op.
pub fn stop_process(pid: u32) {
    if pid as usize >= MAX_PROCESSES {
        return;
    }
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            if proc.state == ProcessState::Stopped || proc.state == ProcessState::Dead {
                return;
            }
            proc.stopped = true;
            proc.state = ProcessState::Stopped;
        } else {
            return;
        }
    }
    // Remove from the run-queue; the process will not be scheduled until
    // continue_process() is called.
    SCHEDULER.lock().remove(pid);
    serial_println!("  JobControl: PID {} stopped", pid);
}

/// Resume `pid`: clear the `stopped` flag, set state to `Ready`, and
/// re-add it to the scheduler run-queue.
///
/// If `pid` is not currently stopped the call is a no-op.
pub fn continue_process(pid: u32) {
    if pid as usize >= MAX_PROCESSES {
        return;
    }
    let should_enqueue = {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            if !proc.stopped {
                false
            } else {
                proc.stopped = false;
                proc.state = ProcessState::Ready;
                true
            }
        } else {
            false
        }
    };
    if should_enqueue {
        SCHEDULER.lock().add(pid);
        serial_println!("  JobControl: PID {} continued", pid);
    }
}

/// Returns `true` if `pid` is currently in the `Stopped` state.
pub fn is_stopped(pid: u32) -> bool {
    if pid as usize >= MAX_PROCESSES {
        return false;
    }
    let table = PROCESS_TABLE.lock();
    table[pid as usize]
        .as_ref()
        .map(|p| p.state == ProcessState::Stopped)
        .unwrap_or(false)
}

/// Return an array of all currently stopped PIDs (at most 32 entries).
///
/// Unused slots in the returned array are filled with 0.
/// Inspect the entire array up to the point where values are 0.
pub fn list_stopped_jobs() -> [u32; 32] {
    let mut result = [0u32; 32];
    let mut idx = 0usize;
    let table = PROCESS_TABLE.lock();
    for slot in table.iter() {
        if idx >= 32 {
            break;
        }
        if let Some(proc) = slot.as_ref() {
            if proc.state == ProcessState::Stopped {
                result[idx] = proc.pid;
                idx = idx.saturating_add(1);
            }
        }
    }
    result
}
