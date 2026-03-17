use crate::sync::Mutex;
/// Deadline Scheduler (EDF - Earliest Deadline First)
///
/// Implements EDF scheduling for hard real-time tasks with deadline guarantees.
/// Each task has a deadline, runtime budget, and period. The scheduler always
/// picks the task with the earliest absolute deadline.
///
/// Includes admission test: sum of (runtime/period) must be <= 1.0 (using fixed-point).
/// No heap; uses fixed-size static array for max 32 deadline tasks.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const DL_MAX_TASKS: usize = 32;
const DL_SCALE: u64 = 1_000_000; // Fixed-point scale for admission test

/// Deadline task structure (must be Copy and have const fn empty())
#[repr(C)]
#[derive(Copy, Clone)]
pub struct DlTask {
    pub pid: u32,
    pub runtime_ns: u64,       // Task runtime budget per period
    pub deadline_ns: u64,      // Relative deadline (from release)
    pub period_ns: u64,        // Period between releases
    pub abs_deadline: u64,     // Absolute deadline for current period
    pub abs_runtime_left: u64, // Runtime budget remaining in current period
    pub next_period: u64,      // Absolute time of next period release
    pub active: bool,
    pub throttled: bool, // Task exceeded budget
}

impl DlTask {
    const fn empty() -> Self {
        DlTask {
            pid: 0,
            runtime_ns: 0,
            deadline_ns: 0,
            period_ns: 0,
            abs_deadline: 0,
            abs_runtime_left: 0,
            next_period: 0,
            active: false,
            throttled: false,
        }
    }
}

/// Global deadline scheduler state
static DL_TASKS: Mutex<[DlTask; DL_MAX_TASKS]> = Mutex::new([DlTask::empty(); DL_MAX_TASKS]);
static DL_TIMER_NS: AtomicU64 = AtomicU64::new(0);

/// Add a deadline task
/// Validates: runtime <= deadline <= period
pub fn dl_task_add(pid: u32, runtime_ns: u64, deadline_ns: u64, period_ns: u64) -> bool {
    if pid == 0 || period_ns == 0 {
        return false; // Invalid PID or period
    }

    // Validate constraints: runtime <= deadline <= period
    if runtime_ns > deadline_ns || deadline_ns > period_ns {
        return false;
    }

    let mut tasks = DL_TASKS.lock();

    // Find free slot
    for i in 0..DL_MAX_TASKS {
        if !tasks[i].active {
            let current_time = DL_TIMER_NS.load(Ordering::Relaxed);
            tasks[i] = DlTask {
                pid,
                runtime_ns,
                deadline_ns,
                period_ns,
                abs_deadline: current_time.saturating_add(deadline_ns),
                abs_runtime_left: runtime_ns,
                next_period: current_time.saturating_add(period_ns),
                active: true,
                throttled: false,
            };
            return true;
        }
    }

    false
}

/// Remove a deadline task
pub fn dl_task_remove(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let mut tasks = DL_TASKS.lock();
    for i in 0..DL_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            tasks[i].active = false;
            return true;
        }
    }
    false
}

/// Pick next deadline task (non-throttled, earliest absolute deadline)
pub fn dl_pick_next(current_ns: u64) -> Option<u32> {
    let tasks = DL_TASKS.lock();

    let mut earliest_deadline = u64::MAX;
    let mut selected_pid = None;

    for i in 0..DL_MAX_TASKS {
        if tasks[i].active && !tasks[i].throttled && tasks[i].abs_deadline < earliest_deadline {
            earliest_deadline = tasks[i].abs_deadline;
            selected_pid = Some(tasks[i].pid);
        }
    }

    selected_pid
}

/// Replenish a task's budget and move to next period
pub fn dl_replenish(pid: u32, current_ns: u64) {
    if pid == 0 {
        return;
    }

    let mut tasks = DL_TASKS.lock();
    for i in 0..DL_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            // Reset budget and move to next period
            tasks[i].abs_runtime_left = tasks[i].runtime_ns;
            tasks[i].next_period = tasks[i].next_period.saturating_add(tasks[i].period_ns);
            tasks[i].abs_deadline = tasks[i].next_period.saturating_add(tasks[i].deadline_ns);
            tasks[i].throttled = false;
            return;
        }
    }
}

/// Process one deadline tick (1ms = 1_000_000 nanoseconds)
pub fn dl_tick(current_ns: u64) {
    const TICK_NS: u64 = 1_000_000;

    let mut tasks = DL_TASKS.lock();

    // Update timer
    DL_TIMER_NS.store(current_ns, Ordering::Relaxed);

    for i in 0..DL_MAX_TASKS {
        if !tasks[i].active {
            continue;
        }

        // Check if task's period has expired; if so, replenish
        if tasks[i].next_period <= current_ns {
            tasks[i].abs_runtime_left = tasks[i].runtime_ns;
            tasks[i].next_period = tasks[i].next_period.saturating_add(tasks[i].period_ns);
            tasks[i].abs_deadline = tasks[i].next_period.saturating_add(tasks[i].deadline_ns);
            tasks[i].throttled = false;
            continue;
        }

        // Decrement runtime budget
        if tasks[i].abs_runtime_left > TICK_NS {
            tasks[i].abs_runtime_left -= TICK_NS;
        } else {
            // Budget depleted; throttle the task
            tasks[i].abs_runtime_left = 0;
            tasks[i].throttled = true;
        }
    }
}

/// Admission test: new task (runtime, deadline, period) can be scheduled?
/// Sum of (runtime_i / period_i) for all active tasks + new task must be <= 1.0
/// Uses fixed-point arithmetic: sum of (runtime_i * SCALE / period_i) <= SCALE
pub fn dl_admission_test(runtime_ns: u64, _deadline_ns: u64, period_ns: u64) -> bool {
    if period_ns == 0 {
        return false; // Invalid period
    }

    let tasks = DL_TASKS.lock();

    // Compute sum of utilization
    let mut total_util = 0u64; // Scaled by DL_SCALE

    for i in 0..DL_MAX_TASKS {
        if !tasks[i].active || tasks[i].period_ns == 0 {
            continue; // Skip inactive tasks or division-by-zero
        }

        // util_i = (runtime_i * SCALE) / period_i
        let util_i = (tasks[i].runtime_ns.saturating_mul(DL_SCALE)) / (tasks[i].period_ns as u64);
        total_util = total_util.saturating_add(util_i);
    }

    // New task utilization
    let new_util = (runtime_ns.saturating_mul(DL_SCALE)) / (period_ns as u64);
    total_util = total_util.saturating_add(new_util);

    // Admit if total <= 1.0 (or DL_SCALE in fixed-point)
    total_util <= DL_SCALE
}

/// Initialize deadline scheduler
pub fn init() {
    DL_TIMER_NS.store(0, Ordering::Relaxed);
    crate::serial_println!("[sched_dl] Deadline (EDF) scheduler initialized");
}
