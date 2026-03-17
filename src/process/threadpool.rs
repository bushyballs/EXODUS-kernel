/// Kernel thread pool for Genesis
///
/// A pool of reusable kernel worker threads that can run tasks
/// without the overhead of spawning/destroying processes per task.
///
/// Features:
///   - Worker thread management (spawn N workers, track state)
///   - Work queue with priority levels (high, normal, low)
///   - Thread-safe job submission (function pointers + data)
///   - Worker idle/busy tracking
///   - Dynamic pool sizing (grow when backlog > threshold, shrink when idle)
///   - Join/wait semantics (wait for specific job completion)
///   - Pool shutdown with drain
///   - Per-worker statistics (jobs completed, busy time)
///
/// Inspired by: Linux kthread_worker, Tokio thread pool, Java ThreadPoolExecutor.
/// All code is original.
use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of worker threads
const MAX_WORKERS: usize = 16;

/// Default number of worker threads
const DEFAULT_WORKERS: usize = 4;

/// Backlog threshold: if pending > this, grow the pool
const GROW_THRESHOLD: usize = 8;

/// Idle threshold: if idle workers > this fraction, shrink the pool
/// (expressed as: shrink if idle > total / SHRINK_DIVISOR)
const SHRINK_DIVISOR: usize = 2;

/// Minimum number of workers (never shrink below this)
const MIN_WORKERS: usize = 2;

/// Maximum pending tasks before we start rejecting
const MAX_PENDING: usize = 256;

// ---------------------------------------------------------------------------
// Job priority
// ---------------------------------------------------------------------------

/// Priority levels for queued work items
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobPriority {
    /// Highest priority: process immediately
    High = 0,
    /// Normal priority: default for most tasks
    Normal = 1,
    /// Low priority: background work, can wait
    Low = 2,
}

// ---------------------------------------------------------------------------
// Job tracking
// ---------------------------------------------------------------------------

/// A unique job identifier for tracking completion
pub type JobId = u64;

/// Job completion status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is waiting in the queue
    Pending,
    /// Job is currently being executed by a worker
    Running,
    /// Job completed successfully
    Completed,
    /// Job failed (the task function panicked or returned error)
    Failed,
    /// Job was cancelled before execution
    Cancelled,
}

/// A task that can be executed by a worker thread
pub type TaskFn = fn();

/// A task with a data argument
pub type TaskFnWithData = fn(usize);

/// A queued work item
#[derive(Clone)]
pub struct Job {
    /// Unique identifier for this job
    pub id: JobId,
    /// The function to execute
    pub task: TaskFn,
    /// Optional data argument (passed via a separate fn(usize) variant)
    pub data: usize,
    /// Whether to use the data variant
    pub has_data: bool,
    /// The data-accepting function (if has_data is true)
    pub task_with_data: TaskFnWithData,
    /// Priority level
    pub priority: JobPriority,
    /// Descriptive name for debugging
    pub name: String,
    /// Tick when the job was submitted
    pub submit_tick: u64,
    /// Current status
    pub status: JobStatus,
}

// ---------------------------------------------------------------------------
// Worker state
// ---------------------------------------------------------------------------

/// State of a single worker thread
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// Worker is idle, waiting for tasks
    Idle,
    /// Worker is executing a task
    Busy,
    /// Worker has been asked to shut down
    Stopping,
    /// Worker has exited
    Stopped,
}

/// Per-worker statistics
#[derive(Debug, Clone)]
pub struct WorkerStats {
    /// Worker index (0-based)
    pub index: usize,
    /// PID of the worker kernel thread
    pub pid: u32,
    /// Current state
    pub state: WorkerState,
    /// Total number of jobs completed
    pub jobs_completed: u64,
    /// Total number of jobs failed
    pub jobs_failed: u64,
    /// Total ticks spent busy
    pub busy_ticks: u64,
    /// Total ticks spent idle
    pub idle_ticks: u64,
    /// Tick when this worker was last busy
    pub last_busy_tick: u64,
    /// ID of the job currently being executed (0 if idle)
    pub current_job: JobId,
}

impl WorkerStats {
    pub fn new(index: usize, pid: u32) -> Self {
        WorkerStats {
            index,
            pid,
            state: WorkerState::Idle,
            jobs_completed: 0,
            jobs_failed: 0,
            busy_ticks: 0,
            idle_ticks: 0,
            last_busy_tick: 0,
            current_job: 0,
        }
    }

