/// Process tracing (ptrace) for Genesis
///
/// Implements Linux-style ptrace operations: attach/detach, peek/poke memory,
/// single-step execution, syscall tracing, and software breakpoints.
///
/// The tracer (debugger) attaches to a tracee (target process) and can
/// inspect/modify its registers, memory, and control its execution.
///
/// Inspired by: Linux ptrace(2), GDB stub protocol, OpenBSD pledge.
/// All code is original.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Ptrace request codes ─────────────────────────────────────────────

/// Attach to a process (tracer becomes parent-like controller)
pub const PTRACE_ATTACH: u32 = 1;
/// Detach from a traced process
pub const PTRACE_DETACH: u32 = 2;
/// Read a word from tracee memory
pub const PTRACE_PEEKDATA: u32 = 3;
/// Write a word to tracee memory
pub const PTRACE_POKEDATA: u32 = 4;
/// Read a word from tracee register area
pub const PTRACE_PEEKUSER: u32 = 5;
/// Write a word to tracee register area
pub const PTRACE_POKEUSER: u32 = 6;
/// Resume tracee, stopping at next syscall entry/exit
pub const PTRACE_SYSCALL: u32 = 7;
/// Resume tracee, executing a single instruction
pub const PTRACE_SINGLESTEP: u32 = 8;
/// Resume tracee normally
pub const PTRACE_CONT: u32 = 9;
/// Get all registers
pub const PTRACE_GETREGS: u32 = 10;
/// Set all registers
pub const PTRACE_SETREGS: u32 = 11;
/// Kill the tracee
pub const PTRACE_KILL: u32 = 12;

// ── Trace stop reasons ───────────────────────────────────────────────

/// Why the tracee stopped
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceStopReason {
    /// Stopped after PTRACE_ATTACH
    Attached,
    /// Single-step completed
    SingleStep,
    /// Entering a syscall
    SyscallEntry,
    /// Exiting a syscall
    SyscallExit,
    /// Hit a breakpoint
    Breakpoint { address: usize },
    /// Received a signal
    Signal { signum: u8 },
    /// Tracee exited
    Exited { code: i32 },
}

// ── Trace state per process ──────────────────────────────────────────

/// Per-process trace state (attached to tracee)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceMode {
    /// Not being traced
    None,
    /// Normal tracing (stop on signals)
    Attached,
    /// Stop at next syscall
    SyscallTrace,
    /// Execute one instruction then stop
    SingleStep,
}

/// Software breakpoint record
#[derive(Debug, Clone, Copy)]
pub struct Breakpoint {
    /// Address of the breakpoint
    pub address: usize,
    /// Original byte at that address (replaced with INT3 = 0xCC)
    pub original_byte: u8,
    /// Whether the breakpoint is active
    pub enabled: bool,
    /// Breakpoint ID
    pub id: u32,
}

/// Trace session binding a tracer to a tracee
#[derive(Debug, Clone)]
pub struct TraceSession {
    /// PID of the tracer (debugger)
    pub tracer_pid: u32,
    /// PID of the tracee (target)
    pub tracee_pid: u32,
    /// Current trace mode
    pub mode: TraceMode,
    /// Whether the tracee is currently stopped
    pub stopped: bool,
    /// Reason the tracee last stopped
    pub stop_reason: TraceStopReason,
    /// Breakpoints set on the tracee
    pub breakpoints: Vec<Breakpoint>,
    /// Next breakpoint ID
    pub next_bp_id: u32,
    /// Number of syscalls intercepted
    pub syscall_count: u64,
    /// Number of single-steps executed
    pub singlestep_count: u64,
    /// Whether to suppress signal delivery to tracee
    pub suppress_signals: bool,
    /// Saved register state at stop (register index -> value)
    pub saved_registers: [u64; 21],
}

