/// ptrace — process tracing
///
/// Implements the ptrace(2) subsystem that lets one process (tracer)
/// inspect and control another (tracee): attach, detach, read/write
/// memory and registers, single-step, syscall tracing.
///
/// Operations:
///   PTRACE_ATTACH       — tracer attaches to tracee (sends SIGSTOP)
///   PTRACE_DETACH       — tracer releases tracee (optionally resumes)
///   PTRACE_PEEKDATA     — read one word from tracee's address space
///   PTRACE_POKEDATA     — write one word to tracee's address space
///   PTRACE_GETREGS      — copy tracee register set to tracer
///   PTRACE_SETREGS      — install new register set in tracee
///   PTRACE_CONT         — resume tracee until next stop
///   PTRACE_SINGLESTEP   — set TF; tracee stops after next instruction
///   PTRACE_SYSCALL      — stop at each syscall entry/exit
///   PTRACE_KILL         — send SIGKILL to tracee
///
/// Design:
///   - TraceeEntry: per-traced-process state (tracer_pid, flags, saved regs)
///   - Static table of 64 tracees
///   - Peek/Poke are stubs — real implementation calls into the VMM
///   - Register get/set operate on the saved X64Regs from interrupt frame
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// Reuse the register struct from coredump
use super::coredump::X64Regs;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const PTRACE_MAX_TRACEES: usize = 64;

// ptrace request codes
pub const PTRACE_ATTACH: u32 = 16;
pub const PTRACE_DETACH: u32 = 17;
pub const PTRACE_PEEKDATA: u32 = 2;
pub const PTRACE_POKEDATA: u32 = 5;
pub const PTRACE_GETREGS: u32 = 12;
pub const PTRACE_SETREGS: u32 = 13;
pub const PTRACE_CONT: u32 = 7;
pub const PTRACE_SINGLESTEP: u32 = 9;
pub const PTRACE_SYSCALL: u32 = 24;
pub const PTRACE_KILL: u32 = 8;
pub const PTRACE_SETOPTIONS: u32 = 0x4200;
pub const PTRACE_GETEVENTMSG: u32 = 0x4201;

// ptrace options (bitmask for PTRACE_SETOPTIONS)
pub const PTRACE_O_TRACESYSGOOD: u32 = 0x0001;
pub const PTRACE_O_TRACEFORK: u32 = 0x0002;
pub const PTRACE_O_TRACECLONE: u32 = 0x0008;
pub const PTRACE_O_TRACEEXEC: u32 = 0x0010;
pub const PTRACE_O_TRACEEXIT: u32 = 0x0040;

// Stop reasons
pub const PTRACE_STOP_NONE: u8 = 0;
pub const PTRACE_STOP_SIGNAL: u8 = 1;
pub const PTRACE_STOP_SYSCALL: u8 = 2;
pub const PTRACE_STOP_SINGLESTEP: u8 = 3;
pub const PTRACE_STOP_FORK: u8 = 4;
pub const PTRACE_STOP_EXEC: u8 = 5;
pub const PTRACE_STOP_EXIT: u8 = 6;

// Error codes
pub const PTRACE_OK: i64 = 0;
pub const PTRACE_EPERM: i64 = -1;
pub const PTRACE_ESRCH: i64 = -3;
pub const PTRACE_EINVAL: i64 = -22;
pub const PTRACE_EBUSY: i64 = -16;

// ---------------------------------------------------------------------------
// Tracee state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TraceeState {
    Empty,
    Running,
    Stopped,
    Killed,
}

// ---------------------------------------------------------------------------
// Tracee entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct TraceeEntry {
    pub tracee_pid: u32,
    pub tracer_pid: u32,
    pub state: TraceeState,
    pub stop_reason: u8,
    pub stop_signal: u8,
    pub syscall_trap: bool,
    pub singlestep: bool,
    pub options: u32,
    pub event_msg: u64,
    pub saved_regs: X64Regs,
    pub valid: bool,
}

