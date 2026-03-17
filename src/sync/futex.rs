/// Futex — Fast Userspace Mutex for Genesis
///
/// Futexes are the foundation of all userspace synchronization (mutexes,
/// condition variables, semaphores, barriers, read-write locks).
///
/// The fast path is entirely in userspace (atomic CAS on a shared int).
/// Only when contention occurs does the kernel get involved (FUTEX_WAIT
/// puts the thread to sleep, FUTEX_WAKE wakes waiters).
///
/// Inspired by: Linux futex (kernel/futex/). All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Futex operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexOp {
    /// Wait if *addr == expected_val, sleep until woken
    Wait,
    /// Wake up to N waiters on this address
    Wake,
    /// Atomically wake N waiters, then requeue M to another address
    Requeue,
    /// Wait with timeout
    WaitTimeout,
    /// Wake one waiter and set the futex value
    WakeOp,
    /// Priority-inheritance lock
    LockPi,
    /// Priority-inheritance unlock
    UnlockPi,
}

/// A futex waiter
#[derive(Debug, Clone)]
struct FutexWaiter {
    /// Process ID waiting
    pid: u32,
    /// Thread ID (if applicable)
    tid: u32,
    /// Bitset for selective waking
    bitset: u32,
    /// Timeout (absolute nanoseconds, 0 = no timeout)
    timeout_ns: u64,
    /// Enqueue time
    enqueued_at: u64,
}

/// Per-address wait queue
struct FutexQueue {
    waiters: Vec<FutexWaiter>,
}

impl FutexQueue {
    fn new() -> Self {
        FutexQueue {
            waiters: Vec::new(),
        }
    }
}

