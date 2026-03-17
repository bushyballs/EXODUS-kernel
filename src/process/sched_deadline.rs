// sched_deadline — SCHED_DEADLINE real-time scheduler
//
// Implements Linux-compatible EDF (Earliest Deadline First) scheduling
// using the CBS (Constant Bandwidth Server) model.
//
// Each task declares three parameters:
//   runtime_ns  — maximum CPU time it may consume per period (budget)
//   period_ns   — length of each scheduling period
//   deadline_ns — deadline relative to period start (must be <= period_ns)
//
// CBS guarantees isolation: a task that exceeds its budget is throttled
// until the start of its next period, preventing one task from starving
// others regardless of how much CPU it tries to consume.
//
// Integration:
//   - dl_enqueue()        — add a task to the EDF run-queue (keeps sorted order)
//   - dl_pick_next()      — return the PID with the earliest absolute deadline
//   - dl_update_runtime() — charge elapsed time; throttle when budget exhausted
//   - dl_replenish_tick() — release throttled tasks on period rollover
//   - dl_schedule()       — top-level entry: DL-first, CFS fallback
//
// Wire-up in sched_core::schedule():
//   Before dequeuing from the CFS run-queue, call dl_schedule().
//   If it returns Some(pid), switch to that task immediately.
//
// No std, no float, no panics.  All arithmetic is saturating.
//
// Inspired by: Linux SCHED_DEADLINE / SCHED_EDF (concept), CBS paper
// (Abeni & Buttazzo 1998). All code is original.

use crate::sync::Mutex;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single SCHED_DEADLINE task.
///
/// All time values are in nanoseconds (TSC-based monotonic clock).
#[derive(Clone)]
pub struct DeadlineTask {
    /// Process identifier.
    pub pid: u32,
    /// Maximum CPU time per period (declared budget).
    pub runtime_ns: u64,
    /// Period length.
    pub period_ns: u64,
    /// Relative deadline within each period.
    pub deadline_ns: u64,
    /// Remaining runtime budget in the current period.
    pub remaining_ns: u64,
    /// Absolute deadline for the current job (nanoseconds, monotonic).
    pub abs_deadline: u64,
    /// Absolute timestamp at which the task's budget is replenished.
    pub next_release_ns: u64,
    /// True when the task has exhausted its budget and is waiting for
    /// the next period to start.
    pub throttled: bool,
}

