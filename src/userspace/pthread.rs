use crate::sync::Mutex;
/// POSIX threads (pthreads) for Genesis
///
/// Implements the core POSIX threading API:
///   - pthread_create / pthread_join / pthread_exit
///   - pthread_mutex_init / lock / unlock / destroy
///   - pthread_cond_init / wait / signal / broadcast
///   - Thread-local storage (TLS)
///
/// Threads are implemented as lightweight processes sharing the same
/// address space (page table).
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Thread identifier (maps to kernel PID)
pub type PthreadT = u32;

/// Mutex identifier
pub type PthreadMutexT = u32;

/// Condition variable identifier
pub type PthreadCondT = u32;

/// Thread state
#[derive(Debug, Clone)]
pub struct ThreadInfo {
    pub tid: PthreadT,
    pub pid: u32,          // Kernel PID
    pub joined: bool,      // Has someone called pthread_join?
    pub exit_value: usize, // Return value from thread function
    pub detached: bool,    // Detached threads are cleaned up automatically
}

/// Mutex state
#[derive(Debug, Clone)]
pub struct MutexInfo {
    pub id: PthreadMutexT,
    pub owner: Option<PthreadT>, // Thread that holds the lock
    pub locked: bool,
    pub waiters: Vec<PthreadT>, // Threads waiting to acquire
    pub recursive: bool,        // PTHREAD_MUTEX_RECURSIVE
    pub lock_count: u32,
}

/// Condition variable state
#[derive(Debug, Clone)]
pub struct CondInfo {
    pub id: PthreadCondT,
    pub waiters: Vec<PthreadT>,
}

/// Global thread registry
static THREADS: Mutex<BTreeMap<PthreadT, ThreadInfo>> = Mutex::new(BTreeMap::new());

/// Global mutex registry
static MUTEXES: Mutex<BTreeMap<PthreadMutexT, MutexInfo>> = Mutex::new(BTreeMap::new());

/// Global condition variable registry
static CONDS: Mutex<BTreeMap<PthreadCondT, CondInfo>> = Mutex::new(BTreeMap::new());

/// Next IDs
static NEXT_MUTEX_ID: Mutex<PthreadMutexT> = Mutex::new(1);
static NEXT_COND_ID: Mutex<PthreadCondT> = Mutex::new(1);

/// Thread-local storage keys and values
static TLS: Mutex<BTreeMap<PthreadT, BTreeMap<u32, usize>>> = Mutex::new(BTreeMap::new());
static NEXT_TLS_KEY: Mutex<u32> = Mutex::new(1);

/// Create a new thread
pub fn pthread_create(entry: fn()) -> Result<PthreadT, i32> {
    // Spawn a new kernel process (shares address space)
    let pid = crate::process::spawn_kernel("pthread", entry).ok_or(-1i32)?;

    let tid = pid; // TID = PID for simplicity

    let info = ThreadInfo {
        tid,
        pid,
        joined: false,
        exit_value: 0,
        detached: false,
    };

    THREADS.lock().insert(tid, info);
    Ok(tid)
}

/// Wait for a thread to terminate
pub fn pthread_join(tid: PthreadT) -> Result<usize, i32> {
    // Poll until thread is dead
    loop {
        {
            let table = crate::process::pcb::PROCESS_TABLE.lock();
            if let Some(proc) = table[tid as usize].as_ref() {
                if proc.state == crate::process::pcb::ProcessState::Dead {
                    let exit_val = {
                        let threads = THREADS.lock();
                        threads.get(&tid).map(|t| t.exit_value).unwrap_or(0)
                    };

                    // Clean up
                    THREADS.lock().remove(&tid);
                    return Ok(exit_val);
                }
            } else {
                // Thread already cleaned up
                let exit_val = {
                    let threads = THREADS.lock();
                    threads.get(&tid).map(|t| t.exit_value).unwrap_or(0)
                };
                THREADS.lock().remove(&tid);
                return Ok(exit_val);
            }
        }
        crate::process::yield_now();
    }
}

/// Exit the current thread
pub fn pthread_exit(value: usize) {
    let pid = crate::process::getpid();
    {
        let mut threads = THREADS.lock();
        if let Some(info) = threads.get_mut(&pid) {
            info.exit_value = value;
        }
    }
    crate::process::exit(0);
}

/// Detach a thread (no join needed)
pub fn pthread_detach(tid: PthreadT) -> Result<(), i32> {
    let mut threads = THREADS.lock();
    if let Some(info) = threads.get_mut(&tid) {
        info.detached = true;
        Ok(())
    } else {
        Err(-1)
    }
}

/// Get current thread ID
pub fn pthread_self() -> PthreadT {
    crate::process::getpid()
}

// === Mutexes ===

/// Initialize a mutex
pub fn pthread_mutex_init(recursive: bool) -> PthreadMutexT {
    let mut next = NEXT_MUTEX_ID.lock();
    let id = *next;
    *next = next.saturating_add(1);

    let info = MutexInfo {
        id,
        owner: None,
        locked: false,
        waiters: Vec::new(),
        recursive,
        lock_count: 0,
    };

    MUTEXES.lock().insert(id, info);
    id
}

