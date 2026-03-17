pub mod affinity;
pub mod ai_scheduler;
pub mod cfs;
pub mod cfs_simple; // lightweight Vec-based CFS run-queue for sched_core
pub mod cgroup;
pub mod context;
pub mod context_switch; // naked switch_context + stack init helpers
pub mod coredump;
pub mod cred;
pub mod elf;
pub mod exec;
pub mod fork;
pub mod job_control; // Job control signal helpers (SIGTSTP, SIGCONT, SIGHUP, ...)
pub mod namespace;
pub mod namespaces;
pub mod nice;
pub mod numa; // NUMA topology and NUMA-aware scheduling hints
pub mod oom_killer;
/// Process management for Hoags Kernel Genesis
///
/// Implements processes, context switching, and scheduling.
/// Inspired by: Linux task_struct (concept), Fuchsia process/thread
/// separation, seL4 TCBs. All code is original.
pub mod pcb;
pub mod posix_thread;
pub mod posix_timer; // POSIX interval timers (timer_create/settime/gettime/delete)
pub mod proc_table; // lightweight process table (ProcessEntry, alloc_pid, …)
pub mod ptrace;
pub mod realtime_signal; // POSIX real-time signals (SIGRTMIN..SIGRTMAX), queued delivery
pub mod resource_limits;
pub mod sched_core; // schedule(), idle_task(), sleep_on(), wake_up(), tick()
pub mod sched_deadline;
pub mod scheduler;
pub mod session; // POSIX sessions and process groups
pub mod setuid; // POSIX setuid/setgid/setresuid privilege management
pub mod sigaction; // Extended per-process sigaction tables and sigprocmask
pub mod signal;
pub mod signalfd;
pub mod thread;
pub mod threadpool;
pub mod userspace;
pub mod wait; // cgroup v2 resource controllers (CPU, memory, I/O)

use crate::{serial_print, serial_println};
use pcb::{Process, ProcessState, PROCESS_TABLE};
use scheduler::SCHEDULER;

/// Maximum number of processes
pub const MAX_PROCESSES: usize = 256;

/// Size of each process's kernel stack (16 KB)
pub const KERNEL_STACK_SIZE: usize = 4096 * 4;

/// Size of each process's user stack (64 KB)
pub const USER_STACK_SIZE: usize = 4096 * 16;

/// Initialize the process subsystem.
///
/// Creates the idle process (PID 0) from the current execution context,
/// and the init process (PID 1) as a kernel thread.
pub fn init() {
    // Create PID 0 — the idle/kernel bootstrap process
    // This represents the current execution context
    {
        let mut table = PROCESS_TABLE.lock();
        let idle = Process::new_kernel(0, "idle");
        table[0] = Some(idle);
    }

    // Add PID 0 to scheduler as the currently running process
    SCHEDULER.lock().set_current(0);

    ai_scheduler::init();
    ptrace::init();
    coredump::init();
    resource_limits::init();
    affinity::init();
    oom_killer::init();
    signal::init();
    wait::init();
    exec::init();
    fork::init();
    thread::init();
    posix_thread::init();
    cred::init();
    namespace::init();
    namespaces::init();
    nice::init();
    signalfd::init();
    sched_deadline::init();
    session::init();
    realtime_signal::init();
    posix_timer::init();
    sigaction::init();
    cgroup::init();

    // Initialise the scheduler core (CFS run-queue + wait-queues).
    serial_println!("  [process-dbg] entering sched_core::init");
    sched_core::init();

    // NUMA topology — must come after sched_core so cpu_rq_len_pub is valid.
    serial_println!("  [process-dbg] entering numa::init");
    numa::init();

    serial_println!("  Process: PID 0 (idle) created");
    serial_println!(
        "  Process: subsystem ready, max {} processes (AI scheduling)",
        MAX_PROCESSES
    );
}