impl TraceeEntry {
    pub const fn empty() -> Self {
        TraceeEntry {
            tracee_pid: 0,
            tracer_pid: 0,
            state: TraceeState::Empty,
            stop_reason: PTRACE_STOP_NONE,
            stop_signal: 0,
            syscall_trap: false,
            singlestep: false,
            options: 0,
            event_msg: 0,
            saved_regs: X64Regs::zero(),
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static table
// ---------------------------------------------------------------------------

static TRACEES: Mutex<[TraceeEntry; PTRACE_MAX_TRACEES]> =
    Mutex::new([TraceeEntry::empty(); PTRACE_MAX_TRACEES]);
static TRACEE_COUNT: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_slot_by_tracee(table: &[TraceeEntry; PTRACE_MAX_TRACEES], pid: u32) -> Option<usize> {
    let mut i = 0usize;
    while i < PTRACE_MAX_TRACEES {
        if table[i].valid && table[i].tracee_pid == pid {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

fn find_empty_slot(table: &[TraceeEntry; PTRACE_MAX_TRACEES]) -> Option<usize> {
    let mut i = 0usize;
    while i < PTRACE_MAX_TRACEES {
        if !table[i].valid {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// PTRACE_ATTACH
// ---------------------------------------------------------------------------

/// Attach tracer `tracer_pid` to tracee `tracee_pid`.
pub fn ptrace_attach(tracer_pid: u32, tracee_pid: u32) -> i64 {
    if tracer_pid == tracee_pid {
        return PTRACE_EPERM;
    } // can't trace self
    let mut table = TRACEES.lock();
    // Already being traced?
    if find_slot_by_tracee(&table, tracee_pid).is_some() {
        return PTRACE_EBUSY;
    }
    let slot = match find_empty_slot(&table) {
        Some(s) => s,
        None => return PTRACE_EINVAL,
    };
    table[slot] = TraceeEntry::empty();
    table[slot].tracee_pid = tracee_pid;
    table[slot].tracer_pid = tracer_pid;
    table[slot].state = TraceeState::Stopped;
    table[slot].stop_reason = PTRACE_STOP_SIGNAL;
    table[slot].stop_signal = 19; // SIGSTOP
    table[slot].valid = true;
    TRACEE_COUNT.fetch_add(1, Ordering::Relaxed);
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_DETACH
// ---------------------------------------------------------------------------

pub fn ptrace_detach(tracer_pid: u32, tracee_pid: u32) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    table[slot] = TraceeEntry::empty();
    TRACEE_COUNT.fetch_sub(1, Ordering::Relaxed);
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_PEEKDATA / PTRACE_POKEDATA
// ---------------------------------------------------------------------------

/// Read one 64-bit word from tracee's virtual address.
/// In a real kernel this would walk the tracee's page tables.
/// Stub: returns 0 (address is accepted, value unknown).
pub fn ptrace_peekdata(_tracer_pid: u32, tracee_pid: u32, _addr: u64) -> i64 {
    let table = TRACEES.lock();
    if find_slot_by_tracee(&table, tracee_pid).is_none() {
        return PTRACE_ESRCH;
    }
    0 // stub
}

/// Write one 64-bit word to tracee's virtual address.
/// Stub: silently accepts.
pub fn ptrace_pokedata(_tracer_pid: u32, tracee_pid: u32, _addr: u64, _data: u64) -> i64 {
    let table = TRACEES.lock();
    if find_slot_by_tracee(&table, tracee_pid).is_none() {
        return PTRACE_ESRCH;
    }
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_GETREGS / PTRACE_SETREGS
// ---------------------------------------------------------------------------

pub fn ptrace_getregs(tracer_pid: u32, tracee_pid: u32) -> Option<X64Regs> {
    let table = TRACEES.lock();
    let slot = find_slot_by_tracee(&table, tracee_pid)?;
    if table[slot].tracer_pid != tracer_pid {
        return None;
    }
    if table[slot].state != TraceeState::Stopped {
        return None;
    }
    Some(table[slot].saved_regs)
}

pub fn ptrace_setregs(tracer_pid: u32, tracee_pid: u32, regs: X64Regs) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    if table[slot].state != TraceeState::Stopped {
        return PTRACE_EBUSY;
    }
    table[slot].saved_regs = regs;
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_CONT
// ---------------------------------------------------------------------------

pub fn ptrace_cont(tracer_pid: u32, tracee_pid: u32, signal: u8) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    table[slot].state = TraceeState::Running;
    table[slot].stop_reason = PTRACE_STOP_NONE;
    table[slot].stop_signal = signal;
    table[slot].singlestep = false;
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_SINGLESTEP
// ---------------------------------------------------------------------------

pub fn ptrace_singlestep(tracer_pid: u32, tracee_pid: u32, signal: u8) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    // Set TF in saved EFLAGS
    table[slot].saved_regs.rflags |= 0x100;
    table[slot].state = TraceeState::Running;
    table[slot].singlestep = true;
    table[slot].stop_signal = signal;
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_SYSCALL
// ---------------------------------------------------------------------------

pub fn ptrace_syscall(tracer_pid: u32, tracee_pid: u32, signal: u8) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    table[slot].state = TraceeState::Running;
    table[slot].syscall_trap = true;
    table[slot].stop_signal = signal;
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// PTRACE_SETOPTIONS
// ---------------------------------------------------------------------------

pub fn ptrace_setoptions(tracer_pid: u32, tracee_pid: u32, options: u32) -> i64 {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return PTRACE_ESRCH,
    };
    if table[slot].tracer_pid != tracer_pid {
        return PTRACE_EPERM;
    }
    table[slot].options = options;
    PTRACE_OK
}

// ---------------------------------------------------------------------------
// Kernel-side hooks (called from scheduler/syscall dispatch)
// ---------------------------------------------------------------------------

/// Called on every syscall entry/exit when syscall_trap is set.
/// Stops the tracee and notifies tracer.
pub fn ptrace_notify_syscall(tracee_pid: u32, syscall_nr: u64) {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return,
    };
    if !table[slot].syscall_trap {
        return;
    }
    table[slot].state = TraceeState::Stopped;
    table[slot].stop_reason = PTRACE_STOP_SYSCALL;
    table[slot].event_msg = syscall_nr;
}

/// Called from debug exception handler when TF is set (single-step fires).
pub fn ptrace_notify_step(tracee_pid: u32) {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return,
    };
    if !table[slot].singlestep {
        return;
    }
    table[slot].saved_regs.rflags &= !0x100u64; // clear TF
    table[slot].singlestep = false;
    table[slot].state = TraceeState::Stopped;
    table[slot].stop_reason = PTRACE_STOP_SINGLESTEP;
}

/// Called when tracee exits — record exit code.
pub fn ptrace_notify_exit(tracee_pid: u32, exit_code: i32) {
    let mut table = TRACEES.lock();
    let slot = match find_slot_by_tracee(&table, tracee_pid) {
        Some(s) => s,
        None => return,
    };
    if table[slot].options & PTRACE_O_TRACEEXIT == 0 {
        return;
    }
    table[slot].state = TraceeState::Stopped;
    table[slot].stop_reason = PTRACE_STOP_EXIT;
    table[slot].event_msg = exit_code as u64;
}

// ---------------------------------------------------------------------------
// Cleanup on process death
// ---------------------------------------------------------------------------

pub fn ptrace_cleanup_pid(pid: u32) {
    let mut table = TRACEES.lock();
    let mut i = 0usize;
    while i < PTRACE_MAX_TRACEES {
        if table[i].valid && (table[i].tracee_pid == pid || table[i].tracer_pid == pid) {
            table[i] = TraceeEntry::empty();
            TRACEE_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Unified ptrace dispatcher — mirrors the ptrace(2) syscall signature.
pub fn ptrace(request: u32, tracer: u32, tracee: u32, addr: u64, data: u64) -> i64 {
    match request {
        PTRACE_ATTACH => ptrace_attach(tracer, tracee),
        PTRACE_DETACH => ptrace_detach(tracer, tracee),
        PTRACE_PEEKDATA => ptrace_peekdata(tracer, tracee, addr),
        PTRACE_POKEDATA => ptrace_pokedata(tracer, tracee, addr, data),
        PTRACE_CONT => ptrace_cont(tracer, tracee, data as u8),
        PTRACE_SINGLESTEP => ptrace_singlestep(tracer, tracee, data as u8),
        PTRACE_SYSCALL => ptrace_syscall(tracer, tracee, data as u8),
        PTRACE_SETOPTIONS => ptrace_setoptions(tracer, tracee, data as u32),
        PTRACE_KILL => ptrace_cont(tracer, tracee, 9),
        _ => PTRACE_EINVAL,
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub fn ptrace_active_count() -> u32 {
    TRACEE_COUNT.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[ptrace] process trace subsystem initialized (slots={})",
        PTRACE_MAX_TRACEES
    );
}
