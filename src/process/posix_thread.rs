use crate::sync::Mutex;
/// POSIX thread (pthread) semantics — user-space threading support.
///
/// Part of the AIOS kernel.
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// TLS key allocator
// ---------------------------------------------------------------------------

/// Monotonically incrementing TLS key counter.
///
/// `pthread_key_create()` increments this and returns the new key value.
/// Key 0 is reserved as "invalid / not set".
static TLS_KEY_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Thread-local storage key.
pub type PthreadKey = u32;

/// Allocate a fresh TLS key (analogous to `pthread_key_create`).
///
/// Returns the new key on success, or `Err` if the key space is exhausted
/// (counter has wrapped past `u32::MAX`).
pub fn pthread_key_create() -> Result<PthreadKey, &'static str> {
    // Relaxed ordering is fine: uniqueness is guaranteed by the atomic
    // fetch_add, which is sequentially consistent on all supported targets.
    let key = TLS_KEY_COUNTER.fetch_add(1, Ordering::Relaxed);
    if key == 0 {
        // Wrapped — almost impossible in practice but handle defensively.
        return Err("pthread_key_create: TLS key space exhausted");
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// Per-thread TLS storage table
// ---------------------------------------------------------------------------

/// Maximum number of TLS (key, value) pairs stored per thread.
const MAX_TLS_PAIRS: usize = 128;

/// Maximum number of threads in the TLS table (one entry per TID).
const MAX_THREADS: usize = 1024;

/// Per-thread TLS storage: a fixed-size array of (key, value) pairs plus
/// the TGID for CLONE_THREAD support.
struct TlsEntry {
    /// Owner thread ID (0 = slot unused).
    tid: u32,
    /// Thread Group ID — same as the group-leader PID for all threads in
    /// the same pthread group.  Equals `tid` for group leaders.
    tgid: u32,
    pairs: [(PthreadKey, usize); MAX_TLS_PAIRS],
    len: usize,
}

impl TlsEntry {
    const fn empty() -> Self {
        TlsEntry {
            tid: 0,
            tgid: 0,
            pairs: [(0, 0); MAX_TLS_PAIRS],
            len: 0,
        }
    }
}

static TLS_TABLE: Mutex<[TlsEntry; MAX_THREADS]> =
    Mutex::new([const { TlsEntry::empty() }; MAX_THREADS]);

/// Store a TLS value for the given (thread, key) pair
/// (`pthread_setspecific(key, val)`).
pub fn pthread_setspecific(tid: u32, key: PthreadKey, val: usize) -> Result<(), &'static str> {
    if (tid as usize) >= MAX_THREADS {
        return Err("pthread_setspecific: TID out of bounds");
    }
    let mut table = TLS_TABLE.lock();
    let entry = &mut table[tid as usize];
    entry.tid = tid;
    // Update an existing slot if the key is already present.
    for i in 0..entry.len {
        if entry.pairs[i].0 == key {
            entry.pairs[i].1 = val;
            return Ok(());
        }
    }
    // Append a new (key, val) pair.
    if entry.len >= MAX_TLS_PAIRS {
        return Err("pthread_setspecific: per-thread TLS storage full");
    }
    entry.pairs[entry.len] = (key, val);
    entry.len += 1;
    Ok(())
}