impl TraceSession {
    /// Create a new trace session
    pub fn new(tracer_pid: u32, tracee_pid: u32) -> Self {
        TraceSession {
            tracer_pid,
            tracee_pid,
            mode: TraceMode::Attached,
            stopped: true,
            stop_reason: TraceStopReason::Attached,
            breakpoints: Vec::new(),
            next_bp_id: 1,
            syscall_count: 0,
            singlestep_count: 0,
            suppress_signals: false,
            saved_registers: [0u64; 21],
        }
    }

    /// Add a breakpoint at the given address
    pub fn add_breakpoint(&mut self, address: usize) -> u32 {
        let id = self.next_bp_id;
        self.next_bp_id = self.next_bp_id.saturating_add(1);

        // Read the original byte at this address
        let original_byte = unsafe { *(address as *const u8) };

        self.breakpoints.push(Breakpoint {
            address,
            original_byte,
            enabled: true,
            id,
        });

        // Write INT3 (0xCC) to trigger debug trap
        unsafe {
            *(address as *mut u8) = 0xCC;
        }

        serial_println!("    [ptrace] breakpoint {} set at {:#X}", id, address);
        id
    }

    /// Remove a breakpoint by ID
    pub fn remove_breakpoint(&mut self, bp_id: u32) -> bool {
        if let Some(pos) = self.breakpoints.iter().position(|bp| bp.id == bp_id) {
            let bp = self.breakpoints.remove(pos);
            if bp.enabled {
                // Restore original byte
                unsafe {
                    *(bp.address as *mut u8) = bp.original_byte;
                }
            }
            serial_println!(
                "    [ptrace] breakpoint {} removed from {:#X}",
                bp_id,
                bp.address
            );
            true
        } else {
            false
        }
    }

    /// Enable or disable a breakpoint
    pub fn set_breakpoint_enabled(&mut self, bp_id: u32, enabled: bool) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.id == bp_id) {
            if enabled && !bp.enabled {
                // Activate: write INT3
                unsafe {
                    *(bp.address as *mut u8) = 0xCC;
                }
            } else if !enabled && bp.enabled {
                // Deactivate: restore original byte
                unsafe {
                    *(bp.address as *mut u8) = bp.original_byte;
                }
            }
            bp.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Check if an address matches a breakpoint
    pub fn find_breakpoint(&self, address: usize) -> Option<&Breakpoint> {
        self.breakpoints
            .iter()
            .find(|bp| bp.address == address && bp.enabled)
    }

    /// Disable all breakpoints (restore original bytes)
    pub fn disable_all_breakpoints(&mut self) {
        for bp in &mut self.breakpoints {
            if bp.enabled {
                unsafe {
                    *(bp.address as *mut u8) = bp.original_byte;
                }
                bp.enabled = false;
            }
        }
    }

    /// Save the current register state from CpuContext
    pub fn save_registers(&mut self, ctx: &super::context::CpuContext) {
        self.saved_registers[0] = ctx.rax;
        self.saved_registers[1] = ctx.rbx;
        self.saved_registers[2] = ctx.rcx;
        self.saved_registers[3] = ctx.rdx;
        self.saved_registers[4] = ctx.rsi;
        self.saved_registers[5] = ctx.rdi;
        self.saved_registers[6] = ctx.rbp;
        self.saved_registers[7] = ctx.r8;
        self.saved_registers[8] = ctx.r9;
        self.saved_registers[9] = ctx.r10;
        self.saved_registers[10] = ctx.r11;
        self.saved_registers[11] = ctx.r12;
        self.saved_registers[12] = ctx.r13;
        self.saved_registers[13] = ctx.r14;
        self.saved_registers[14] = ctx.r15;
        self.saved_registers[15] = ctx.rsp;
        self.saved_registers[16] = ctx.rip;
        self.saved_registers[17] = ctx.rflags;
        self.saved_registers[18] = ctx.cs;
        self.saved_registers[19] = ctx.ss;
        // Index 20 reserved
    }
}

// ── Global trace table ───────────────────────────────────────────────

/// Maximum concurrent trace sessions
const MAX_TRACE_SESSIONS: usize = 32;