/// Futex hash table — maps addresses to wait queues
struct FutexTable {
    /// Map from (address) -> wait queue
    queues: BTreeMap<usize, FutexQueue>,
    /// Statistics
    stats: FutexStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FutexStats {
    pub wait_count: u64,
    pub wake_count: u64,
    pub requeue_count: u64,
    pub timeout_count: u64,
    pub spurious_wakeups: u64,
}

impl FutexTable {
    const fn new() -> Self {
        FutexTable {
            queues: BTreeMap::new(),
            stats: FutexStats {
                wait_count: 0,
                wake_count: 0,
                requeue_count: 0,
                timeout_count: 0,
                spurious_wakeups: 0,
            },
        }
    }
}

static FUTEX_TABLE: Mutex<FutexTable> = Mutex::new(FutexTable::new());

/// FUTEX_WAIT — block if *addr == expected_val
///
/// Returns 0 on success (was woken), -1 on mismatch, -2 on timeout
pub fn futex_wait(addr: usize, expected_val: u32, timeout_ns: u64) -> i32 {
    let mut table = FUTEX_TABLE.lock();

    // Validate addr is non-null and naturally aligned before dereferencing
    if addr == 0 || addr % core::mem::align_of::<u32>() != 0 {
        return -1;
    }
    // Safety: addr is non-null and u32-aligned; caller must ensure it points to
    // mapped user memory valid for the lifetime of this call.
    let current_val = unsafe { core::ptr::read_volatile(addr as *const u32) };
    if current_val != expected_val {
        return -1; // value changed, don't block
    }

    let pid = crate::process::getpid();

    let waiter = FutexWaiter {
        pid,
        tid: pid, // for now, pid == tid (no threads yet)
        bitset: u32::MAX,
        timeout_ns,
        enqueued_at: crate::time::clock::uptime_ms() * 1_000_000,
    };

    let queue = table.queues.entry(addr).or_insert_with(FutexQueue::new);
    queue.waiters.push(waiter);
    table.stats.wait_count = table.stats.wait_count.saturating_add(1);

    // Block the process
    drop(table);
    block_current_process();

    0
}

/// FUTEX_WAKE — wake up to `count` waiters on addr
///
/// Returns the number of waiters actually woken
pub fn futex_wake(addr: usize, count: u32) -> u32 {
    let mut table = FUTEX_TABLE.lock();

    let queue = match table.queues.get_mut(&addr) {
        Some(q) => q,
        None => return 0,
    };

    let mut woken = 0u32;
    while woken < count && !queue.waiters.is_empty() {
        let waiter = queue.waiters.remove(0);
        wake_process(waiter.pid);
        woken = woken.saturating_add(1);
    }

    if queue.waiters.is_empty() {
        table.queues.remove(&addr);
    }

    table.stats.wake_count = table.stats.wake_count.saturating_add(woken as u64);
    woken
}

/// FUTEX_REQUEUE — wake `wake_count` waiters, move `requeue_count` to new_addr
pub fn futex_requeue(addr: usize, new_addr: usize, wake_count: u32, requeue_count: u32) -> u32 {
    let mut table = FUTEX_TABLE.lock();

    let queue = match table.queues.get_mut(&addr) {
        Some(q) => q,
        None => return 0,
    };

    let mut woken = 0u32;
    let mut requeued = 0u32;

    // Wake first batch
    while woken < wake_count && !queue.waiters.is_empty() {
        let waiter = queue.waiters.remove(0);
        wake_process(waiter.pid);
        woken = woken.saturating_add(1);
    }

    // Requeue remaining
    let mut to_requeue = Vec::new();
    while requeued < requeue_count && !queue.waiters.is_empty() {
        to_requeue.push(queue.waiters.remove(0));
        requeued = requeued.saturating_add(1);
    }

    if queue.waiters.is_empty() {
        table.queues.remove(&addr);
    }

    // Add to new queue
    if !to_requeue.is_empty() {
        let new_queue = table.queues.entry(new_addr).or_insert_with(FutexQueue::new);
        new_queue.waiters.extend(to_requeue);
    }

    table.stats.requeue_count = table.stats.requeue_count.saturating_add(requeued as u64);
    woken + requeued
}

/// FUTEX_WAIT_BITSET — like WAIT but with bitset matching
pub fn futex_wait_bitset(addr: usize, expected_val: u32, bitset: u32, timeout_ns: u64) -> i32 {
    let mut table = FUTEX_TABLE.lock();

    // Validate addr alignment and non-null before volatile read
    if addr == 0 || addr % core::mem::align_of::<u32>() != 0 {
        return -1;
    }
    // Safety: addr is non-null and u32-aligned; caller ensures mapped memory.
    let current_val = unsafe { core::ptr::read_volatile(addr as *const u32) };
    if current_val != expected_val {
        return -1;
    }

    let pid = crate::process::getpid();
    let waiter = FutexWaiter {
        pid,
        tid: pid,
        bitset,
        timeout_ns,
        enqueued_at: crate::time::clock::uptime_ms() * 1_000_000,
    };

    let queue = table.queues.entry(addr).or_insert_with(FutexQueue::new);
    queue.waiters.push(waiter);
    table.stats.wait_count = table.stats.wait_count.saturating_add(1);

    drop(table);
    block_current_process();
    0
}

/// FUTEX_WAKE_BITSET — wake waiters whose bitset overlaps
pub fn futex_wake_bitset(addr: usize, count: u32, bitset: u32) -> u32 {
    let mut table = FUTEX_TABLE.lock();

    let queue = match table.queues.get_mut(&addr) {
        Some(q) => q,
        None => return 0,
    };

    let mut woken = 0u32;
    let mut i = 0;
    while woken < count && i < queue.waiters.len() {
        if queue.waiters[i].bitset & bitset != 0 {
            let waiter = queue.waiters.remove(i);
            wake_process(waiter.pid);
            woken = woken.saturating_add(1);
        } else {
            i = i.saturating_add(1);
        }
    }

    if queue.waiters.is_empty() {
        table.queues.remove(&addr);
    }

    table.stats.wake_count = table.stats.wake_count.saturating_add(woken as u64);
    woken
}

/// Check for timed-out futex waiters (called periodically)
pub fn check_timeouts() {
    let now_ns = crate::time::clock::uptime_ms() * 1_000_000;
    let mut table = FUTEX_TABLE.lock();

    let addrs: Vec<usize> = table.queues.keys().copied().collect();
    let mut empty_addrs: Vec<usize> = Vec::new();
    let mut timeout_count = 0u64;

    for addr in &addrs {
        if let Some(queue) = table.queues.get_mut(addr) {
            let mut i = 0;
            while i < queue.waiters.len() {
                let enqueued = queue.waiters[i].enqueued_at;
                let timeout = queue.waiters[i].timeout_ns;
                if timeout > 0 && now_ns >= enqueued + timeout {
                    let waiter = queue.waiters.remove(i);
                    wake_process(waiter.pid);
                    timeout_count = timeout_count.saturating_add(1);
                } else {
                    i = i.saturating_add(1);
                }
            }
            if queue.waiters.is_empty() {
                empty_addrs.push(*addr);
            }
        }
    }
    table.stats.timeout_count = table.stats.timeout_count.saturating_add(timeout_count);
    for addr in empty_addrs {
        table.queues.remove(&addr);
    }
}

/// Block the current process (puts it in Blocked state)
fn block_current_process() {
    use crate::process::pcb::{ProcessState, PROCESS_TABLE};
    use crate::process::MAX_PROCESSES;
    let pid = crate::process::getpid();
    let idx = pid as usize;
    {
        let mut table = PROCESS_TABLE.lock();
        // Bounds-check pid before indexing the process table
        if idx < MAX_PROCESSES {
            if let Some(proc) = table[idx].as_mut() {
                proc.state = ProcessState::Blocked;
            }
        }
    }
    crate::process::scheduler::SCHEDULER.lock().remove(pid);
    crate::process::yield_now();
}

/// Wake a blocked process
fn wake_process(pid: u32) {
    use crate::process::pcb::{ProcessState, PROCESS_TABLE};
    use crate::process::MAX_PROCESSES;
    let idx = pid as usize;
    {
        let mut table = PROCESS_TABLE.lock();
        // Bounds-check pid before indexing the process table
        if idx < MAX_PROCESSES {
            if let Some(proc) = table[idx].as_mut() {
                if proc.state == ProcessState::Blocked {
                    proc.state = ProcessState::Ready;
                }
            }
        }
    }
    crate::process::scheduler::SCHEDULER.lock().add(pid);
}

/// Get futex statistics
pub fn stats() -> FutexStats {
    FUTEX_TABLE.lock().stats
}

pub fn init() {
    crate::serial_println!("  [futex] Fast userspace mutex initialized");
}
