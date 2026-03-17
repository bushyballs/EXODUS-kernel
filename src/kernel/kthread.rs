/// kthread — kernel thread management
///
/// Provides the kernel-internal thread lifecycle: create, park, unpark,
/// stop, and per-CPU kthread binding. Kernel threads never enter user space.
///
/// Design:
///   - MAX_KTHREADS = 64 static slots
///   - Each KthreadEntry tracks name, state, function pointer, CPU affinity
///   - kthread_run() allocates a slot and marks it Running
///   - kthread_stop() requests cancellation; the thread checks kthread_should_stop()
///   - kthread_park()/kthread_unpark() implement the park/unpark protocol
///   - per_cpu_thread() registers one thread per online CPU
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_KTHREADS: usize = 64;
pub const KTHREAD_ANY_CPU: u32 = u32::MAX;

// ---------------------------------------------------------------------------
// Thread state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KthreadState {
    Empty,
    Created,
    Running,
    Parked,
    Stopping,
    Stopped,
}

// ---------------------------------------------------------------------------
// Kthread entry (Copy + const fn empty — fits in static Mutex array)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct KthreadEntry {
    pub id: u32,
    pub name: [u8; 32],
    pub name_len: u8,
    pub state: KthreadState,
    pub cpu: u32, // KTHREAD_ANY_CPU = no affinity
    pub stop_req: bool,
    pub park_req: bool,
    /// Encoded function pointer: stored as usize for Copy compatibility.
    /// Caller reconstructs via `fn_ptr as fn()`.
    pub fn_ptr: usize,
    pub data: u64, // opaque argument passed to thread fn
}

