/// Simple CFS run-queue for the scheduler core.
///
/// This is a deliberately minimal implementation: a Vec<CfsTask> kept
/// sorted by vruntime after each insertion.  All arithmetic is saturating.
///
/// The full-featured CFS (with BTreeMap red-black tree, cgroups, SCHED_DEADLINE,
/// load balancing, etc.) lives in cfs.rs.  This module provides the lightweight
/// interface consumed by sched_core.rs.
///
/// No std, no float, no panics.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// CfsTask
// ---------------------------------------------------------------------------

/// A single runnable task tracked by the CFS run-queue.
///
/// Fits within 32 bytes (pid:4 + pad:4 + vruntime:8 + weight:4 + pad:12 = 32).
/// Aligned to 32 bytes so that two tasks share one 64-byte cache line, halving
/// cache pressure during the sorted-Vec scan in pick_next / update_vruntime.
// hot struct: iterated on every schedule() call (~1K/s)
#[repr(C, align(32))]
#[derive(Clone, Debug)]
pub struct CfsTask {
    /// Process identifier.
    pub pid: u32,
    // 4 bytes padding before vruntime (align u64 to 8)
    _pad0: u32,
    /// Virtual runtime (weighted nanoseconds).
    /// Lower vruntime = scheduled sooner.
    pub vruntime: u64,
    /// CFS weight (from nice_to_weight).  Higher weight = more CPU share.
    pub weight: u32,
    /// Padding to fill the 32-byte slot.
    _pad1: [u8; 12],
}

impl CfsTask {
    /// Construct a CfsTask from the three meaningful fields.
    /// Padding fields are zeroed automatically.
    pub fn new(pid: u32, vruntime: u64, weight: u32) -> Self {
        CfsTask {
            pid,
            _pad0: 0,
            vruntime,
            weight,
            _pad1: [0u8; 12],
        }
    }
}

// ---------------------------------------------------------------------------
// CfsRunQueue
// ---------------------------------------------------------------------------

/// A CFS run-queue sorted by vruntime (ascending).
///
/// Invariant: `tasks` is sorted by vruntime at all times.
pub struct CfsRunQueue {
    /// Runnable tasks, sorted by vruntime (ascending).
    pub tasks: Vec<CfsTask>,
    /// The smallest vruntime among all tasks.  New tasks start here to
    /// prevent starvation (they don't inherit a historically low vruntime).
    pub min_vruntime: u64,
    /// Sum of weights of all tasks in the queue.
    pub total_weight: u32,
}

impl CfsRunQueue {
    /// Create an empty run-queue.
    pub const fn new() -> Self {
        CfsRunQueue {
            tasks: Vec::new(),
            min_vruntime: 0,
            total_weight: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Enqueue / dequeue
    // -----------------------------------------------------------------------

    /// Add a task to the run-queue.
    ///
    /// If the task's vruntime is below min_vruntime, it is raised to
    /// min_vruntime to prevent newly-added/woken tasks from monopolising
    /// the CPU.
    ///
    /// The task is inserted at the correct sorted position using binary search.
    // hot path: called on every wakeup and re-schedule (~1K/s)
    #[inline]
    pub fn enqueue(&mut self, mut task: CfsTask) {
        // Enforce minimum vruntime (starvation prevention).
        if task.vruntime < self.min_vruntime {
            task.vruntime = self.min_vruntime;
        }

        // Accumulate total weight.
        self.total_weight = self.total_weight.saturating_add(task.weight);

        // Binary search for the insertion point.
        // We sort by (vruntime, pid) to make ties deterministic.
        let key = (task.vruntime, task.pid);
        let pos = self.tasks.partition_point(|t| (t.vruntime, t.pid) < key);
        self.tasks.insert(pos, task);
    }

    /// Remove and return the task with the lowest vruntime (the leftmost task).
    /// Returns None if the queue is empty.
    // hot path: called on every schedule() — the innermost scheduling operation
    #[inline(always)]
    pub fn dequeue_min(&mut self) -> Option<CfsTask> {
        if self.tasks.is_empty() {
            return None;
        }
        let task = self.tasks.remove(0);
        self.total_weight = self.total_weight.saturating_sub(task.weight);
        // Update min_vruntime to the next task's vruntime (or keep current).
        if let Some(first) = self.tasks.first() {
            if first.vruntime > self.min_vruntime {
                self.min_vruntime = first.vruntime;
            }
        }
        Some(task)
    }

    /// Remove a specific task by PID.
    /// Returns the task if found, None otherwise.
    pub fn dequeue(&mut self, pid: u32) {
        if let Some(pos) = self.tasks.iter().position(|t| t.pid == pid) {
            let task = self.tasks.remove(pos);
            self.total_weight = self.total_weight.saturating_sub(task.weight);
        }
        // Recompute min_vruntime from the remaining tasks.
        self.recompute_min_vruntime();
    }

    // -----------------------------------------------------------------------
    // vruntime update
    // -----------------------------------------------------------------------

    /// Charge `delta_ns` nanoseconds of CPU time to task `pid`.
    ///
    /// vruntime delta = (delta_ns * 1024) / weight
    ///
    /// Using 1024 (NICE_0_WEIGHT) as the reference: a task with nice 0
    /// (weight=1024) accumulates 1 ns of vruntime per 1 ns of real time;
    /// a heavier task (higher weight) accumulates less vruntime per real ns
    /// (so it runs more); a lighter task accumulates more (runs less).
    ///
    /// All arithmetic is saturating to avoid wrapping on overflow.
    ///
    /// After updating, the task is re-inserted at its new sorted position.
    pub fn update_vruntime(&mut self, pid: u32, delta_ns: u64) {
        if delta_ns == 0 {
            return;
        }

        let pos = match self.tasks.iter().position(|t| t.pid == pid) {
            Some(p) => p,
            None => return, // task not in queue (it is currently running, handled by caller)
        };

        let mut task = self.tasks.remove(pos);

        let weight = if task.weight == 0 { 1 } else { task.weight };
        // vruntime_delta = (delta_ns * NICE_0_WEIGHT) / weight
        // Use u128 intermediate to avoid overflow on large delta_ns values.
        let vrt_delta = ((delta_ns as u128).saturating_mul(1024) / weight as u128) as u64;
        task.vruntime = task.vruntime.saturating_add(vrt_delta);

        // Re-insert at correct sorted position.
        let key = (task.vruntime, task.pid);
        let new_pos = self.tasks.partition_point(|t| (t.vruntime, t.pid) < key);
        self.tasks.insert(new_pos, task);

        // Update total weight (unchanged — same task).
        // Update min_vruntime.
        self.recompute_min_vruntime();
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    /// Return the PID of the task with the lowest vruntime without removing it.
    // hot path: peeked from schedule() before deciding whether to switch
    #[inline(always)]
    pub fn pick_next(&self) -> Option<u32> {
        self.tasks.first().map(|t| t.pid)
    }

    /// Return the number of runnable tasks.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Return true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn recompute_min_vruntime(&mut self) {
        if let Some(first) = self.tasks.first() {
            // min_vruntime only ever moves forward.
            if first.vruntime > self.min_vruntime {
                self.min_vruntime = first.vruntime;
            }
        }
    }
}