/// Global trace session table
pub struct TraceTable {
    sessions: Vec<TraceSession>,
    total_attaches: u64,
    total_detaches: u64,
}

impl TraceTable {
    pub const fn new() -> Self {
        TraceTable {
            sessions: Vec::new(),
            total_attaches: 0,
            total_detaches: 0,
        }
    }

    /// Attach tracer to tracee
    pub fn attach(&mut self, tracer_pid: u32, tracee_pid: u32) -> Result<(), &'static str> {
        // Cannot trace yourself
        if tracer_pid == tracee_pid {
            return Err("cannot trace self");
        }

        // Cannot trace PID 0 (idle) or PID 1 (init)
        if tracee_pid <= 1 {
            return Err("cannot trace kernel processes");
        }

        // Check if already being traced
        if self.sessions.iter().any(|s| s.tracee_pid == tracee_pid) {
            return Err("process already being traced");
        }

        // Enforce session limit
        if self.sessions.len() >= MAX_TRACE_SESSIONS {
            return Err("too many trace sessions");
        }

        let session = TraceSession::new(tracer_pid, tracee_pid);
        self.sessions.push(session);
        self.total_attaches = self.total_attaches.saturating_add(1);

        serial_println!(
            "    [ptrace] PID {} attached to PID {}",
            tracer_pid,
            tracee_pid
        );
        Ok(())
    }

    /// Detach tracer from tracee
    pub fn detach(&mut self, tracee_pid: u32) -> Result<(), &'static str> {
        if let Some(pos) = self
            .sessions
            .iter()
            .position(|s| s.tracee_pid == tracee_pid)
        {
            let mut session = self.sessions.remove(pos);
            // Restore all breakpoints before detaching
            session.disable_all_breakpoints();
            self.total_detaches = self.total_detaches.saturating_add(1);
            serial_println!("    [ptrace] detached from PID {}", tracee_pid);
            Ok(())
        } else {
            Err("process not being traced")
        }
    }

    /// Get a trace session by tracee PID
    pub fn get_session(&self, tracee_pid: u32) -> Option<&TraceSession> {
        self.sessions.iter().find(|s| s.tracee_pid == tracee_pid)
    }

    /// Get a mutable trace session by tracee PID
    pub fn get_session_mut(&mut self, tracee_pid: u32) -> Option<&mut TraceSession> {
        self.sessions
            .iter_mut()
            .find(|s| s.tracee_pid == tracee_pid)
    }

    /// Check if a process is being traced
    pub fn is_traced(&self, pid: u32) -> bool {
        self.sessions.iter().any(|s| s.tracee_pid == pid)
    }

    /// Get all sessions where a given PID is the tracer
    pub fn sessions_for_tracer(&self, tracer_pid: u32) -> Vec<u32> {
        self.sessions
            .iter()
            .filter(|s| s.tracer_pid == tracer_pid)
            .map(|s| s.tracee_pid)
            .collect()
    }

    /// Remove all trace sessions involving a PID (as tracer or tracee)
    pub fn cleanup_pid(&mut self, pid: u32) {
        // Disable breakpoints for sessions where pid is tracee
        for session in self.sessions.iter_mut() {
            if session.tracee_pid == pid {
                session.disable_all_breakpoints();
            }
        }
        self.sessions
            .retain(|s| s.tracer_pid != pid && s.tracee_pid != pid);
    }

    /// Number of active trace sessions
    pub fn active_sessions(&self) -> usize {
        self.sessions.len()
    }
}

static TRACE_TABLE: Mutex<TraceTable> = Mutex::new(TraceTable::new());

// ── Public API ───────────────────────────────────────────────────────

/// Peek a word from tracee memory
pub fn peek_data(tracee_pid: u32, address: usize) -> Result<u64, &'static str> {
    let table = TRACE_TABLE.lock();
    if !table.is_traced(tracee_pid) {
        return Err("process not being traced");
    }
    // Read 8 bytes from the tracee's address space
    let value = unsafe { *(address as *const u64) };
    Ok(value)
}