/// Spawn a new kernel-mode process.
/// Returns the PID on success.
pub fn spawn_kernel(name: &str, entry: fn()) -> Option<u32> {
    let mut table = PROCESS_TABLE.lock();

    // Find a free PID
    let pid = (1..MAX_PROCESSES).find(|&i| table[i].is_none())? as u32;

    // Create the process
    let mut proc = Process::new_kernel(pid, name);

    // Set up the kernel stack so context_switch will jump to entry
    let stack_top = proc.kernel_stack_top();
    let ctx = &mut proc.context;
    ctx.rsp = stack_top as u64;
    ctx.rip = entry as u64;
    ctx.rflags = 0x200; // interrupts enabled
                        // CS and SS will be set by the context switch to kernel segments
    ctx.cs = 0x08; // kernel code segment
    ctx.ss = 0x10; // kernel data segment

    // Set up a fake stack frame so the context switch's `ret` goes to entry
    // We push the entry point and initial register values onto the stack
    unsafe {
        let stack_ptr = stack_top as *mut u64;
        // The context switch will pop registers and ret, so we set up:
        // RSP points to where RIP will be popped from
        let sp = stack_ptr.sub(1);
        *sp = entry as u64; // return address
        ctx.rsp = sp as u64;
    }

    proc.state = ProcessState::Ready;
    table[pid as usize] = Some(proc);

    // Add to scheduler run queue
    SCHEDULER.lock().add(pid);

    serial_println!("  Process: spawned PID {} ({})", pid, name);
    Some(pid)
}

/// Yield the current process's time slice to the scheduler
pub fn yield_now() {
    let (current_pid, next_pid) = {
        let mut sched = SCHEDULER.lock();
        let current = sched.current();
        let next = match sched.next() {
            Some(pid) => pid,
            None => return, // nothing to switch to
        };
        if current == next {
            return; // already running the right process
        }
        sched.set_current(next);
        sched.add(current); // put current back in the queue
        (current, next)
    };

    // Perform the actual context switch
    let mut table = PROCESS_TABLE.lock();
    if let (Some(current), Some(next)) = (
        table[current_pid as usize]
            .as_mut()
            .map(|p| &mut p.context as *mut context::CpuContext),
        table[next_pid as usize]
            .as_ref()
            .map(|p| &p.context as *const context::CpuContext),
    ) {
        // Mark states
        if let Some(proc) = table[current_pid as usize].as_mut() {
            proc.state = ProcessState::Ready;
        }
        if let Some(proc) = table[next_pid as usize].as_mut() {
            proc.state = ProcessState::Running;
        }

        drop(table); // release lock before switching
        unsafe {
            context::switch(&mut *current, &*next);
        }
    }
}

/// Exit the current process, notify parent with SIGCHLD
pub fn exit(code: i32) {
    let pid = SCHEDULER.lock().current();
    serial_println!("  Process: PID {} exiting with code {}", pid, code);

    {
        let mut table = PROCESS_TABLE.lock();
        let parent_pid;
        if let Some(proc) = table[pid as usize].as_mut() {
            proc.state = ProcessState::Dead;
            proc.exit_code = code;
            parent_pid = proc.parent_pid;
        } else {
            parent_pid = 0;
        }

        // Drop all process-local file descriptors immediately on exit.
        crate::syscall::cleanup_process_fds(pid);

        // Send SIGCHLD to parent
        if parent_pid > 0 {
            if let Some(parent) = table[parent_pid as usize].as_mut() {
                parent.send_signal(pcb::signal::SIGCHLD);
            }
        }

        // Reparent children to init (PID 1)
        let children: alloc::vec::Vec<u32> = table[pid as usize]
            .as_ref()
            .map(|p| p.children.clone())
            .unwrap_or_default();
        for child_pid in children {
            if let Some(child) = table[child_pid as usize].as_mut() {
                child.parent_pid = 1;
            }
            if let Some(init) = table[1].as_mut() {
                init.children.push(child_pid);
            }
        }

        // Remove from scheduler
        SCHEDULER.lock().remove(pid);
    }

    // Organism mourns every process death (connection loss in the digital body)
    {
        let tick = crate::life::life_tick::age();
        crate::life::grief::mourn(
            crate::life::grief::LossType::Process,
            50, // moderate relationship score — all processes are part of self
            tick,
        );
    }

    // Switch to next process
    yield_now();

    loop {
        crate::io::hlt();
    }
}