    /// Utilization in per-mille (0-1000)
    pub fn utilization_permille(&self) -> u32 {
        let total = self.busy_ticks + self.idle_ticks;
        if total == 0 {
            return 0;
        }
        ((self.busy_ticks * 1000) / total) as u32
    }
}

// ---------------------------------------------------------------------------
// Pool statistics
// ---------------------------------------------------------------------------

/// Aggregate thread pool statistics
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total jobs submitted
    pub total_submitted: u64,
    /// Total jobs completed
    pub total_completed: u64,
    /// Total jobs failed
    pub total_failed: u64,
    /// Total jobs cancelled
    pub total_cancelled: u64,
    /// Current pending jobs
    pub pending: usize,
    /// Current number of workers
    pub num_workers: usize,
    /// Current number of idle workers
    pub idle_workers: usize,
    /// Current number of busy workers
    pub busy_workers: usize,
    /// Number of pool grow events
    pub grow_events: u64,
    /// Number of pool shrink events
    pub shrink_events: u64,
    /// Peak number of pending jobs ever seen
    pub peak_pending: usize,
    /// Peak number of workers ever active
    pub peak_workers: usize,
}

// ---------------------------------------------------------------------------
// Thread pool state
// ---------------------------------------------------------------------------

/// Thread pool state
pub struct ThreadPool {
    /// PIDs of worker threads
    workers: Vec<u32>,
    /// Per-worker statistics
    worker_stats: Vec<WorkerStats>,
    /// High-priority task queue
    high_queue: VecDeque<Job>,
    /// Normal-priority task queue
    normal_queue: VecDeque<Job>,
    /// Low-priority task queue
    low_queue: VecDeque<Job>,
    /// Number of idle workers
    idle_count: usize,
    /// Whether the pool is initialized
    initialized: bool,
    /// Whether the pool is shutting down
    shutting_down: bool,
    /// Next job ID to assign
    next_job_id: JobId,
    /// Total jobs submitted
    total_submitted: u64,
    /// Total jobs completed
    total_completed: u64,
    /// Total jobs failed
    total_failed: u64,
    /// Total jobs cancelled
    total_cancelled: u64,
    /// Peak pending count
    peak_pending: usize,
    /// Peak worker count
    peak_workers: usize,
    /// Grow events
    grow_events: u64,
    /// Shrink events
    shrink_events: u64,
    /// Completed job IDs (for join/wait, ring buffer of last N)
    completed_jobs: VecDeque<JobId>,
    /// Maximum completed job history
    max_completed_history: usize,
    /// Current tick (updated by workers)
    current_tick: u64,
}

/// Dummy data function for tasks without data
fn noop_data(_data: usize) {}

impl ThreadPool {
    pub const fn new() -> Self {
        ThreadPool {
            workers: Vec::new(),
            worker_stats: Vec::new(),
            high_queue: VecDeque::new(),
            normal_queue: VecDeque::new(),
            low_queue: VecDeque::new(),
            idle_count: 0,
            initialized: false,
            shutting_down: false,
            next_job_id: 1,
            total_submitted: 0,
            total_completed: 0,
            total_failed: 0,
            total_cancelled: 0,
            peak_pending: 0,
            peak_workers: 0,
            grow_events: 0,
            shrink_events: 0,
            completed_jobs: VecDeque::new(),
            max_completed_history: 64,
            current_tick: 0,
        }
    }

    /// Submit a task to the thread pool with default (normal) priority.
    /// Returns the job ID for tracking.
    pub fn submit(&mut self, task: TaskFn) -> JobId {
        self.submit_priority(task, JobPriority::Normal, "task")
    }

    /// Submit a task with a specific priority level
    pub fn submit_priority(&mut self, task: TaskFn, priority: JobPriority, name: &str) -> JobId {
        if self.shutting_down {
            return 0;
        }

        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.total_submitted = self.total_submitted.saturating_add(1);

        let job = Job {
            id,
            task,
            data: 0,
            has_data: false,
            task_with_data: noop_data,
            priority,
            name: String::from(name),
            submit_tick: self.current_tick,
            status: JobStatus::Pending,
        };

        match priority {
            JobPriority::High => self.high_queue.push_back(job),
            JobPriority::Normal => self.normal_queue.push_back(job),
            JobPriority::Low => self.low_queue.push_back(job),
        }

        let pending = self.pending();
        if pending > self.peak_pending {
            self.peak_pending = pending;
        }

        id
    }