/// Poke (write) a word into tracee memory
pub fn poke_data(tracee_pid: u32, address: usize, value: u64) -> Result<(), &'static str> {
    let table = TRACE_TABLE.lock();
    if !table.is_traced(tracee_pid) {
        return Err("process not being traced");
    }
    // Write 8 bytes to the tracee's address space
    unsafe {
        *(address as *mut u64) = value;
    }
    Ok(())
}

/// Request single-step mode for a tracee
pub fn request_singlestep(tracee_pid: u32) -> Result<(), &'static str> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(tracee_pid).ok_or("not traced")?;

    if !session.stopped {
        return Err("tracee not stopped");
    }

    session.mode = TraceMode::SingleStep;
    session.stopped = false;
    session.singlestep_count += 1;

    // Set the Trap Flag (TF) in RFLAGS to trigger #DB after one instruction
    // Bit 8 of RFLAGS
    session.saved_registers[17] |= 1 << 8;

    serial_println!("    [ptrace] single-step requested for PID {}", tracee_pid);
    Ok(())
}

/// Request syscall tracing mode for a tracee
pub fn request_syscall_trace(tracee_pid: u32) -> Result<(), &'static str> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(tracee_pid).ok_or("not traced")?;

    if !session.stopped {
        return Err("tracee not stopped");
    }

    session.mode = TraceMode::SyscallTrace;
    session.stopped = false;
    session.syscall_count += 1;

    serial_println!("    [ptrace] syscall trace mode for PID {}", tracee_pid);
    Ok(())
}

/// Continue a stopped tracee
pub fn continue_tracee(tracee_pid: u32) -> Result<(), &'static str> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(tracee_pid).ok_or("not traced")?;

    if !session.stopped {
        return Err("tracee not stopped");
    }

    session.mode = TraceMode::Attached;
    session.stopped = false;

    // Clear Trap Flag if it was set
    session.saved_registers[17] &= !(1u64 << 8);

    Ok(())
}

/// Handle a debug trap (#DB exception) for a traced process
pub fn handle_debug_trap(pid: u32, rip: usize) -> Option<TraceStopReason> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(pid)?;

    // Check if this is a breakpoint (RIP-1 because INT3 increments RIP)
    let bp_addr = rip.wrapping_sub(1);
    if let Some(_bp) = session.find_breakpoint(bp_addr) {
        session.stopped = true;
        session.stop_reason = TraceStopReason::Breakpoint { address: bp_addr };
        return Some(TraceStopReason::Breakpoint { address: bp_addr });
    }

    // Otherwise it is a single-step trap
    if session.mode == TraceMode::SingleStep {
        session.stopped = true;
        session.stop_reason = TraceStopReason::SingleStep;
        return Some(TraceStopReason::SingleStep);
    }

    None
}

/// Handle a syscall entry/exit for a traced process
pub fn handle_syscall(pid: u32, is_entry: bool) -> Option<TraceStopReason> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(pid)?;

    if session.mode != TraceMode::SyscallTrace {
        return None;
    }

    let reason = if is_entry {
        TraceStopReason::SyscallEntry
    } else {
        TraceStopReason::SyscallExit
    };

    session.stopped = true;
    session.stop_reason = reason;
    session.syscall_count += 1;

    Some(reason)
}