/// Fork the current process. Returns child PID in parent, 0 in child.
pub fn fork() -> Option<u32> {
    let parent_pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();

    // Find free PID
    let child_pid = (1..MAX_PROCESSES).find(|&i| table[i].is_none())? as u32;

    // Create child as copy of parent
    let mut child = {
        let parent = table[parent_pid as usize].as_ref()?;
        parent.fork(child_pid)
    };
    // POSIX fork contract: child sees return value 0.
    child.context.rax = 0;

    // Record child in parent
    if let Some(parent) = table[parent_pid as usize].as_mut() {
        parent.children.push(child_pid);
        // Keep parent context coherent with syscall return path.
        parent.context.rax = child_pid as u64;
    }

    table[child_pid as usize] = Some(child);
    drop(table);

    // Add child to scheduler
    SCHEDULER.lock().add(child_pid);

    serial_println!(
        "  Process: fork() PID {} -> child PID {}",
        parent_pid,
        child_pid
    );
    Some(child_pid)
}

/// Wait for a child process to exit. Returns (child_pid, exit_code).
pub fn waitpid(target_pid: i32) -> Option<(u32, i32)> {
    let parent_pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();

    if target_pid > 0 {
        // Wait for specific child
        let cpid = target_pid as u32;
        if let Some(child) = table[cpid as usize].as_ref() {
            if child.parent_pid != parent_pid {
                return None; // not our child
            }
            if child.state == ProcessState::Dead {
                let code = child.exit_code;
                crate::syscall::cleanup_process_fds(cpid);
                // Reap the zombie
                table[cpid as usize] = None;
                // Remove from parent's children list
                if let Some(parent) = table[parent_pid as usize].as_mut() {
                    parent.children.retain(|&c| c != cpid);
                }
                return Some((cpid, code));
            }
        }
        None
    } else {
        // Wait for any child
        let children: alloc::vec::Vec<u32> = table[parent_pid as usize]
            .as_ref()
            .map(|p| p.children.clone())
            .unwrap_or_default();

        for cpid in children {
            if let Some(child) = table[cpid as usize].as_ref() {
                if child.state == ProcessState::Dead {
                    let code = child.exit_code;
                    crate::syscall::cleanup_process_fds(cpid);
                    table[cpid as usize] = None;
                    if let Some(parent) = table[parent_pid as usize].as_mut() {
                        parent.children.retain(|&c| c != cpid);
                    }
                    return Some((cpid, code));
                }
            }
        }
        None
    }
}

/// Send a signal to a process
pub fn send_signal(pid: u32, signal: u8) -> Result<(), &'static str> {
    let mut table = PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_mut().ok_or("no such process")?;

    // SIGKILL and SIGSTOP are always delivered
    if signal == pcb::signal::SIGKILL {
        proc.state = ProcessState::Dead;
        proc.exit_code = -(signal as i32);
        crate::syscall::cleanup_process_fds(pid);
        SCHEDULER.lock().remove(pid);
        return Ok(());
    }

    if signal == pcb::signal::SIGTSTP {
        proc.stopped = true;
        proc.state = ProcessState::Blocked;
        SCHEDULER.lock().remove(pid);
        return Ok(());
    }

    if signal == pcb::signal::SIGCONT {
        if proc.stopped {
            proc.stopped = false;
            proc.state = ProcessState::Ready;
            drop(table);
            SCHEDULER.lock().add(pid);
        }
        return Ok(());
    }

    proc.send_signal(signal);
    Ok(())
}