/// Lock a mutex
pub fn pthread_mutex_lock(mutex: PthreadMutexT) -> Result<(), i32> {
    let tid = pthread_self();

    loop {
        let mut mutexes = MUTEXES.lock();
        if let Some(m) = mutexes.get_mut(&mutex) {
            if !m.locked {
                m.locked = true;
                m.owner = Some(tid);
                m.lock_count = 1;
                return Ok(());
            } else if m.recursive && m.owner == Some(tid) {
                m.lock_count = m.lock_count.saturating_add(1);
                return Ok(());
            } else {
                // Already locked by another thread — add to waiters
                if !m.waiters.contains(&tid) {
                    m.waiters.push(tid);
                }
                drop(mutexes);
                crate::process::yield_now();
            }
        } else {
            return Err(-1);
        }
    }
}

/// Unlock a mutex
pub fn pthread_mutex_unlock(mutex: PthreadMutexT) -> Result<(), i32> {
    let mut mutexes = MUTEXES.lock();
    if let Some(m) = mutexes.get_mut(&mutex) {
        if m.recursive && m.lock_count > 1 {
            m.lock_count -= 1;
            return Ok(());
        }

        m.locked = false;
        m.owner = None;
        m.lock_count = 0;

        // Wake first waiter
        if !m.waiters.is_empty() {
            let _waker = m.waiters.remove(0);
            // Waker will acquire on next iteration of its lock loop
        }
        Ok(())
    } else {
        Err(-1)
    }
}

/// Destroy a mutex
pub fn pthread_mutex_destroy(mutex: PthreadMutexT) -> Result<(), i32> {
    MUTEXES.lock().remove(&mutex);
    Ok(())
}

// === Condition Variables ===

/// Initialize a condition variable
pub fn pthread_cond_init() -> PthreadCondT {
    let mut next = NEXT_COND_ID.lock();
    let id = *next;
    *next = next.saturating_add(1);

    CONDS.lock().insert(
        id,
        CondInfo {
            id,
            waiters: Vec::new(),
        },
    );
    id
}

/// Wait on a condition variable (atomically releases mutex and waits)
pub fn pthread_cond_wait(cond: PthreadCondT, mutex: PthreadMutexT) -> Result<(), i32> {
    let tid = pthread_self();

    // Add to waiters
    {
        let mut conds = CONDS.lock();
        if let Some(c) = conds.get_mut(&cond) {
            c.waiters.push(tid);
        } else {
            return Err(-1);
        }
    }

    // Release mutex
    let _ = pthread_mutex_unlock(mutex);

    // Wait until removed from waiters list (signaled)
    loop {
        {
            let conds = CONDS.lock();
            if let Some(c) = conds.get(&cond) {
                if !c.waiters.contains(&tid) {
                    // We were signaled — reacquire mutex and return
                    drop(conds);
                    let _ = pthread_mutex_lock(mutex);
                    return Ok(());
                }
            }
        }
        crate::process::yield_now();
    }
}

/// Signal one waiter on a condition variable
pub fn pthread_cond_signal(cond: PthreadCondT) -> Result<(), i32> {
    let mut conds = CONDS.lock();
    if let Some(c) = conds.get_mut(&cond) {
        if !c.waiters.is_empty() {
            c.waiters.remove(0);
        }
        Ok(())
    } else {
        Err(-1)
    }
}

/// Wake all waiters on a condition variable
pub fn pthread_cond_broadcast(cond: PthreadCondT) -> Result<(), i32> {
    let mut conds = CONDS.lock();
    if let Some(c) = conds.get_mut(&cond) {
        c.waiters.clear();
        Ok(())
    } else {
        Err(-1)
    }
}

/// Destroy a condition variable
pub fn pthread_cond_destroy(cond: PthreadCondT) -> Result<(), i32> {
    CONDS.lock().remove(&cond);
    Ok(())
}

// === Thread-Local Storage ===

/// Create a new TLS key
pub fn pthread_key_create() -> u32 {
    let mut next = NEXT_TLS_KEY.lock();
    let key = *next;
    *next = next.saturating_add(1);
    key
}

/// Set TLS value for current thread
pub fn pthread_setspecific(key: u32, value: usize) {
    let tid = pthread_self();
    let mut tls = TLS.lock();
    tls.entry(tid)
        .or_insert_with(BTreeMap::new)
        .insert(key, value);
}

/// Get TLS value for current thread
pub fn pthread_getspecific(key: u32) -> usize {
    let tid = pthread_self();
    let tls = TLS.lock();
    tls.get(&tid)
        .and_then(|m| m.get(&key))
        .copied()
        .unwrap_or(0)
}

/// Initialize pthreads subsystem
pub fn init() {
    serial_println!("  pthreads: POSIX threads ready (create, join, mutex, cond, TLS)");
}