/// Dispatch a ptrace request
pub fn ptrace(
    request: u32,
    tracer_pid: u32,
    tracee_pid: u32,
    addr: usize,
    data: u64,
) -> Result<u64, &'static str> {
    match request {
        PTRACE_ATTACH => {
            TRACE_TABLE.lock().attach(tracer_pid, tracee_pid)?;
            Ok(0)
        }
        PTRACE_DETACH => {
            TRACE_TABLE.lock().detach(tracee_pid)?;
            Ok(0)
        }
        PTRACE_PEEKDATA => peek_data(tracee_pid, addr),
        PTRACE_POKEDATA => {
            poke_data(tracee_pid, addr, data)?;
            Ok(0)
        }
        PTRACE_SINGLESTEP => {
            request_singlestep(tracee_pid)?;
            Ok(0)
        }
        PTRACE_SYSCALL => {
            request_syscall_trace(tracee_pid)?;
            Ok(0)
        }
        PTRACE_CONT => {
            continue_tracee(tracee_pid)?;
            Ok(0)
        }
        PTRACE_KILL => {
            TRACE_TABLE.lock().detach(tracee_pid).ok();
            // Delegate to process kill
            super::send_signal(tracee_pid, super::pcb::signal::SIGKILL).ok();
            Ok(0)
        }
        _ => Err("unknown ptrace request"),
    }
}

/// Add a breakpoint on a traced process
pub fn add_breakpoint(tracee_pid: u32, address: usize) -> Result<u32, &'static str> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(tracee_pid).ok_or("not traced")?;
    if !session.stopped {
        return Err("tracee must be stopped to set breakpoint");
    }
    Ok(session.add_breakpoint(address))
}

/// Remove a breakpoint from a traced process
pub fn remove_breakpoint(tracee_pid: u32, bp_id: u32) -> Result<(), &'static str> {
    let mut table = TRACE_TABLE.lock();
    let session = table.get_session_mut(tracee_pid).ok_or("not traced")?;
    if session.remove_breakpoint(bp_id) {
        Ok(())
    } else {
        Err("breakpoint not found")
    }
}

/// Clean up all trace sessions for a process that is exiting
pub fn cleanup_for_exit(pid: u32) {
    TRACE_TABLE.lock().cleanup_pid(pid);
}

/// Get trace statistics
pub fn stats() -> (usize, u64, u64) {
    let table = TRACE_TABLE.lock();
    (
        table.active_sessions(),
        table.total_attaches,
        table.total_detaches,
    )
}

/// Initialize the ptrace subsystem
pub fn init() {
    serial_println!(
        "    [ptrace] process tracing initialized (attach, peek/poke, breakpoints, single-step)"
    );
}

// ── Typed ptrace API ─────────────────────────────────────────────────────────

/// Typed representation of a ptrace request.
///
/// This is the higher-level, type-safe interface layered on top of the
/// raw `u32`-based `ptrace()` dispatcher above.  Callers that construct
/// requests programmatically should prefer this API.
#[derive(Debug, Clone, Copy)]
pub enum PtraceRequest {
    /// Attach to the target process; the caller becomes its tracer.
    Attach,
    /// Detach from the target process, restoring all breakpoints.
    Detach,
    /// Read 8 bytes from the target process's virtual address space.
    PeekData { addr: u64 },
    /// Write 8 bytes into the target process's virtual address space.
    PokeData { addr: u64, data: u64 },
    /// Copy the target's saved general-purpose register state.
    GetRegs,
    /// Restore register state into the target's saved context.
    SetRegs,
    /// Execute one instruction in the target then stop again.
    SingleStep,
    /// Resume the target normally (clear single-step, stay attached).
    Continue,
    /// Kill the target process (detach and send SIGKILL).
    Kill,
}