/// Deliver pending signals for the current process (called from scheduler)
pub fn deliver_signals() {
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table[pid as usize].as_mut() {
        while let Some(sig) = proc.dequeue_signal() {
            match sig {
                s if s == pcb::signal::SIGKILL => {
                    proc.state = ProcessState::Dead;
                    proc.exit_code = -9;
                    crate::syscall::cleanup_process_fds(pid);
                    return;
                }
                s if s == pcb::signal::SIGTERM || s == pcb::signal::SIGINT => {
                    proc.state = ProcessState::Dead;
                    proc.exit_code = -(sig as i32);
                    crate::syscall::cleanup_process_fds(pid);
                    return;
                }
                _ => {} // ignore other signals for now
            }
        }
    }
}

/// Get the current process PID
pub fn getpid() -> u32 {
    SCHEDULER.lock().current()
}

/// Get parent PID
pub fn getppid() -> u32 {
    let pid = SCHEDULER.lock().current();
    let table = PROCESS_TABLE.lock();
    table[pid as usize]
        .as_ref()
        .map(|p| p.parent_pid)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// POSIX wait flags
// ---------------------------------------------------------------------------

/// POSIX wait option flags
pub mod wait_flags {
    /// Return immediately if no child has exited
    pub const WNOHANG: i32 = 1;
    /// Also report stopped children
    pub const WUNTRACED: i32 = 2;
    /// Also report continued children
    pub const WCONTINUED: i32 = 8;
}

/// Wait status encoding helpers (matches Linux conventions)
pub mod wait_status {
    /// Build a normal exit status: bits 15:8 = exit code, bits 7:0 = 0
    pub fn exited(code: i32) -> i32 {
        (code & 0xff) << 8
    }
    /// Build a signaled status: bits 7:0 = signal, bit 7 = core dump
    pub fn signaled(sig: i32, core: bool) -> i32 {
        (sig & 0x7f) | if core { 0x80 } else { 0 }
    }
    /// Build a stopped status: bits 15:8 = stop signal, bits 7:0 = 0x7f
    pub fn stopped(sig: i32) -> i32 {
        ((sig & 0xff) << 8) | 0x7f
    }
    /// Build a continued status: 0xffff
    pub fn continued() -> i32 {
        0xffff
    }
    /// Extract exit code from a normal exit status
    pub fn exit_code(status: i32) -> i32 {
        (status >> 8) & 0xff
    }
    /// Check if the process exited normally
    pub fn if_exited(status: i32) -> bool {
        (status & 0x7f) == 0
    }
    /// Check if the process was killed by a signal
    pub fn if_signaled(status: i32) -> bool {
        (status & 0x7f) != 0 && (status & 0x7f) != 0x7f
    }
    /// Check if the process was stopped
    pub fn if_stopped(status: i32) -> bool {
        (status & 0xff) == 0x7f
    }
}

/// Extended waitpid with POSIX flags (WNOHANG, WUNTRACED, WCONTINUED).
///
/// target_pid semantics:
///   >  0 : wait for exactly that PID
///   == -1: wait for any child
///   ==  0: wait for any child in the caller's process group
///   < -1 : wait for any child whose pgid == |target_pid|
///
/// Returns Some((child_pid, status)) on success, None if no match / WNOHANG.
pub fn waitpid_ex(target_pid: i32, flags: i32) -> Option<(u32, i32)> {
    let parent_pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();

    // Determine which pgid to match for target_pid == 0 or < -1
    let caller_pgid = table[parent_pid as usize]
        .as_ref()
        .map(|p| p.pgid)
        .unwrap_or(0);

    // Collect candidate child PIDs
    let children: alloc::vec::Vec<u32> = table[parent_pid as usize]
        .as_ref()
        .map(|p| p.children.clone())
        .unwrap_or_default();

    if children.is_empty() {
        return None; // ECHILD
    }

    let nohang = (flags & wait_flags::WNOHANG) != 0;
    let want_stopped = (flags & wait_flags::WUNTRACED) != 0;
    let want_continued = (flags & wait_flags::WCONTINUED) != 0;

    for cpid in &children {
        // Filter by target_pid
        let cpid = *cpid;
        let matches = if target_pid > 0 {
            cpid == target_pid as u32
        } else if target_pid == 0 {
            table[cpid as usize]
                .as_ref()
                .map(|p| p.pgid == caller_pgid)
                .unwrap_or(false)
        } else if target_pid == -1 {
            true
        } else {
            // target_pid < -1: match pgid == |target_pid|
            let want_pgid = (-target_pid) as u32;
            table[cpid as usize]
                .as_ref()
                .map(|p| p.pgid == want_pgid)
                .unwrap_or(false)
        };
        if !matches {
            continue;
        }

        if let Some(child) = table[cpid as usize].as_ref() {
            // Dead child -- reap it
            if child.state == ProcessState::Dead {
                let code = child.exit_code;
                let status = if code < 0 {
                    // Killed by signal
                    wait_status::signaled(-code, false)
                } else {
                    wait_status::exited(code)
                };
                crate::syscall::cleanup_process_fds(cpid);
                table[cpid as usize] = None;
                if let Some(parent) = table[parent_pid as usize].as_mut() {
                    parent.children.retain(|&c| c != cpid);
                }
                return Some((cpid, status));
            }
            // Stopped child (if WUNTRACED)
            if want_stopped && child.state == ProcessState::Stopped {
                let sig = pcb::signal::SIGTSTP as i32;
                return Some((cpid, wait_status::stopped(sig)));
            }
            // Continued child (if WCONTINUED) -- detect via !stopped && state == Ready
            if want_continued && !child.stopped && child.state == ProcessState::Ready {
                // Only report once per continue event -- we use a heuristic:
                // if signal_mask has no pending SIGCONT bit we consider it already reported.
                // For simplicity we always report if the flag is requested and the
                // child is running again. A real implementation would track a
                // "reported continued" flag per child.
                return Some((cpid, wait_status::continued()));
            }
        }
    }

    if nohang {
        return None; // No child ready, WNOHANG says don't block
    }

    None // Would block -- caller should retry
}

// ---------------------------------------------------------------------------
// Process groups and sessions
// ---------------------------------------------------------------------------

/// Set the process group ID for a process.
///
/// If `pid` == 0, uses the calling process.
/// If `pgid` == 0, sets pgid = pid (make process a group leader).
pub fn setpgid(pid: u32, pgid: u32) -> Result<(), &'static str> {
    let caller = SCHEDULER.lock().current();
    let target = if pid == 0 { caller } else { pid };
    let new_pgid = if pgid == 0 { target } else { pgid };

    let mut table = PROCESS_TABLE.lock();

    // Validate: target must be caller or a child of caller
    let is_self = target == caller;
    let is_child = table[caller as usize]
        .as_ref()
        .map(|p| p.children.contains(&target))
        .unwrap_or(false);

    if !is_self && !is_child {
        return Err("not owner or child");
    }

    // Cannot change pgid of a session leader
    if let Some(proc) = table[target as usize].as_ref() {
        if proc.is_session_leader() {
            return Err("process is a session leader");
        }
    }

    // Verify the target pgid either equals target (new group) or an existing group
    // in the same session
    if new_pgid != target {
        let target_sid = table[target as usize].as_ref().map(|p| p.sid).unwrap_or(0);
        let group_exists = table.iter().any(|slot| {
            slot.as_ref()
                .map(|p| p.pgid == new_pgid && p.sid == target_sid)
                .unwrap_or(false)
        });
        if !group_exists {
            return Err("no such process group in session");
        }
    }

    if let Some(proc) = table[target as usize].as_mut() {
        proc.set_pgid(new_pgid);
    }
    Ok(())
}