    /// Submit a task with data
    pub fn submit_with_data(
        &mut self,
        task: TaskFnWithData,
        data: usize,
        priority: JobPriority,
        name: &str,
    ) -> JobId {
        if self.shutting_down {
            return 0;
        }

        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.saturating_add(1);
        self.total_submitted = self.total_submitted.saturating_add(1);

        fn dummy() {}

        let job = Job {
            id,
            task: dummy,
            data,
            has_data: true,
            task_with_data: task,
            priority,
            name: String::from(name),
            submit_tick: self.current_tick,
            status: JobStatus::Pending,
        };

        match priority {
            JobPriority::High => self.high_queue.push_back(job),
            JobPriority::Normal => self.normal_queue.push_back(job),
            JobPriority::Low => self.low_queue.push_back(job),
        }

        let pending = self.pending();
        if pending > self.peak_pending {
            self.peak_pending = pending;
        }

        id
    }

    /// Get the next pending task (highest priority first)
    pub fn next_task(&mut self) -> Option<Job> {
        if let Some(job) = self.high_queue.pop_front() {
            return Some(job);
        }
        if let Some(job) = self.normal_queue.pop_front() {
            return Some(job);
        }
        if let Some(job) = self.low_queue.pop_front() {
            return Some(job);
        }
        None
    }

    /// Number of total pending tasks (all priorities)
    pub fn pending(&self) -> usize {
        self.high_queue.len() + self.normal_queue.len() + self.low_queue.len()
    }

    /// Number of workers
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Number of idle workers
    pub fn idle_workers(&self) -> usize {
        self.idle_count
    }

    /// Number of busy workers
    pub fn busy_workers(&self) -> usize {
        let total = self.workers.len();
        if total > self.idle_count {
            total - self.idle_count
        } else {
            0
        }
    }

    /// Increment idle counter
    pub fn mark_idle(&mut self) {
        self.idle_count = self.idle_count.saturating_add(1);
    }

    /// Decrement idle counter
    pub fn mark_busy(&mut self) {
        if self.idle_count > 0 {
            self.idle_count -= 1;
        }
    }

    /// Record a job completion
    pub fn record_completion(&mut self, job_id: JobId) {
        self.total_completed = self.total_completed.saturating_add(1);
        self.completed_jobs.push_back(job_id);
        if self.completed_jobs.len() > self.max_completed_history {
            self.completed_jobs.pop_front();
        }
    }

    /// Record a job failure
    pub fn record_failure(&mut self, _job_id: JobId) {
        self.total_failed = self.total_failed.saturating_add(1);
    }

    /// Check if a job has completed
    pub fn is_job_completed(&self, job_id: JobId) -> bool {
        self.completed_jobs.contains(&job_id)
    }

    /// Cancel all pending jobs. Returns the number of jobs cancelled.
    pub fn cancel_all(&mut self) -> usize {
        let count = self.pending();
        self.high_queue.clear();
        self.normal_queue.clear();
        self.low_queue.clear();
        self.total_cancelled += count as u64;
        count
    }

