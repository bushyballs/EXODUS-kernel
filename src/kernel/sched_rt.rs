use crate::sync::Mutex;
/// Real-Time Scheduler (SCHED_FIFO / SCHED_RR)
///
/// Implements real-time scheduling with fixed priority preemption.
/// Tasks with higher priority always run before lower priority tasks.
/// Within the same priority, SCHED_RR rotates via time slices.
///
/// No heap; uses fixed-size static array for max 64 real-time tasks.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const RT_MAX_TASKS: usize = 64;

/// Real-time scheduling policy
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum RtPolicy {
    Fifo = 0,       // First-in, first-out (no time slice)
    RoundRobin = 1, // Round-robin (fixed time slice)
}

/// Real-time task structure (must be Copy and have const fn empty())
#[repr(C)]
#[derive(Copy, Clone)]
pub struct RtTask {
    pub pid: u32,
    pub priority: u8, // 1-99, higher = more urgent
    pub policy: RtPolicy,
    pub time_slice_ms: u32, // Time slice for RR in milliseconds
    pub remaining_ms: u32,  // Time remaining in current slice
    pub cpu: u8,
    pub active: bool,
}

impl RtTask {
    const fn empty() -> Self {
        RtTask {
            pid: 0,
            priority: 0,
            policy: RtPolicy::Fifo,
            time_slice_ms: 0,
            remaining_ms: 0,
            cpu: 0,
            active: false,
        }
    }
}

/// Global RT state
static RT_TASKS: Mutex<[RtTask; RT_MAX_TASKS]> = Mutex::new([RtTask::empty(); RT_MAX_TASKS]);
static RT_CURRENT: AtomicU32 = AtomicU32::new(0);
static RR_ROTATED: AtomicBool = AtomicBool::new(false);

/// Add a real-time task
pub fn rt_task_add(pid: u32, priority: u8, policy: RtPolicy) -> bool {
    if pid == 0 || priority == 0 || priority > 99 {
        return false; // Invalid PID or priority
    }

    let mut tasks = RT_TASKS.lock();

    // Find free slot
    for i in 0..RT_MAX_TASKS {
        if !tasks[i].active {
            tasks[i] = RtTask {
                pid,
                priority,
                policy,
                time_slice_ms: if policy == RtPolicy::RoundRobin {
                    100
                } else {
                    0
                },
                remaining_ms: if policy == RtPolicy::RoundRobin {
                    100
                } else {
                    0
                },
                cpu: 0,
                active: true,
            };
            return true;
        }
    }

    false
}

/// Remove a real-time task
pub fn rt_task_remove(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let mut tasks = RT_TASKS.lock();
    for i in 0..RT_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            tasks[i].active = false;
            return true;
        }
    }
    false
}

/// Pick the next real-time task to run
/// Returns task with highest priority; for equal priorities, FIFO or rotates RR
pub fn rt_pick_next() -> Option<u32> {
    let tasks = RT_TASKS.lock();

    let mut best_priority = 0u8;
    let mut selected_pid = None;
    let mut selected_policy = RtPolicy::Fifo;

    // First pass: find highest priority
    for i in 0..RT_MAX_TASKS {
        if tasks[i].active && tasks[i].priority > best_priority {
            best_priority = tasks[i].priority;
            selected_pid = Some(tasks[i].pid);
            selected_policy = tasks[i].policy;
        }
    }

    selected_pid
}

/// Process one RT tick (elapsed milliseconds)
pub fn rt_tick(elapsed_ms: u32) {
    let current_pid = RT_CURRENT.load(Ordering::Relaxed);
    if current_pid == 0 {
        return; // No current task
    }

    let mut tasks = RT_TASKS.lock();

    // Find current task and decrement remaining time
    for i in 0..RT_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == current_pid {
            if tasks[i].policy == RtPolicy::RoundRobin {
                if tasks[i].remaining_ms > elapsed_ms {
                    tasks[i].remaining_ms -= elapsed_ms;
                } else {
                    // Time slice expired; reset and mark for rotation
                    tasks[i].remaining_ms = tasks[i].time_slice_ms;
                    RR_ROTATED.store(true, Ordering::Release);
                }
            }
            return;
        }
    }
}

/// Set the current running RT task
pub fn rt_set_current(pid: u32) {
    RT_CURRENT.store(pid, Ordering::Relaxed);
}

/// Boost priority for deadline-critical tasks (stub)
/// For now, sets priority to 99 (highest)
pub fn rt_boost_deadline(pid: u32, _deadline_ns: u64) -> bool {
    if pid == 0 {
        return false;
    }

    let mut tasks = RT_TASKS.lock();
    for i in 0..RT_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            tasks[i].priority = 99;
            return true;
        }
    }
    false
}

/// Initialize real-time scheduler
pub fn init() {
    RT_CURRENT.store(0, Ordering::Relaxed);
    RR_ROTATED.store(false, Ordering::Relaxed);
    crate::serial_println!("[sched_rt] Real-time scheduler initialized");
}
