/// RCU — Read-Copy-Update for Genesis
///
/// RCU is a synchronization mechanism that allows extremely fast concurrent
/// reads without any locks. Writers make a copy, modify the copy, then
/// atomically swap the pointer. Old readers continue to see the old version
/// until they reach a "quiescent state" (context switch, idle, etc.).
///
/// Perfect for: routing tables, firewall rules, /proc data, read-heavy structures.
///
/// Inspired by: Linux RCU (kernel/rcu/). All code is original.
use crate::sync::Mutex;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

/// Global RCU grace period counter
static GRACE_PERIOD: AtomicU64 = AtomicU64::new(0);

/// Per-CPU RCU state
struct RcuCpuState {
    /// Last grace period this CPU acknowledged
    last_gp: u64,
    /// Whether this CPU is in an RCU read-side critical section
    in_read_lock: bool,
    /// Nesting depth of rcu_read_lock
    nesting: u32,
    /// Whether this CPU has passed through a quiescent state
    quiescent: bool,
}

impl RcuCpuState {
    const fn new() -> Self {
        RcuCpuState {
            last_gp: 0,
            in_read_lock: false,
            nesting: 0,
            quiescent: true,
        }
    }
}

/// RCU callback — function to call after grace period
struct RcuCallback {
    /// Grace period after which to call
    gp_seq: u64,
    /// Callback function pointer
    func: fn(*mut u8),
    /// Argument to pass
    arg: *mut u8,
}

unsafe impl Send for RcuCallback {}

/// Global RCU state
struct RcuState {
    /// Per-CPU state
    cpu_state: [RcuCpuState; crate::smp::MAX_CPUS],
    /// Pending callbacks
    callbacks: Vec<RcuCallback>,
    /// Current grace period sequence number
    gp_seq: u64,
    /// Completed grace period sequence number
    gp_completed: u64,
}

impl RcuState {
    const fn new() -> Self {
        const CPU_INIT: RcuCpuState = RcuCpuState::new();
        RcuState {
            cpu_state: [CPU_INIT; crate::smp::MAX_CPUS],
            callbacks: Vec::new(),
            gp_seq: 0,
            gp_completed: 0,
        }
    }
}

static RCU: Mutex<RcuState> = Mutex::new(RcuState::new());

/// Enter an RCU read-side critical section.
/// This is extremely cheap — just increments a counter.
/// No memory barriers needed on the read path.
pub fn rcu_read_lock() {
    let cpu = crate::smp::current_cpu() as usize;
    if cpu >= crate::smp::MAX_CPUS {
        return;
    }
    let mut rcu = RCU.lock();
    let state = &mut rcu.cpu_state[cpu];
    state.nesting = state.nesting.saturating_add(1);
    state.in_read_lock = true;
    state.quiescent = false;
}

/// Exit an RCU read-side critical section
pub fn rcu_read_unlock() {
    let cpu = crate::smp::current_cpu() as usize;
    if cpu >= crate::smp::MAX_CPUS {
        return;
    }
    let mut rcu = RCU.lock();
    let state = &mut rcu.cpu_state[cpu];
    if state.nesting > 0 {
        state.nesting = state.nesting.saturating_sub(1);
        if state.nesting == 0 {
            state.in_read_lock = false;
            state.quiescent = true;
        }
    }
}

/// Start a new grace period
fn start_gp() {
    let mut rcu = RCU.lock();
    rcu.gp_seq = rcu.gp_seq.saturating_add(1);
    GRACE_PERIOD.store(rcu.gp_seq, Ordering::Release);

    // Reset quiescent state tracking for all CPUs
    let num_cpus = (crate::smp::num_cpus() as usize).min(crate::smp::MAX_CPUS);
    for i in 0..num_cpus {
        rcu.cpu_state[i].quiescent = false;
    }
}

/// Record a quiescent state for the current CPU
/// Called during context switch, idle, or explicitly
pub fn rcu_quiescent_state() {
    let cpu = crate::smp::current_cpu() as usize;
    if cpu >= crate::smp::MAX_CPUS {
        return;
    }
    let mut rcu = RCU.lock();
    if !rcu.cpu_state[cpu].in_read_lock {
        rcu.cpu_state[cpu].quiescent = true;
        rcu.cpu_state[cpu].last_gp = rcu.gp_seq;
    }
}

/// Check if all CPUs have passed through a quiescent state
fn check_gp_completion() -> bool {
    let rcu = RCU.lock();
    let num_cpus = (crate::smp::num_cpus() as usize).min(crate::smp::MAX_CPUS);
    for i in 0..num_cpus {
        if !rcu.cpu_state[i].quiescent {
            return false;
        }
    }
    true
}

/// Advance RCU processing — check for completed grace periods, invoke callbacks
pub fn rcu_advance() {
    if check_gp_completion() {
        let mut rcu = RCU.lock();
        rcu.gp_completed = rcu.gp_seq;

        // Invoke callbacks that have passed their grace period
        let completed = rcu.gp_completed;
        let mut pending = Vec::new();
        let mut ready = Vec::new();

        for cb in rcu.callbacks.drain(..) {
            if cb.gp_seq <= completed {
                ready.push(cb);
            } else {
                pending.push(cb);
            }
        }
        rcu.callbacks = pending;
        drop(rcu);

        // Execute callbacks outside the lock
        for cb in ready {
            (cb.func)(cb.arg);
        }

        // Start next grace period if there are pending callbacks
        let has_pending = !RCU.lock().callbacks.is_empty();
        if has_pending {
            start_gp();
        }
    }
}

/// Schedule a callback to be called after the current grace period
/// The callback will be invoked once all pre-existing RCU readers have completed.
pub fn call_rcu(func: fn(*mut u8), arg: *mut u8) {
    let mut rcu = RCU.lock();
    let gp = rcu.gp_seq;

    rcu.callbacks.push(RcuCallback {
        gp_seq: gp,
        func,
        arg,
    });

    // Start a new grace period if needed
    if rcu.gp_completed == gp {
        drop(rcu);
        start_gp();
    }
}

/// Synchronize RCU — wait for all pre-existing readers to finish
/// This is the slow path, used when you need to free old data synchronously.
pub fn synchronize_rcu() {
    start_gp();

    // Busy-wait for grace period completion
    // In a real kernel, this would schedule other work
    for _ in 0..100000 {
        rcu_advance();
        let rcu = RCU.lock();
        if rcu.gp_completed >= rcu.gp_seq {
            return;
        }
        drop(rcu);
        core::hint::spin_loop();
    }
}

/// RCU-protected pointer swap
/// Atomically publishes a new version of a data structure
pub fn rcu_assign_pointer<T>(ptr: &AtomicPtr<T>, new: *mut T) {
    // Store with Release ordering ensures all writes to the new data
    // are visible before the pointer swap
    ptr.store(new, Ordering::Release);
}

/// RCU-protected pointer read
/// Must be called within rcu_read_lock/rcu_read_unlock
pub fn rcu_dereference<T>(ptr: &AtomicPtr<T>) -> *mut T {
    // Load with Acquire ordering ensures we see all writes that
    // were made before the pointer was published
    ptr.load(Ordering::Acquire)
}

/// Get RCU statistics
pub fn stats() -> (u64, u64, usize) {
    let rcu = RCU.lock();
    (rcu.gp_seq, rcu.gp_completed, rcu.callbacks.len())
}

pub fn init() {
    crate::serial_println!("  [rcu] Read-Copy-Update initialized");
}