impl DeadlineTask {
    /// Construct a new DeadlineTask.
    ///
    /// `enqueue_time_ns` is the current monotonic timestamp used to
    /// compute the initial absolute deadline.
    pub fn new(
        pid: u32,
        runtime_ns: u64,
        period_ns: u64,
        deadline_ns: u64,
        enqueue_time_ns: u64,
    ) -> Self {
        // Guard against degenerate parameters.
        let period_ns = period_ns.max(1);
        let deadline_ns = deadline_ns.min(period_ns).max(1);
        let runtime_ns = runtime_ns.min(deadline_ns).max(1);

        DeadlineTask {
            pid,
            runtime_ns,
            period_ns,
            deadline_ns,
            remaining_ns: runtime_ns,
            abs_deadline: enqueue_time_ns.saturating_add(deadline_ns),
            next_release_ns: 0, // only set when throttled
            throttled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// EDF run-queue
// ---------------------------------------------------------------------------

/// Global EDF deadline run-queue.
///
/// Kept sorted by `abs_deadline` ascending (min-heap property maintained
/// by insertion sort on every enqueue).  The queue is expected to be small
/// (tens of real-time tasks at most), so O(n) insertion is acceptable.
static DL_RUNQUEUE: Mutex<Vec<DeadlineTask>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Run-queue operations
// ---------------------------------------------------------------------------

/// Insert a `DeadlineTask` into the EDF run-queue, maintaining ascending
/// abs_deadline order (earliest deadline first).
///
/// If a task with the same PID already exists it is replaced.
pub fn dl_enqueue(task: DeadlineTask) {
    let mut rq = DL_RUNQUEUE.lock();

    // Remove any existing entry for this PID.
    rq.retain(|t| t.pid != task.pid);

    // Find the insertion position using binary search on abs_deadline.
    let pos = rq.partition_point(|t| t.abs_deadline <= task.abs_deadline);
    rq.insert(pos, task);
}

/// Remove a task from the EDF run-queue by PID.
pub fn dl_dequeue(pid: u32) {
    DL_RUNQUEUE.lock().retain(|t| t.pid != pid);
}

/// Return the PID of the task with the earliest absolute deadline that is
/// not throttled and still has remaining budget.
///
/// Returns `None` if the queue is empty or all tasks are throttled.
pub fn dl_pick_next() -> Option<u32> {
    let rq = DL_RUNQUEUE.lock();
    rq.iter()
        .find(|t| !t.throttled && t.remaining_ns > 0)
        .map(|t| t.pid)
}

/// Charge `elapsed_ns` of CPU time to `pid`.
///
/// If the task's remaining budget reaches zero:
///   1. Mark it throttled.
///   2. Compute `next_release_ns = abs_deadline + period_ns - deadline_ns`
///      (i.e., the start of the next period).
///   3. Reset `remaining_ns = runtime_ns` so it is ready when released.
///   4. Advance `abs_deadline` by `period_ns` for the next job.
pub fn dl_update_runtime(pid: u32, elapsed_ns: u64) {
    let mut rq = DL_RUNQUEUE.lock();
    let task = match rq.iter_mut().find(|t| t.pid == pid) {
        Some(t) => t,
        None => return,
    };

    task.remaining_ns = task.remaining_ns.saturating_sub(elapsed_ns);

    if task.remaining_ns == 0 {
        // Budget exhausted — throttle until next period.
        task.throttled = true;
        // Release at the start of next period:
        // next period start = current abs_deadline - relative_deadline + period
        let period_start = task.abs_deadline.saturating_sub(task.deadline_ns);
        task.next_release_ns = period_start.saturating_add(task.period_ns);
        // Replenish budget and advance deadline for the next job.
        task.remaining_ns = task.runtime_ns;
        task.abs_deadline = task.abs_deadline.saturating_add(task.period_ns);

        // Re-sort: remove and reinsert so ordering is maintained.
        // (We can't do this in-place because we hold the mutable borrow.)
        // We'll do the re-sort in dl_replenish_tick when unthrottling.
    }
}

/// Release any throttled tasks whose `next_release_ns` has passed.
///
/// Called from the timer interrupt handler on every tick.
/// `current_ns` is the current monotonic timestamp.
pub fn dl_replenish_tick(current_ns: u64) {
    let mut rq = DL_RUNQUEUE.lock();

    let mut reinsert: Vec<DeadlineTask> = Vec::new();
    let mut i = 0;
    while i < rq.len() {
        let task = &rq[i];
        if task.throttled && current_ns >= task.next_release_ns {
            let mut t = rq.remove(i);
            t.throttled = false;
            reinsert.push(t);
            // Don't increment i — element at i is now the next one.
        } else {
            i = i.saturating_add(1);
        }
    }

    // Reinsert unthrottled tasks in abs_deadline order.
    for task in reinsert {
        let pos = rq.partition_point(|t| t.abs_deadline <= task.abs_deadline);
        rq.insert(pos, task);
    }
}

// ---------------------------------------------------------------------------
// Top-level scheduler entry point
// ---------------------------------------------------------------------------

/// Scheduling entry point: try the DL run-queue first, fall back to CFS.
///
/// Returns `Some(pid)` if a SCHED_DEADLINE task should run next, or
/// `None` to let the CFS scheduler select.
///
/// This function is called from `sched_core::schedule()` at the start
/// of the scheduling decision, before the CFS dequeue.
pub fn dl_schedule() -> Option<u32> {
    dl_pick_next()
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Return the number of active (non-throttled) deadline tasks.
pub fn dl_active_count() -> usize {
    DL_RUNQUEUE.lock().iter().filter(|t| !t.throttled).count()
}

/// Return the number of throttled deadline tasks.
pub fn dl_throttled_count() -> usize {
    DL_RUNQUEUE.lock().iter().filter(|t| t.throttled).count()
}

/// Return a snapshot of deadline and remaining budget for every queued task.
///
/// Each element is `(pid, abs_deadline, remaining_ns, throttled)`.
pub fn dl_snapshot() -> Vec<(u32, u64, u64, bool)> {
    DL_RUNQUEUE
        .lock()
        .iter()
        .map(|t| (t.pid, t.abs_deadline, t.remaining_ns, t.throttled))
        .collect()
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the SCHED_DEADLINE subsystem.
///
/// Called from `process::init()`.
pub fn init() {
    crate::serial_println!(
        "    [sched_deadline] EDF deadline scheduler ready (CBS, integrated with CFS fallback)"
    );
}
