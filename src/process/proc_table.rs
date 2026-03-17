use crate::sync::Mutex;
use alloc::vec::Vec;
/// Process Table — lightweight flat-array process registry
///
/// Provides a fixed-size table of ProcessEntry records indexed by PID,
/// a monotonically increasing PID counter, and all the operations the
/// scheduler needs: add, get, set_state, remove, list_runnable.
///
/// This is intentionally a thin layer on top of a simple array so that
/// the scheduler can operate without touching the heavy PCB (pcb.rs).
/// The canonical PCB lives in pcb::PROCESS_TABLE; this module keeps a
/// parallel lightweight view used by the CFS scheduler and context switch.
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concurrently live processes.
/// Must not exceed pcb::MAX_PROCESSES (256).
pub const MAX_PROCESSES: usize = 1024;

// ---------------------------------------------------------------------------
// PID allocator
// ---------------------------------------------------------------------------

/// Monotonically increasing PID counter.
/// PID 0 = idle task, PID 1 = init.  New processes start at 2.
static PID_COUNTER: AtomicU32 = AtomicU32::new(2);

/// Allocate the next available PID.
/// Uses relaxed ordering because PIDs are never used as synchronisation
/// tokens; uniqueness within a single boot is all that is required.
pub fn alloc_pid() -> u32 {
    PID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Peek at the next PID that would be allocated without consuming it.
pub fn peek_next_pid() -> u32 {
    PID_COUNTER.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Process states (lightweight copy, mirrors pcb::ProcessState)
// ---------------------------------------------------------------------------

/// Lightweight process state used by the process table and scheduler.
/// Does NOT need to be kept in sync with pcb::ProcessState byte-for-byte,
/// but the names are intentionally identical for readability.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ProcessState {
    /// Currently executing on a CPU.
    #[default]
    Running,
    /// Ready to be scheduled; sitting in the run queue.
    Runnable,
    /// Blocked waiting for an event (I/O, sleep, mutex, …).
    Sleeping,
    /// Terminated, waiting for parent to collect exit status.
    Zombie,
    /// Stopped by signal (SIGTSTP / SIGSTOP).
    Stopped,
}

// ---------------------------------------------------------------------------
// ProcessEntry
// ---------------------------------------------------------------------------

/// One row in the process table — all scheduler-relevant fields for a process.
///
/// All fields are `Copy` types so that `Option<ProcessEntry>` is also `Copy`,
/// enabling the `[None; MAX_PROCESSES]` array initialiser in a `const fn`.
///
/// Aligned to a 64-byte cache line to prevent false sharing on SMP: the
/// scheduler reads/writes this struct from the timer IRQ on every tick
/// (~100/s per CPU), so fitting it in one cache line eliminates coherency
/// traffic between CPUs sharing the process table.
// hot struct: read/written by scheduler on every context switch (~1K/s)
#[repr(C, align(64))]
#[derive(Clone, Copy, Debug)]
pub struct ProcessEntry {
    /// Process identifier.
    pub pid: u32,
    /// Parent process identifier (0 for the idle task).
    pub ppid: u32,
    /// Current lifecycle state.
    pub state: ProcessState,
    /// Nice value in the range [-20, +19].  Lower = higher priority.
    pub nice: i8,
    // 3 bytes implicit padding before vruntime (alignment)
    _pad0: [u8; 3],
    /// CFS virtual runtime (weighted nanoseconds).
    /// The process with the smallest vruntime is scheduled next.
    pub vruntime: u64,
    /// CFS weight derived from the nice value via nice_to_weight().
    pub weight: u32,
    // 4 bytes padding before saved_rsp (alignment)
    _pad1: [u8; 4],
    /// Kernel stack pointer saved by the context switch.
    /// The `switch_context` naked function reads/writes this field.
    pub saved_rsp: u64,
    /// Top of this process's kernel stack (used to initialise saved_rsp).
    pub kernel_stack_top: u64,
    /// Exit code (meaningful only when state == Zombie).
    pub exit_code: i32,
    /// Padding to fill remainder of 64-byte cache line.
    /// sizeof fields: 4+4+1(enum)+1+3+8+4+4+8+8+4 = 49 bytes → pad 15
    _pad2: [u8; 15],
}

impl ProcessEntry {
    /// Construct a new entry for a process that has not yet been scheduled.
    pub fn new(pid: u32, ppid: u32, kernel_stack_top: u64) -> Self {
        ProcessEntry {
            pid,
            ppid,
            state: ProcessState::Runnable,
            nice: 0,
            _pad0: [0u8; 3],
            vruntime: 0,
            weight: crate::process::nice::nice_to_weight(0), // weight for nice 0 = 1024
            _pad1: [0u8; 4],
            saved_rsp: kernel_stack_top,
            kernel_stack_top,
            exit_code: 0,
            _pad2: [0u8; 15],
        }
    }
}

// ---------------------------------------------------------------------------
// Global process table
// ---------------------------------------------------------------------------

/// The global process table.
///
/// Indexed by PID directly (table[pid]).  Entries outside the occupied
/// range and freed entries are None.
///
/// MAX_PROCESSES is 1024, so this costs 1024 * sizeof(Option<ProcessEntry>)
/// on the heap (the Mutex itself lives in the BSS).  Each ProcessEntry is
/// small (≈ 64 bytes), so the table is ≈ 64 KiB — perfectly fine.
pub static PROCESS_TABLE: Mutex<ProcessTableInner> = Mutex::new(ProcessTableInner::new());

/// Inner structure wrapped by the Mutex so the lock guard gives a direct
/// reference to the array without a second pointer indirection.
pub struct ProcessTableInner {
    /// Slots indexed by PID.  None = free.
    pub slots: [Option<ProcessEntry>; MAX_PROCESSES],
    /// Number of non-None entries currently live.
    pub count: usize,
}

impl ProcessTableInner {
    pub const fn new() -> Self {
        // ProcessEntry derives Copy, so Option<ProcessEntry> is also Copy,
        // allowing the [NONE; MAX_PROCESSES] initialiser inside a const fn.
        const NONE: Option<ProcessEntry> = None;
        ProcessTableInner {
            slots: [NONE; MAX_PROCESSES],
            count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Insert a new process entry into the table.
///
/// Returns `Ok(())` on success, or `Err` if the table is full or
/// the PID slot is already occupied.
pub fn add_process(entry: ProcessEntry) -> Result<(), &'static str> {
    let pid = entry.pid as usize;
    if pid >= MAX_PROCESSES {
        return Err("pid exceeds MAX_PROCESSES");
    }
    let mut tbl = PROCESS_TABLE.lock();
    if tbl.count >= MAX_PROCESSES {
        return Err("process table full");
    }
    if tbl.slots[pid].is_some() {
        return Err("pid already in use");
    }
    tbl.slots[pid] = Some(entry);
    tbl.count = tbl.count.saturating_add(1);
    Ok(())
}

/// Retrieve a clone of a process entry by PID.
/// Returns None if the PID is not in the table.
pub fn get_process(pid: u32) -> Option<ProcessEntry> {
    if (pid as usize) >= MAX_PROCESSES {
        return None;
    }
    PROCESS_TABLE.lock().slots[pid as usize].clone()
}

/// Update the state of an existing process.
/// No-op if the PID is not in the table.
pub fn set_state(pid: u32, state: ProcessState) {
    if (pid as usize) >= MAX_PROCESSES {
        return;
    }
    let mut tbl = PROCESS_TABLE.lock();
    if let Some(ref mut entry) = tbl.slots[pid as usize] {
        entry.state = state;
    }
}

/// Update the saved RSP of a process (called after context switch).
pub fn set_saved_rsp(pid: u32, rsp: u64) {
    if (pid as usize) >= MAX_PROCESSES {
        return;
    }
    let mut tbl = PROCESS_TABLE.lock();
    if let Some(ref mut entry) = tbl.slots[pid as usize] {
        entry.saved_rsp = rsp;
    }
}

/// Update the vruntime of a process.
pub fn set_vruntime(pid: u32, vruntime: u64) {
    if (pid as usize) >= MAX_PROCESSES {
        return;
    }
    let mut tbl = PROCESS_TABLE.lock();
    if let Some(ref mut entry) = tbl.slots[pid as usize] {
        entry.vruntime = vruntime;
    }
}

/// Set exit code and mark a process as Zombie.
pub fn set_zombie(pid: u32, exit_code: i32) {
    if (pid as usize) >= MAX_PROCESSES {
        return;
    }
    let mut tbl = PROCESS_TABLE.lock();
    if let Some(ref mut entry) = tbl.slots[pid as usize] {
        entry.state = ProcessState::Zombie;
        entry.exit_code = exit_code;
    }
}

/// Remove a process from the table and return its entry.
/// Returns None if the PID was not present.
pub fn remove_process(pid: u32) -> Option<ProcessEntry> {
    if (pid as usize) >= MAX_PROCESSES {
        return None;
    }
    let mut tbl = PROCESS_TABLE.lock();
    let entry = tbl.slots[pid as usize].take();
    if entry.is_some() {
        tbl.count = tbl.count.saturating_sub(1);
    }
    entry
}

/// Collect the PIDs of all processes currently in the Runnable state.
pub fn list_runnable() -> Vec<u32> {
    let tbl = PROCESS_TABLE.lock();
    let mut result = Vec::new();
    for slot in tbl.slots.iter() {
        if let Some(ref e) = slot {
            if e.state == ProcessState::Runnable {
                result.push(e.pid);
            }
        }
    }
    result
}

/// Collect the PIDs of all processes that are alive (not Zombie).
pub fn list_alive() -> Vec<u32> {
    let tbl = PROCESS_TABLE.lock();
    let mut result = Vec::new();
    for slot in tbl.slots.iter() {
        if let Some(ref e) = slot {
            if e.state != ProcessState::Zombie {
                result.push(e.pid);
            }
        }
    }
    result
}

/// Return the number of live processes.
pub fn process_count() -> usize {
    PROCESS_TABLE.lock().count
}