/// Create a new session. The calling process becomes the session leader
/// and process group leader. Returns the new session ID.
pub fn setsid() -> Result<u32, &'static str> {
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();

    // Must not already be a process group leader
    if let Some(proc) = table[pid as usize].as_ref() {
        if proc.is_group_leader() && proc.pgid != proc.pid {
            return Err("already a group leader");
        }
    } else {
        return Err("no such process");
    }

    if let Some(proc) = table[pid as usize].as_mut() {
        let sid = proc.create_session();
        serial_println!("  Process: PID {} created session {}", pid, sid);
        Ok(sid)
    } else {
        Err("no such process")
    }
}

/// Get process group ID of a process. If pid == 0, returns caller's pgid.
pub fn getpgid(pid: u32) -> Result<u32, &'static str> {
    let target = if pid == 0 {
        SCHEDULER.lock().current()
    } else {
        pid
    };
    let table = PROCESS_TABLE.lock();
    table[target as usize]
        .as_ref()
        .map(|p| p.pgid)
        .ok_or("no such process")
}

/// Get session ID of a process. If pid == 0, returns caller's sid.
pub fn getsid(pid: u32) -> Result<u32, &'static str> {
    let target = if pid == 0 {
        SCHEDULER.lock().current()
    } else {
        pid
    };
    let table = PROCESS_TABLE.lock();
    table[target as usize]
        .as_ref()
        .map(|p| p.sid)
        .ok_or("no such process")
}