/// Typed ptrace dispatcher.
///
/// Parameters:
/// - `request`  — the operation to perform
/// - `pid`      — the target (tracee) PID
/// - `addr`     — address operand (used by `PeekData` / `PokeData`)
/// - `data`     — data operand (used by `PokeData` / `SetRegs`)
///
/// The `tracer_pid` is resolved automatically from the current process.
///
/// Returns `Ok(value)` on success (value is meaningful only for `PeekData`
/// and `GetRegs`).  Returns `Err(errno)` as a Linux-compatible negative
/// error code on failure.
pub fn ptrace_typed(request: PtraceRequest, pid: u32, addr: u64, data: u64) -> Result<i64, i32> {
    // Resolve the current process as the tracer.
    let tracer_pid = crate::process::scheduler::SCHEDULER.lock().current();

    match request {
        // ── Attach ───────────────────────────────────────────────────────────
        // Sets the PCB stopped flag, adds a trace session, and sends SIGSTOP.
        PtraceRequest::Attach => {
            TRACE_TABLE
                .lock()
                .attach(tracer_pid, pid)
                .map_err(|_| -1i32)?;

            // Mark the tracee as stopped in the PCB.
            {
                let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_mut() {
                    proc.stopped = true;
                    proc.state = crate::process::pcb::ProcessState::Traced;
                }
            }

            // Send SIGSTOP so the tracee halts at the next opportunity.
            super::send_signal(pid, super::pcb::signal::SIGSTOP).map_err(|_| -1i32)?;

            serial_println!("    [ptrace] ATTACH: tracer={} tracee={}", tracer_pid, pid);
            Ok(0)
        }

        // ── Detach ───────────────────────────────────────────────────────────
        PtraceRequest::Detach => {
            TRACE_TABLE.lock().detach(pid).map_err(|_| -1i32)?;

            // Resume the tracee.
            {
                let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_mut() {
                    if proc.state == crate::process::pcb::ProcessState::Traced {
                        proc.stopped = false;
                        proc.state = crate::process::pcb::ProcessState::Ready;
                    }
                }
            }

            serial_println!("    [ptrace] DETACH: tracee={}", pid);
            Ok(0)
        }

        // ── PeekData ─────────────────────────────────────────────────────────
        // Read 8 bytes from the tracee's address space.
        //
        // In a full implementation this would switch to the tracee's page
        // tables (CR3), read the word at `addr`, then switch back.  For now
        // we share a single kernel address space, so a direct pointer read
        // is correct for kernel addresses; user-space addresses would require
        // the page-table switch.
        PtraceRequest::PeekData { addr } => {
            let _table = TRACE_TABLE.lock();
            if !_table.is_traced(pid) {
                return Err(-3); // ESRCH
            }
            drop(_table);

            // SAFETY: the caller is responsible for providing a valid address.
            let value = unsafe { core::ptr::read_volatile(addr as *const u64) };
            Ok(value as i64)
        }

        // ── PokeData ─────────────────────────────────────────────────────────
        // Write 8 bytes into the tracee's virtual address space.
        PtraceRequest::PokeData { addr, data } => {
            let _table = TRACE_TABLE.lock();
            if !_table.is_traced(pid) {
                return Err(-3); // ESRCH
            }
            drop(_table);

            // SAFETY: caller responsibility.
            unsafe { core::ptr::write_volatile(addr as *mut u64, data) };
            Ok(0)
        }

        // ── GetRegs ──────────────────────────────────────────────────────────
        // Copy the saved register state from the target PCB into `saved_registers`
        // of the active trace session, then write them to the address pointed to
        // by `data` (treated as a pointer to a [u64; 21] array in kernel memory).
        PtraceRequest::GetRegs => {
            // Snapshot register state into the session's saved_registers array.
            {
                let table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_ref() {
                    let ctx = &proc.context;
                    let mut tt = TRACE_TABLE.lock();
                    if let Some(session) = tt.get_session_mut(pid) {
                        session.save_registers(ctx);
                    }
                }
            }

            // If `data` is a non-null kernel pointer, copy the registers there.
            if data != 0 {
                let session_regs = {
                    let tt = TRACE_TABLE.lock();
                    tt.get_session(pid).map(|s| s.saved_registers)
                };
                if let Some(regs) = session_regs {
                    // SAFETY: caller guarantees `data` points to a [u64; 21].
                    let dst = data as *mut [u64; 21];
                    unsafe { *dst = regs };
                }
            }

            Ok(0)
        }

        // ── SetRegs ──────────────────────────────────────────────────────────
        // Restore register state from `data` pointer into the target PCB.
        PtraceRequest::SetRegs => {
            if data == 0 {
                return Err(-22); // EINVAL
            }

            // SAFETY: caller guarantees `data` points to a [u64; 21].
            let regs: [u64; 21] = unsafe { *(data as *const [u64; 21]) };

            let mut table = crate::process::pcb::PROCESS_TABLE.lock();
            if let Some(proc) = table[pid as usize].as_mut() {
                let ctx = &mut proc.context;
                ctx.rax = regs[0];
                ctx.rbx = regs[1];
                ctx.rcx = regs[2];
                ctx.rdx = regs[3];
                ctx.rsi = regs[4];
                ctx.rdi = regs[5];
                ctx.rbp = regs[6];
                ctx.r8 = regs[7];
                ctx.r9 = regs[8];
                ctx.r10 = regs[9];
                ctx.r11 = regs[10];
                ctx.r12 = regs[11];
                ctx.r13 = regs[12];
                ctx.r14 = regs[13];
                ctx.r15 = regs[14];
                ctx.rsp = regs[15];
                ctx.rip = regs[16];
                ctx.rflags = regs[17];
                ctx.cs = regs[18];
                ctx.ss = regs[19];
                // regs[20] reserved
            }

            Ok(0)
        }

        // ── SingleStep ───────────────────────────────────────────────────────
        // Set the Trap Flag (TF, RFLAGS bit 8) in the tracee's saved RFLAGS and
        // resume it.  The CPU will raise a #DB exception after the next instruction,
        // which the debug-trap handler delivers back as a SIGTRAP stop.
        PtraceRequest::SingleStep => {
            {
                // Set TF in the saved context.
                let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_mut() {
                    proc.context.rflags |= 1 << 8; // TF bit
                    proc.stopped = false;
                    if proc.state == crate::process::pcb::ProcessState::Traced {
                        proc.state = crate::process::pcb::ProcessState::Ready;
                    }
                }
            }

            // Update the trace session mode.
            {
                let mut tt = TRACE_TABLE.lock();
                if let Some(session) = tt.get_session_mut(pid) {
                    session.mode = TraceMode::SingleStep;
                    session.stopped = false;
                    session.singlestep_count = session.singlestep_count.saturating_add(1);
                    // Mirror TF into the session's saved RFLAGS.
                    session.saved_registers[17] |= 1 << 8;
                }
            }

            // Re-add the tracee to the run queue so it executes one instruction.
            crate::process::scheduler::SCHEDULER.lock().add(pid);

            serial_println!("    [ptrace] SINGLESTEP: tracee={}", pid);
            Ok(0)
        }

        // ── Continue ─────────────────────────────────────────────────────────
        // Clear the Trap Flag, mark tracee as Attached (not single-step), resume.
        PtraceRequest::Continue => {
            {
                let mut table = crate::process::pcb::PROCESS_TABLE.lock();
                if let Some(proc) = table[pid as usize].as_mut() {
                    proc.context.rflags &= !(1u64 << 8); // clear TF
                    proc.stopped = false;
                    if proc.state == crate::process::pcb::ProcessState::Traced
                        || proc.state == crate::process::pcb::ProcessState::Blocked
                    {
                        proc.state = crate::process::pcb::ProcessState::Ready;
                    }
                }
            }

            {
                let mut tt = TRACE_TABLE.lock();
                if let Some(session) = tt.get_session_mut(pid) {
                    session.mode = TraceMode::Attached;
                    session.stopped = false;
                    session.saved_registers[17] &= !(1u64 << 8);
                }
            }

            crate::process::scheduler::SCHEDULER.lock().add(pid);
            serial_println!("    [ptrace] CONTINUE: tracee={}", pid);
            Ok(0)
        }

        // ── Kill ─────────────────────────────────────────────────────────────
        // Detach from the tracee (restoring breakpoints) then send SIGKILL.
        PtraceRequest::Kill => {
            TRACE_TABLE.lock().detach(pid).ok();
            super::send_signal(pid, super::pcb::signal::SIGKILL).map_err(|_| -3i32)?;
            serial_println!("    [ptrace] KILL: tracee={}", pid);
            Ok(0)
        }
    }
}