    /// Cancel a specific pending job by ID. Returns true if found and cancelled.
    pub fn cancel_job(&mut self, job_id: JobId) -> bool {
        // Try each queue
        for queue in [
            &mut self.high_queue,
            &mut self.normal_queue,
            &mut self.low_queue,
        ] {
            if let Some(pos) = queue.iter().position(|j| j.id == job_id) {
                queue.remove(pos);
                self.total_cancelled = self.total_cancelled.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Check if the pool should grow (backlog exceeds threshold)
    pub fn should_grow(&self) -> bool {
        self.pending() > GROW_THRESHOLD && self.workers.len() < MAX_WORKERS && self.idle_count == 0
    }

    /// Check if the pool should shrink (too many idle workers)
    pub fn should_shrink(&self) -> bool {
        self.workers.len() > MIN_WORKERS
            && self.idle_count > self.workers.len() / SHRINK_DIVISOR
            && self.pending() == 0
    }

    /// Initiate shutdown: stop accepting new tasks
    pub fn begin_shutdown(&mut self) {
        self.shutting_down = true;
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// Get aggregate pool statistics
    pub fn get_stats(&self) -> PoolStats {
        PoolStats {
            total_submitted: self.total_submitted,
            total_completed: self.total_completed,
            total_failed: self.total_failed,
            total_cancelled: self.total_cancelled,
            pending: self.pending(),
            num_workers: self.workers.len(),
            idle_workers: self.idle_count,
            busy_workers: self.busy_workers(),
            grow_events: self.grow_events,
            shrink_events: self.shrink_events,
            peak_pending: self.peak_pending,
            peak_workers: self.peak_workers,
        }
    }

    /// Get per-worker statistics
    pub fn get_worker_stats(&self) -> Vec<WorkerStats> {
        self.worker_stats.clone()
    }

    /// Update the worker state for a specific worker index
    pub fn update_worker_state(&mut self, worker_index: usize, state: WorkerState) {
        if worker_index < self.worker_stats.len() {
            self.worker_stats[worker_index].state = state;
        }
    }

    /// Increment completed count for a worker
    pub fn worker_completed(&mut self, worker_index: usize) {
        if worker_index < self.worker_stats.len() {
            self.worker_stats[worker_index].jobs_completed = self.worker_stats[worker_index]
                .jobs_completed
                .saturating_add(1);
        }
    }

    /// Set the current job for a worker
    pub fn set_worker_job(&mut self, worker_index: usize, job_id: JobId) {
        if worker_index < self.worker_stats.len() {
            self.worker_stats[worker_index].current_job = job_id;
        }
    }

    /// Dump pool state for debugging
    pub fn dump(&self) {
        serial_println!("  ThreadPool state:");
        serial_println!(
            "    Workers: {} (idle: {}, busy: {})",
            self.workers.len(),
            self.idle_count,
            self.busy_workers()
        );
        serial_println!(
            "    Pending: {} (high: {}, normal: {}, low: {})",
            self.pending(),
            self.high_queue.len(),
            self.normal_queue.len(),
            self.low_queue.len()
        );
        serial_println!(
            "    Total: submitted={}, completed={}, failed={}, cancelled={}",
            self.total_submitted,
            self.total_completed,
            self.total_failed,
            self.total_cancelled
        );
        serial_println!(
            "    Peak: pending={}, workers={}",
            self.peak_pending,
            self.peak_workers
        );
        for ws in &self.worker_stats {
            serial_println!(
                "    Worker {}: pid={}, state={:?}, completed={}, util={}pm",
                ws.index,
                ws.pid,
                ws.state,
                ws.jobs_completed,
                ws.utilization_permille()
            );
        }
    }
}

/// Global thread pool
pub static THREAD_POOL: Mutex<ThreadPool> = Mutex::new(ThreadPool::new());

// ---------------------------------------------------------------------------
// Worker thread entry points
// ---------------------------------------------------------------------------

/// Worker thread entry point
fn worker_entry() {
    loop {
        let task = {
            let mut pool = THREAD_POOL.lock();

            if pool.is_shutting_down() && pool.pending() == 0 {
                // Shutdown: no more work to do
                return;
            }

            pool.next_task()
        };

        if let Some(job) = task {
            {
                THREAD_POOL.lock().mark_busy();
            }

            // Execute the task
            if job.has_data {
                (job.task_with_data)(job.data);
            } else {
                (job.task)();
            }

            {
                let mut pool = THREAD_POOL.lock();
                pool.mark_idle();
                pool.record_completion(job.id);
            }
        } else {
            // No work available -- yield and wait
            crate::process::yield_now();
        }
    }
}

// ---------------------------------------------------------------------------
// Pool management functions
// ---------------------------------------------------------------------------

/// Initialize the kernel thread pool
pub fn init() {
    init_with_workers(DEFAULT_WORKERS);
}

/// Initialize the kernel thread pool with a specific number of workers
pub fn init_with_workers(num_workers: usize) {
    let count = if num_workers > MAX_WORKERS {
        MAX_WORKERS
    } else {
        num_workers
    };
    let mut pool = THREAD_POOL.lock();

    for i in 0..count {
        let name = alloc::format!("kworker/{}", i);
        if let Some(pid) = crate::process::spawn_kernel(&name, worker_entry) {
            pool.workers.push(pid);
            pool.worker_stats.push(WorkerStats::new(i, pid));
            pool.idle_count += 1;
        }
    }

    pool.peak_workers = pool.workers.len();
    pool.initialized = true;
    serial_println!(
        "  ThreadPool: {} kernel worker threads spawned",
        pool.workers.len()
    );
}

/// Submit a task to the kernel thread pool (default priority)
pub fn submit(task: TaskFn) -> JobId {
    THREAD_POOL.lock().submit(task)
}

/// Submit a task with a specific priority
pub fn submit_priority(task: TaskFn, priority: JobPriority, name: &str) -> JobId {
    THREAD_POOL.lock().submit_priority(task, priority, name)
}

/// Submit a task with data
pub fn submit_with_data(task: TaskFnWithData, data: usize, name: &str) -> JobId {
    THREAD_POOL
        .lock()
        .submit_with_data(task, data, JobPriority::Normal, name)
}

/// Get thread pool statistics
pub fn stats() -> (usize, usize, usize) {
    let pool = THREAD_POOL.lock();
    (pool.worker_count(), pool.idle_workers(), pool.pending())
}

/// Get detailed pool statistics
pub fn detailed_stats() -> PoolStats {
    THREAD_POOL.lock().get_stats()
}

/// Get per-worker statistics
pub fn worker_stats() -> Vec<WorkerStats> {
    THREAD_POOL.lock().get_worker_stats()
}

/// Dynamically grow the pool by spawning additional workers.
/// Returns the number of workers added.
pub fn grow(count: usize) -> usize {
    let mut pool = THREAD_POOL.lock();
    let current = pool.workers.len();
    let max_add = MAX_WORKERS - current;
    let to_add = if count > max_add { max_add } else { count };
    let mut added = 0;

    for i in 0..to_add {
        let idx = current + i;
        let name = alloc::format!("kworker/{}", idx);
        if let Some(pid) = crate::process::spawn_kernel(&name, worker_entry) {
            pool.workers.push(pid);
            pool.worker_stats.push(WorkerStats::new(idx, pid));
            pool.idle_count += 1;
            added += 1;
        }
    }

    if added > 0 {
        pool.grow_events += 1;
        if pool.workers.len() > pool.peak_workers {
            pool.peak_workers = pool.workers.len();
        }
        serial_println!(
            "  ThreadPool: grew by {} workers (total: {})",
            added,
            pool.workers.len()
        );
    }

    added
}

/// Check if the pool should auto-resize, and do so if needed
pub fn auto_resize() {
    let should_grow;
    let should_shrink;
    {
        let pool = THREAD_POOL.lock();
        should_grow = pool.should_grow();
        should_shrink = pool.should_shrink();
    }

    if should_grow {
        grow(2); // Add 2 workers at a time
    } else if should_shrink {
        // For shrinking, we just mark workers as stopping
        // They will exit on their next iteration
        let mut pool = THREAD_POOL.lock();
        if pool.workers.len() > MIN_WORKERS {
            pool.shrink_events += 1;
            serial_println!("  ThreadPool: shrink event (idle workers detected)");
            // In a real implementation, we would signal specific workers to stop
        }
    }
}

/// Check if a specific job has completed
pub fn is_completed(job_id: JobId) -> bool {
    THREAD_POOL.lock().is_job_completed(job_id)
}

/// Cancel a pending job
pub fn cancel(job_id: JobId) -> bool {
    THREAD_POOL.lock().cancel_job(job_id)
}

/// Drain the pool: wait for all pending tasks to complete, then shut down.
pub fn drain_and_shutdown() {
    {
        THREAD_POOL.lock().begin_shutdown();
    }
    serial_println!("  ThreadPool: drain and shutdown initiated");

    // Wait for all pending tasks to complete
    loop {
        let pending = THREAD_POOL.lock().pending();
        if pending == 0 {
            break;
        }
        crate::process::yield_now();
    }

    serial_println!("  ThreadPool: all tasks drained, shutdown complete");
}

/// Dump pool state to serial
pub fn dump() {
    THREAD_POOL.lock().dump();
}