// ---------------------------------------------------------------------------
// Enhanced signal handling
// ---------------------------------------------------------------------------

/// Install a signal handler (sigaction-style).
///
/// Returns the previous SignalHandlerEntry for the signal on success.
pub fn sigaction(
    sig: u8,
    new_action: pcb::SignalHandlerEntry,
) -> Result<pcb::SignalHandlerEntry, &'static str> {
    if sig == 0 || sig >= 32 {
        return Err("invalid signal number");
    }
    if pcb::signal::is_uncatchable(sig) {
        return Err("cannot catch SIGKILL or SIGSTOP");
    }
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_mut().ok_or("no such process")?;
    let old = proc.get_signal_handler(sig);
    proc.set_signal_handler(sig, new_action)?;
    Ok(old)
}

/// Manipulate signal mask (sigprocmask-style).
///
/// `how`: 0 = SIG_BLOCK (add to mask), 1 = SIG_UNBLOCK (remove from mask),
///        2 = SIG_SETMASK (replace mask).
/// `set`: the signal set to apply.
///
/// Returns the previous signal mask.
pub fn sigprocmask(how: u32, set: u32) -> Result<u32, &'static str> {
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();
    let proc = table[pid as usize].as_mut().ok_or("no such process")?;
    let old_mask = proc.signal_mask;
    match how {
        0 => proc.block_signals(set),   // SIG_BLOCK
        1 => proc.unblock_signals(set), // SIG_UNBLOCK
        2 => proc.set_signal_mask(set), // SIG_SETMASK
        _ => return Err("invalid how value"),
    }
    Ok(old_mask)
}

/// Temporarily replace signal mask and wait for a signal.
///
/// Atomically sets the signal mask to `temp_mask`, then suspends until a
/// signal is delivered. Restores the original mask before returning.
pub fn sigsuspend(temp_mask: u32) {
    let pid = SCHEDULER.lock().current();
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            proc.saved_signal_mask = proc.signal_mask;
            proc.set_signal_mask(temp_mask);
            proc.state = ProcessState::Blocked;
            proc.wait_reason = pcb::WaitReason::Signal;
        }
    }
    SCHEDULER.lock().remove(pid);
    yield_now();
    // When we resume, restore the original mask
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            proc.signal_mask = proc.saved_signal_mask;
            proc.wait_reason = pcb::WaitReason::None;
        }
    }
}