/// Retrieve a TLS value for the given (thread, key) pair
/// (`pthread_getspecific(key)`).  Returns `None` if the key has not been set.
pub fn pthread_getspecific(tid: u32, key: PthreadKey) -> Option<usize> {
    if (tid as usize) >= MAX_THREADS {
        return None;
    }
    let table = TLS_TABLE.lock();
    let entry = &table[tid as usize];
    for i in 0..entry.len {
        if entry.pairs[i].0 == key {
            return Some(entry.pairs[i].1);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Pthread attributes
// ---------------------------------------------------------------------------

/// Pthread attributes for thread creation.
pub struct PthreadAttr {
    pub stack_size: usize,
    pub detached: bool,
    pub priority: i8,
}

/// Default user-space stack size: 8 MiB.
const PTHREAD_DEFAULT_STACK_SIZE: usize = 8 * 1024 * 1024;

impl PthreadAttr {
    pub fn default() -> Self {
        PthreadAttr {
            stack_size: PTHREAD_DEFAULT_STACK_SIZE,
            detached: false,
            priority: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Kernel-side POSIX thread descriptor
// ---------------------------------------------------------------------------

/// Kernel-side state for a POSIX thread.
pub struct PosixThread {
    pub tid: u32,
    /// Thread Group ID (TGID): all threads in the same pthread group share
    /// this value; it equals the PID of the group leader.
    pub tgid: u32,
    pub join_waiters: Vec<u32>,
    pub detached: bool,
}

impl PosixThread {
    pub fn new(tid: u32) -> Self {
        PosixThread {
            tid,
            tgid: tid, // default: each thread is its own group leader
            join_waiters: Vec::new(),
            detached: false,
        }
    }
}

// ---------------------------------------------------------------------------
// pthread_create
// ---------------------------------------------------------------------------

/// Create a new POSIX thread in the calling process.
///
/// Implements the full `clone(CLONE_VM | CLONE_SIGHAND | CLONE_THREAD)` path:
///
/// 1. **CLONE_VM**: `do_fork(CloneFlags::thread())` copies the parent's
///    `page_table` pointer (CR3 value) into the child unchanged so both
///    threads share the same virtual address space.
///
/// 2. **CLONE_SIGHAND**: `CloneFlags::thread()` sets `share_sighand = true`,
///    so `do_fork` does not reset signal handlers — the child inherits the
///    parent's signal disposition table.
///
/// 3. **CLONE_THREAD**: places the child in the same thread group as the
///    parent by writing `child.tgid = parent_tgid` in the TLS table.
///
/// 4. **TLS key**: allocates a fresh TLS key via `pthread_key_create()` and
///    records the thread entry-point address in per-thread storage.
pub fn pthread_create(attr: &PthreadAttr, entry: usize, arg: usize) -> Result<u32, &'static str> {
    use crate::process::fork::{do_fork, CloneFlags};
    use crate::process::pcb::PROCESS_TABLE;
    use crate::process::scheduler::SCHEDULER;

    let parent_pid = SCHEDULER.lock().current();

    crate::serial_println!(
        "pthread_create: parent={} entry=0x{:x} stack_size={} detached={}",
        parent_pid,
        entry,
        attr.stack_size,
        attr.detached
    );

    // Step 1 & 2: fork with CLONE_VM | CLONE_SIGHAND | CLONE_FILES | CLONE_FS.
    let child_tid = do_fork(CloneFlags::thread())?;

    // In the parent do_fork returns child_tid > 0; in the child PCB rax = 0.
    // Overwrite the child's instruction pointer and first argument register so
    // the context-switch jumps to `entry(arg)` when the child is first scheduled.
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(child) = table[child_tid as usize].as_mut() {
            // rip: thread entry function.
            child.context.rip = entry as u64;
            // rdi: first argument (arg) per the SysV AMD64 calling convention.
            child.context.rdi = arg as u64;
        }
    }

    // Step 3: CLONE_THREAD — inherit the parent's TGID.
    let parent_tgid: u32 = {
        if (parent_pid as usize) < MAX_THREADS {
            let table = TLS_TABLE.lock();
            let e = &table[parent_pid as usize];
            if e.tid == parent_pid {
                e.tgid
            } else {
                parent_pid
            }
        } else {
            parent_pid
        }
    };

    // Initialise the child's TLS table entry with inherited TGID.
    if (child_tid as usize) < MAX_THREADS {
        let mut table = TLS_TABLE.lock();
        let e = &mut table[child_tid as usize];
        e.tid = child_tid;
        e.tgid = parent_tgid;
        e.len = 0;
    }

    // Step 4: allocate a TLS key for the thread start-function address.
    let start_key = pthread_key_create()?;
    let _ = pthread_setspecific(child_tid, start_key, entry);

    crate::serial_println!(
        "pthread_create: created TID {} (TGID={}) start_key={}",
        child_tid,
        parent_tgid,
        start_key
    );

    Ok(child_tid)
}

// ---------------------------------------------------------------------------
// Module initialiser
// ---------------------------------------------------------------------------

/// Initialize the POSIX threading subsystem.
///
/// Seeds the TLS key counter (already done via the static initialiser) and
/// logs readiness.
pub fn init() {
    // TLS_KEY_COUNTER starts at 1 — no additional setup needed.
    crate::serial_println!(
        "  posix_thread: TLS key allocator ready (next_key={})",
        TLS_KEY_COUNTER.load(Ordering::Relaxed)
    );
}