impl KthreadEntry {
    pub const fn empty() -> Self {
        KthreadEntry {
            id: 0,
            name: [0u8; 32],
            name_len: 0,
            state: KthreadState::Empty,
            cpu: KTHREAD_ANY_CPU,
            stop_req: false,
            park_req: false,
            fn_ptr: 0,
            data: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static table
// ---------------------------------------------------------------------------

static KTHREADS: Mutex<[KthreadEntry; MAX_KTHREADS]> =
    Mutex::new([KthreadEntry::empty(); MAX_KTHREADS]);
static KTHREAD_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Name helper
// ---------------------------------------------------------------------------

fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let n = src.len().min(31);
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

fn name_eq(entry: &KthreadEntry, name: &[u8]) -> bool {
    if entry.name_len as usize != name.len() {
        return false;
    }
    let mut i = 0usize;
    while i < name.len() {
        if entry.name[i] != name[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a kernel thread. Returns its ID or 0 on failure (table full).
///
/// `thread_fn` is the function to call. `cpu` is the preferred CPU
/// (KTHREAD_ANY_CPU for no preference). `data` is an opaque argument.
pub fn kthread_create(name: &[u8], thread_fn: fn(u64), cpu: u32, data: u64) -> u32 {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].state == KthreadState::Empty {
            let id = KTHREAD_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i].id = id;
            table[i].name_len = copy_name(&mut table[i].name, name);
            table[i].state = KthreadState::Created;
            table[i].cpu = cpu;
            table[i].stop_req = false;
            table[i].park_req = false;
            table[i].fn_ptr = thread_fn as usize;
            table[i].data = data;
            return id;
        }
        i = i.saturating_add(1);
    }
    0 // table full
}

/// Create and immediately mark as Running.
pub fn kthread_run(name: &[u8], thread_fn: fn(u64), cpu: u32, data: u64) -> u32 {
    let id = kthread_create(name, thread_fn, cpu, data);
    if id != 0 {
        kthread_wake(id);
    }
    id
}

/// Transition Created → Running (wake a just-created thread).
pub fn kthread_wake(id: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id && table[i].state == KthreadState::Created {
            table[i].state = KthreadState::Running;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Request a thread to stop. Thread must call kthread_should_stop() and exit.
pub fn kthread_stop(id: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            match table[i].state {
                KthreadState::Running | KthreadState::Parked | KthreadState::Created => {
                    table[i].stop_req = true;
                    table[i].state = KthreadState::Stopping;
                    return true;
                }
                _ => return false,
            }
        }
        i = i.saturating_add(1);
    }
    false
}

/// Called by the thread to check if it should exit.
pub fn kthread_should_stop(id: u32) -> bool {
    let table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            return table[i].stop_req;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Request park (suspend). Thread checks kthread_should_park() and calls kthread_parkme().
pub fn kthread_park(id: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id && table[i].state == KthreadState::Running {
            table[i].park_req = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Called by the thread to acknowledge park request: Running → Parked.
pub fn kthread_parkme(id: u32) {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id && table[i].park_req {
            table[i].state = KthreadState::Parked;
            table[i].park_req = false;
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Wake a parked thread: Parked → Running.
pub fn kthread_unpark(id: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id && table[i].state == KthreadState::Parked {
            table[i].state = KthreadState::Running;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Mark thread as stopped (called by thread fn before it returns).
pub fn kthread_exit(id: u32) {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            table[i].state = KthreadState::Stopped;
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Reap a stopped thread slot (free for reuse).
pub fn kthread_reap(id: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id && table[i].state == KthreadState::Stopped {
            table[i] = KthreadEntry::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Check if a thread should park (called inside thread loop).
pub fn kthread_should_park(id: u32) -> bool {
    let table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            return table[i].park_req;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Get thread state.
pub fn kthread_state(id: u32) -> KthreadState {
    let table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            return table[i].state;
        }
        i = i.saturating_add(1);
    }
    KthreadState::Empty
}

/// Find thread ID by name.
pub fn kthread_find(name: &[u8]) -> u32 {
    let table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].state != KthreadState::Empty && name_eq(&table[i], name) {
            return table[i].id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Bind a thread to a specific CPU.
pub fn kthread_bind(id: u32, cpu: u32) -> bool {
    let mut table = KTHREADS.lock();
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        if table[i].id == id {
            table[i].cpu = cpu;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Count live threads (Created + Running + Parked + Stopping).
pub fn kthread_count() -> usize {
    let table = KTHREADS.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < MAX_KTHREADS {
        match table[i].state {
            KthreadState::Empty | KthreadState::Stopped => {}
            _ => n = n.saturating_add(1),
        }
        i = i.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Per-CPU thread helpers
// ---------------------------------------------------------------------------

/// Spawn one named thread per CPU in 0..ncpus, returning the count created.
/// Thread function receives the CPU index as `data`.
pub fn kthread_per_cpu(base_name: &[u8], thread_fn: fn(u64), ncpus: u32) -> u32 {
    let mut created = 0u32;
    let mut cpu = 0u32;
    while cpu < ncpus {
        // Build name: base_name/N (truncated to 31 chars)
        let mut name = [0u8; 32];
        let bn = base_name.len().min(28);
        let mut k = 0usize;
        while k < bn {
            name[k] = base_name[k];
            k = k.saturating_add(1);
        }
        name[k] = b'/';
        k = k.saturating_add(1);
        // append cpu digit(s)
        if cpu < 10 {
            name[k] = b'0' + cpu as u8;
        } else {
            name[k] = b'0' + (cpu / 10) as u8;
            name[k + 1] = b'0' + (cpu % 10) as u8;
        }
        let nlen = if k < 31 { k + 1 } else { 31 };
        if kthread_run(&name[..nlen], thread_fn, cpu, cpu as u64) != 0 {
            created = created.saturating_add(1);
        }
        cpu = cpu.saturating_add(1);
    }
    created
}

// ---------------------------------------------------------------------------
// Standard kernel threads spawned at boot
// ---------------------------------------------------------------------------

fn kswapd(_data: u64) {
    // Memory reclaim daemon — would loop checking memory pressure
}

fn ksoftirqd(_data: u64) {
    // Soft-IRQ daemon — would drain softirq queues
}

fn kworker(_data: u64) {
    // Generic workqueue worker
}

fn khugepaged(_data: u64) {
    // Huge-page collapse daemon
}

fn kcompactd(_data: u64) {
    // Memory compaction daemon
}

pub fn init() {
    kthread_run(b"kswapd0", kswapd, 0, 0);
    kthread_run(b"ksoftirqd/0", ksoftirqd, 0, 0);
    kthread_run(b"kworker/0:0", kworker, 0, 0);
    kthread_run(b"khugepaged", khugepaged, KTHREAD_ANY_CPU, 0);
    kthread_run(b"kcompactd0", kcompactd, 0, 0);
    serial_println!(
        "[kthread] kernel thread subsystem initialized ({} threads)",
        kthread_count()
    );
}