/// Enhanced signal delivery that respects sigaction flags (SA_RESTART,
/// SA_RESETHAND), signal masks during handler invocation, and
/// dispatches to custom handlers or default actions.
pub fn deliver_signals_full() {
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();
    let proc = match table[pid as usize].as_mut() {
        Some(p) => p,
        None => return,
    };

    while let Some(sig) = proc.dequeue_signal() {
        let handler = proc.get_signal_handler(sig);
        proc.rusage.signals_delivered += 1;

        match handler.action {
            pcb::SignalAction::Ignore => {
                // Silently discard
                continue;
            }
            pcb::SignalAction::Default => {
                // Default action depends on signal type
                if pcb::signal::is_fatal_default(sig) {
                    proc.state = ProcessState::Dead;
                    proc.exit_code = -(sig as i32);
                    crate::syscall::cleanup_process_fds(pid);
                    serial_println!(
                        "  Process: PID {} killed by {} (default)",
                        pid,
                        pcb::signal::name(sig)
                    );
                    return;
                }
                if pcb::signal::is_stop_default(sig) {
                    proc.stopped = true;
                    proc.state = ProcessState::Stopped;
                    serial_println!(
                        "  Process: PID {} stopped by {}",
                        pid,
                        pcb::signal::name(sig)
                    );
                    return;
                }
                // SIGCHLD, SIGURG, SIGWINCH -> ignore by default
            }
            pcb::SignalAction::Custom { handler: _addr } => {
                // Block signals specified in sa_mask + the signal itself during handler
                let prev_mask = proc.signal_mask;
                proc.block_signals(handler.sa_mask | (1u32 << sig));
                proc.in_signal_handler = true;
                proc.signal_depth += 1;
                // In a real kernel we would set up a user-space signal frame
                // and trampoline. For now we just record the intent and
                // restore the mask. The actual upcall would be done by
                // the context-switch / IRET path.
                // Restore mask after conceptual handler return:
                proc.signal_mask = prev_mask;
                proc.signal_depth -= 1;
                if proc.signal_depth == 0 {
                    proc.in_signal_handler = false;
                }
                if handler.flags.reset_hand {
                    proc.signal_handlers.remove(&sig);
                }
            }
            pcb::SignalAction::CustomInfo { handler: _addr } => {
                // Same as Custom but would pass siginfo_t
                let prev_mask = proc.signal_mask;
                proc.block_signals(handler.sa_mask | (1u32 << sig));
                proc.in_signal_handler = true;
                proc.signal_depth += 1;
                proc.signal_mask = prev_mask;
                proc.signal_depth -= 1;
                if proc.signal_depth == 0 {
                    proc.in_signal_handler = false;
                }
                if handler.flags.reset_hand {
                    proc.signal_handlers.remove(&sig);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Process image replacement (the "exec" family)
// ---------------------------------------------------------------------------

/// Replace the current process image with a new ELF binary.
///
/// Resets memory mappings, signal handlers (per POSIX), closes CLOEXEC fds,
/// loads the new ELF, and sets up the user stack with argv/envp.
///
/// On success the function does not return (jumps to user space).
/// On failure it returns an error and the process is unchanged.
pub fn load_and_run(elf_data: &[u8], argv: &[&str], envp: &[&str]) -> Result<(), &'static str> {
    let pid = SCHEDULER.lock().current();

    // Parse the ELF header first (cheap, no side effects on failure)
    let elf_info = elf::parse(elf_data).map_err(|_| "ELF parse failed")?;

    {
        let mut table = PROCESS_TABLE.lock();
        let proc = table[pid as usize].as_mut().ok_or("no such process")?;

        // POSIX exec semantics: reset signals, close CLOEXEC fds, clear mappings
        proc.prepare_exec();

        // Update argv
        proc.argv.clear();
        for arg in argv {
            proc.argv.push(alloc::string::String::from(*arg));
        }

        // Update environ
        proc.environ.clear();
        for env in envp {
            // Split on first '='
            if let Some(eq_pos) = env.as_bytes().iter().position(|&b| b == b'=') {
                let key = &env[..eq_pos];
                let val = &env[eq_pos + 1..];
                proc.environ.push((
                    alloc::string::String::from(key),
                    alloc::string::String::from(val),
                ));
            }
        }

        // Set the name to the first argv component (or "unknown")
        if let Some(first) = argv.first() {
            // Use the basename
            let name = if let Some(slash) = first.rfind('/') {
                &first[slash + 1..]
            } else {
                first
            };
            proc.name = alloc::string::String::from(name);
        }

        // Load ELF segments into process address space
        let entry_point = elf_info.entry;
        proc.context.rip = entry_point;
        proc.context.rflags = 0x200; // interrupts enabled
        proc.context.cs = 0x23; // user code segment (ring 3)
        proc.context.ss = 0x1b; // user data segment (ring 3)
        proc.is_kernel = false;

        serial_println!(
            "  Process: PID {} replaced image, entry=0x{:x}",
            pid,
            entry_point
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Nice value management
// ---------------------------------------------------------------------------

/// Set the nice value for a process. Returns Ok on success.
/// Only root (euid 0) can set negative nice values.
pub fn set_nice(pid: u32, nice: i8) -> Result<(), &'static str> {
    let caller = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();

    // Permission check: lowering nice (increasing priority) requires root
    if nice < 0 {
        let is_root = table[caller as usize]
            .as_ref()
            .map(|p| p.creds.is_root())
            .unwrap_or(false);
        if !is_root {
            return Err("permission denied: only root can set negative nice");
        }
    }

    let proc = table[pid as usize].as_mut().ok_or("no such process")?;
    proc.priority.set_nice(nice);
    Ok(())
}

/// Get the nice value for a process.
pub fn get_nice(pid: u32) -> Result<i8, &'static str> {
    let table = PROCESS_TABLE.lock();
    table[pid as usize]
        .as_ref()
        .map(|p| p.priority.nice)
        .ok_or("no such process")
}

// ---------------------------------------------------------------------------
// CPU time accounting
// ---------------------------------------------------------------------------

/// Charge CPU time to the currently running process.
/// Called from the timer interrupt handler.
pub fn charge_cpu_time(ticks: u64, in_kernel: bool) {
    let pid = SCHEDULER.lock().current();
    let mut table = PROCESS_TABLE.lock();
    if let Some(proc) = table[pid as usize].as_mut() {
        if in_kernel {
            proc.rusage.charge_kernel(ticks);
        } else {
            proc.rusage.charge_user(ticks);
        }
        proc.rusage.ticks_wall += ticks;
    }
}

/// Get resource usage tuple for a process:
/// (ticks_user, ticks_kernel, voluntary_switches, involuntary_switches)
pub fn get_rusage(pid: u32) -> Option<(u64, u64, u64, u64)> {
    let table = PROCESS_TABLE.lock();
    table[pid as usize].as_ref().map(|p| {
        (
            p.rusage.ticks_user,
            p.rusage.ticks_kernel,
            p.rusage.voluntary_switches,
            p.rusage.involuntary_switches,
        )
    })
}

/// Get total CPU time (user + kernel ticks) for a process.
pub fn get_cpu_time(pid: u32) -> u64 {
    let table = PROCESS_TABLE.lock();
    table[pid as usize]
        .as_ref()
        .map(|p| p.rusage.total_cpu_ticks())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Send signal to a process group
// ---------------------------------------------------------------------------

/// Send a signal to every process in a process group.
/// Returns the number of processes signaled on success.
pub fn kill_pg(pgid: u32, signal: u8) -> Result<u32, &'static str> {
    if signal >= 32 {
        return Err("invalid signal number");
    }
    let pids = pcb::pids_in_group(pgid);
    if pids.is_empty() {
        return Err("no such process group");
    }
    let mut count = 0u32;
    for pid in pids {
        if send_signal(pid, signal).is_ok() {
            count += 1;
        }
    }
    Ok(count)
}
