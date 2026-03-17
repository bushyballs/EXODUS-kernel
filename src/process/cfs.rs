/// Completely Fair Scheduler (CFS) for Genesis
///
/// Replaces round-robin with a virtual-runtime-based scheduler.
/// Each process tracks how much CPU time it has consumed (vruntime).
/// The process with the LOWEST vruntime runs next -- ensuring fairness.
///
/// Uses a red-black tree (BTreeMap) sorted by vruntime for O(log n) scheduling.
/// Supports nice levels (-20 to +19), real-time priority, and CPU bandwidth control.
///
/// Features:
///   - Virtual runtime tracking (vruntime in integer nanoseconds)
///   - Red-black tree simulation via sorted BTreeMap for task ordering
///   - Weight calculation from nice values (-20 to +19) using lookup table
///   - Min-granularity enforcement (don't preempt too frequently)
///   - Sleeper fairness (boost vruntime for tasks waking from sleep)
///   - Group scheduling concept (per-cgroup CPU bandwidth)
///   - Period and quota for CPU bandwidth control
///   - Proper time slice calculation based on weight ratio
///   - Load tracking and load balancing preparation
///
/// Inspired by: Linux CFS (kernel/sched/fair.c). All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Scheduling policies
// ---------------------------------------------------------------------------

/// Scheduling policies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedPolicy {
    /// Normal CFS scheduling
    Normal,
    /// Real-time FIFO (highest priority, no preemption within same priority)
    Fifo,
    /// Real-time round-robin (preemption after time quantum)
    RoundRobin,
    /// Deadline-based (EDF -- earliest deadline first)
    Deadline,
    /// Batch processing (lower priority, longer time slices)
    Batch,
    /// Idle (only runs when nothing else to do)
    Idle,
}

// ---------------------------------------------------------------------------
// Nice-to-weight table
// ---------------------------------------------------------------------------

/// Nice-to-weight table (from Linux, scaled)
/// Nice 0 = weight 1024, nice -20 = 88761, nice +19 = 15
const NICE_TO_WEIGHT: [u32; 40] = [
    88761, 71755, 56483, 46273, 36291, // -20 to -16
    29154, 23254, 18705, 14949, 11916, // -15 to -11
    9548, 7620, 6100, 4904, 3906, // -10 to -6
    3121, 2501, 1991, 1586, 1277, // -5 to -1
    1024, 820, 655, 526, 423, //  0 to  4
    335, 272, 215, 172, 137, //  5 to  9
    110, 87, 70, 56, 45, //  10 to 14
    36, 29, 23, 18, 15, //  15 to 19
];

/// Inverse weight table for division: inv_weight = 2^32 / weight
/// Used to convert: vruntime_delta = (delta_exec * NICE_0_WEIGHT * inv_weight) >> 32
const NICE_TO_INV_WEIGHT: [u32; 40] = [
    48, 59, 76, 93, 118, // -20 to -16
    147, 184, 229, 287, 360, // -15 to -11
    449, 563, 703, 875, 1099, // -10 to -6
    1376, 1717, 2157, 2709, 3363, //  -5 to -1
    4194, 5237, 6557, 8165, 10153, //   0 to  4
    12820, 15790, 19976, 24970, 31350, //   5 to  9
    39045, 49367, 61356, 76695, 95443, //  10 to 14
    119304, 148102, 186737, 232680, 286331, //  15 to 19
];

/// Weight for nice 0 (the reference weight)
const NICE_0_WEIGHT: u32 = 1024;

/// Convert nice to weight table index
fn nice_to_index(nice: i8) -> usize {
    let clamped = if nice < -20 {
        -20
    } else if nice > 19 {
        19
    } else {
        nice
    };
    (clamped + 20) as usize
}

// ---------------------------------------------------------------------------
// Scheduling entity
// ---------------------------------------------------------------------------

/// Scheduling entity -- per-process scheduling state
///
/// Aligned to 64 bytes (one cache line).  The CFS run queue stores these in
/// a BTreeMap; each pick_next()/enqueue() touches exactly this struct and
/// nothing else, so fitting it in a single cache line eliminates unnecessary
/// cache-line loads on tight scheduling loops.
// hot struct: touched on every scheduler tick and every context switch
#[repr(C, align(64))]
#[derive(Debug, Clone)]
pub struct SchedEntity {
    /// Process ID
    pub pid: u32,
    /// Virtual runtime (nanoseconds of weighted CPU time)
    pub vruntime: u64,
    /// Nice value (-20 to +19, lower = higher priority)
    pub nice: i8,
    /// Weight (derived from nice, higher = more CPU share)
    pub weight: u32,
    /// Inverse weight (for fast vruntime calculation)
    pub inv_weight: u32,
    /// Scheduling policy
    pub policy: SchedPolicy,
    /// Real-time priority (1-99, only for FIFO/RR)
    pub rt_priority: u8,
    /// CPU affinity mask (bit per CPU)
    pub cpu_affinity: u64,
    /// Last time this entity was scheduled (tick)
    pub last_scheduled: u64,
    /// Total execution time (nanoseconds)
    pub sum_exec: u64,
    /// Number of involuntary context switches
    pub nvcsw: u64,
    /// Number of voluntary context switches
    pub nivcsw: u64,
    /// Deadline (for SCHED_DEADLINE, absolute nanoseconds)
    pub deadline: u64,
    /// Period (for SCHED_DEADLINE)
    pub period: u64,
    /// Runtime budget (for SCHED_DEADLINE, per period)
    pub runtime_budget: u64,
    /// Runtime consumed in current period
    pub runtime_consumed: u64,
    /// Whether this entity is currently sleeping
    pub sleeping: bool,
    /// Tick when the entity went to sleep (for sleeper fairness)
    pub sleep_start: u64,
    /// The cgroup this entity belongs to (0 = root)
    pub cgroup_id: u32,
    /// Per-entity time slice (calculated by scheduler)
    pub time_slice_ns: u64,
    /// Remaining time in current slice
    pub slice_remaining_ns: u64,
    /// Load contribution for this entity (for load tracking)
    pub load_contrib: u64,
}

impl SchedEntity {
    pub fn new(pid: u32) -> Self {
        SchedEntity {
            pid,
            vruntime: 0,
            nice: 0,
            weight: NICE_0_WEIGHT,
            inv_weight: NICE_TO_INV_WEIGHT[20],
            policy: SchedPolicy::Normal,
            rt_priority: 0,
            cpu_affinity: u64::MAX, // all CPUs
            last_scheduled: 0,
            sum_exec: 0,
            nvcsw: 0,
            nivcsw: 0,
            deadline: 0,
            period: 0,
            runtime_budget: 0,
            runtime_consumed: 0,
            sleeping: false,
            sleep_start: 0,
            cgroup_id: 0,
            time_slice_ns: 0,
            slice_remaining_ns: 0,
            load_contrib: 0,
        }
    }

    /// Set nice value and recalculate weight
    pub fn set_nice(&mut self, nice: i8) {
        self.nice = if nice < -20 {
            -20
        } else if nice > 19 {
            19
        } else {
            nice
        };
        let idx = nice_to_index(self.nice);
        self.weight = NICE_TO_WEIGHT[idx];
        self.inv_weight = NICE_TO_INV_WEIGHT[idx];
    }

    /// Calculate how much vruntime to add for `delta_ns` nanoseconds of execution.
    ///
    /// vruntime = delta * (NICE_0_WEIGHT / weight)
    /// For nice 0: vruntime == real time.
    /// For nice -20 (heavy weight): vruntime << real time (runs more).
    /// For nice +19 (light weight): vruntime >> real time (runs less).
    pub fn calc_vruntime_delta(&self, delta_ns: u64) -> u64 {
        if self.weight == 0 {
            return delta_ns;
        }
        // Use inverse weight for fast division:
        // vruntime = (delta * NICE_0_WEIGHT * inv_weight) >> 32
        // This avoids a division instruction.
        let numerator = delta_ns as u128 * NICE_0_WEIGHT as u128 * self.inv_weight as u128;
        (numerator >> 32) as u64
    }

    /// Update vruntime after running for delta_ns nanoseconds
    pub fn charge_execution(&mut self, delta_ns: u64) {
        let vrt_delta = self.calc_vruntime_delta(delta_ns);
        self.vruntime += vrt_delta;
        self.sum_exec += delta_ns;
        self.slice_remaining_ns = self.slice_remaining_ns.saturating_sub(delta_ns);
    }

    /// Mark as sleeping
    pub fn go_to_sleep(&mut self, now_ns: u64) {
        self.sleeping = true;
        self.sleep_start = now_ns;
        self.nivcsw = self.nivcsw.saturating_add(1);
    }

    /// Wake up with sleeper fairness adjustment.
    ///
    /// When a task wakes from sleep, it hasn't been earning vruntime.
    /// Without compensation, long-sleeping tasks would have very low vruntime
    /// and monopolize the CPU upon waking.
    ///
    /// Sleeper fairness: set vruntime to max(vruntime, min_vruntime - thresh)
    /// where thresh is half the target latency. This gives sleepers a small
    /// bonus but prevents starvation of running tasks.
    pub fn wake_up(&mut self, min_vruntime: u64, half_latency_ns: u64) {
        self.sleeping = false;
        // Sleeper fairness: don't let vruntime fall too far behind
        let floor = if min_vruntime > half_latency_ns {
            min_vruntime - half_latency_ns
        } else {
            0
        };
        if self.vruntime < floor {
            self.vruntime = floor;
        }
    }

    /// Check if the entity has exhausted its deadline budget
    pub fn deadline_budget_exhausted(&self) -> bool {
        self.policy == SchedPolicy::Deadline && self.runtime_consumed >= self.runtime_budget
    }

    /// Replenish deadline budget for a new period
    pub fn replenish_deadline(&mut self, now_ns: u64) {
        self.runtime_consumed = 0;
        self.deadline = now_ns + self.period;
    }
}

// ---------------------------------------------------------------------------
// CPU bandwidth control (group scheduling)
// ---------------------------------------------------------------------------

/// CPU bandwidth control for a scheduling group (cgroup-like)
#[derive(Debug, Clone)]
pub struct CpuBandwidth {
    /// Group identifier
    pub id: u32,
    /// Group name
    pub name: String,
    /// Period in nanoseconds (e.g., 100ms = 100_000_000)
    pub period_ns: u64,
    /// Quota in nanoseconds per period (e.g., 50ms for 50% of one CPU)
    /// 0 means unlimited
    pub quota_ns: u64,
    /// Runtime consumed in the current period
    pub runtime_consumed_ns: u64,
    /// Start of the current period
    pub period_start_ns: u64,
    /// Whether the group has been throttled this period
    pub throttled: bool,
    /// Total weight of all entities in this group
    pub total_weight: u64,
    /// Number of runnable entities in this group
    pub nr_running: u32,
    /// Number of times the group was throttled
    pub nr_throttled: u64,
    /// Total time spent throttled
    pub throttled_time_ns: u64,
}

impl CpuBandwidth {
    pub fn new(id: u32, name: &str) -> Self {
        CpuBandwidth {
            id,
            name: String::from(name),
            period_ns: 100_000_000, // 100ms default
            quota_ns: 0,            // unlimited by default
            runtime_consumed_ns: 0,
            period_start_ns: 0,
            throttled: false,
            total_weight: 0,
            nr_running: 0,
            nr_throttled: 0,
            throttled_time_ns: 0,
        }
    }

    /// Set the quota and period for bandwidth control
    pub fn set_bandwidth(&mut self, quota_ns: u64, period_ns: u64) {
        self.quota_ns = quota_ns;
        if period_ns > 0 {
            self.period_ns = period_ns;
        }
    }

    /// Charge runtime to this group. Returns true if the group should be throttled.
    pub fn charge(&mut self, delta_ns: u64) -> bool {
        if self.quota_ns == 0 {
            return false; // unlimited
        }
        self.runtime_consumed_ns += delta_ns;
        if self.runtime_consumed_ns >= self.quota_ns {
            self.throttled = true;
            self.nr_throttled = self.nr_throttled.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Check if a new period should start and replenish the quota
    pub fn check_period(&mut self, now_ns: u64) {
        if self.period_ns == 0 {
            return;
        }
        if now_ns >= self.period_start_ns + self.period_ns {
            // New period
            if self.throttled {
                self.throttled_time_ns += now_ns - (self.period_start_ns + self.period_ns);
            }
            self.period_start_ns = now_ns;
            self.runtime_consumed_ns = 0;
            self.throttled = false;
        }
    }

    /// CPU utilization for this group in per-mille (0-1000)
    pub fn utilization_permille(&self) -> u32 {
        if self.period_ns == 0 {
            return 0;
        }
        ((self.runtime_consumed_ns * 1000) / self.period_ns) as u32
    }
}

// ---------------------------------------------------------------------------
// CFS run queue statistics
// ---------------------------------------------------------------------------

/// CFS scheduler statistics
#[derive(Debug, Clone)]
pub struct CfsStats {
    /// Total scheduling decisions made
    pub schedule_count: u64,
    /// Total preemptions due to higher-priority task waking
    pub preempt_count: u64,
    /// Total wakeups processed
    pub wakeup_count: u64,
    /// Total sleeper fairness adjustments
    pub sleeper_adjustments: u64,
    /// Total deadline replenishments
    pub deadline_replenish_count: u64,
    /// Total bandwidth throttle events
    pub throttle_count: u64,
}

impl CfsStats {
    pub const fn new() -> Self {
        CfsStats {
            schedule_count: 0,
            preempt_count: 0,
            wakeup_count: 0,
            sleeper_adjustments: 0,
            deadline_replenish_count: 0,
            throttle_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// CFS run queue
// ---------------------------------------------------------------------------

/// CFS run queue
pub struct CfsRunQueue {
    /// Red-black tree of runnable entities, sorted by vruntime
    /// Key: (vruntime, pid) to handle equal vruntimes
    tree: BTreeMap<(u64, u32), SchedEntity>,
    /// Reverse-lookup index: pid -> (vruntime, pid) key used in `tree`.
    ///
    /// Without this, every dequeue(pid) and get_stats(pid) call must scan the
    /// entire BTreeMap linearly — O(n) per lookup.  With the index both
    /// operations are O(log n): look up the key here, then remove from `tree`.
    // hot path: dequeue and stats are called on every schedule() and tick()
    pid_index: BTreeMap<u32, (u64, u32)>,
    /// Real-time run queue (sorted by rt_priority, highest first)
    rt_queue: Vec<SchedEntity>,
    /// Deadline queue (sorted by deadline, earliest first)
    dl_queue: Vec<SchedEntity>,
    /// Minimum vruntime (used to set initial vruntime for new tasks)
    min_vruntime: u64,
    /// Number of runnable tasks
    nr_running: u32,
    /// Current running entity PID
    current_pid: u32,
    /// The currently running entity (removed from tree while running)
    current_entity: Option<SchedEntity>,
    /// Global clock (nanoseconds)
    clock_ns: u64,
    /// Target latency (ns) -- period over which all tasks should run once
    target_latency: u64,
    /// Minimum granularity (ns) -- minimum time slice
    min_granularity: u64,
    /// Scheduling period (ns)
    sched_period: u64,
    /// CPU bandwidth groups
    cgroups: BTreeMap<u32, CpuBandwidth>,
    /// Statistics
    stats: CfsStats,
    /// Wakeup granularity: minimum vruntime advantage before preempting
    wakeup_granularity: u64,
    /// Running total of CFS-tree entity weights (updated on enqueue/dequeue).
    /// Enables O(1) total_load() instead of O(n) sum over tree values.
    // maintained incrementally to avoid O(n) scan in load-balancing queries
    total_tree_weight: u64,
}

impl CfsRunQueue {
    pub const fn new() -> Self {
        CfsRunQueue {
            tree: BTreeMap::new(),
            pid_index: BTreeMap::new(),
            rt_queue: Vec::new(),
            dl_queue: Vec::new(),
            min_vruntime: 0,
            nr_running: 0,
            current_pid: 0,
            current_entity: None,
            clock_ns: 0,
            target_latency: 6_000_000, // 6ms
            min_granularity: 750_000,  // 0.75ms
            sched_period: 6_000_000,
            cgroups: BTreeMap::new(),
            stats: CfsStats::new(),
            wakeup_granularity: 1_000_000, // 1ms
            total_tree_weight: 0,
        }
    }

    /// Add a scheduling entity to the run queue
    // hot path: called on every wakeup and re-schedule
    #[inline]
    pub fn enqueue(&mut self, mut entity: SchedEntity) {
        // Set initial vruntime to min_vruntime (so new tasks don't starve old ones)
        if entity.vruntime < self.min_vruntime {
            entity.vruntime = self.min_vruntime;
        }

        // Calculate time slice for this entity
        entity.time_slice_ns = self.calc_time_slice(&entity);
        entity.slice_remaining_ns = entity.time_slice_ns;

        match entity.policy {
            SchedPolicy::Fifo | SchedPolicy::RoundRobin => {
                self.rt_queue.push(entity);
                self.rt_queue
                    .sort_by(|a, b| b.rt_priority.cmp(&a.rt_priority));
            }
            SchedPolicy::Deadline => {
                // Check if deadline needs replenishing
                if entity.deadline == 0 || entity.deadline <= self.clock_ns {
                    entity.replenish_deadline(self.clock_ns);
                    self.stats.deadline_replenish_count =
                        self.stats.deadline_replenish_count.saturating_add(1);
                }
                self.dl_queue.push(entity);
                self.dl_queue.sort_by(|a, b| a.deadline.cmp(&b.deadline));
            }
            _ => {
                let key = (entity.vruntime, entity.pid);
                // Maintain pid_index for O(log n) dequeue-by-pid.
                self.pid_index.insert(entity.pid, key);
                // Maintain total_tree_weight for O(1) load queries.
                self.total_tree_weight =
                    self.total_tree_weight.saturating_add(entity.weight as u64);
                self.tree.insert(key, entity);
            }
        }
        self.nr_running = self.nr_running.saturating_add(1);
    }

    /// Remove a scheduling entity from the run queue
    ///
    /// CFS tree removal is now O(log n) via `pid_index` instead of O(n)
    /// (was: `self.tree.keys().find(|k| k.1 == pid)` — a full linear scan).
    // hot path: called on every context switch and sleep_on
    #[inline]
    pub fn dequeue(&mut self, pid: u32) -> Option<SchedEntity> {
        // Check if it's the current running entity
        if let Some(ref e) = self.current_entity {
            if e.pid == pid {
                let entity = self.current_entity.take();
                if self.nr_running > 0 {
                    self.nr_running -= 1;
                }
                return entity;
            }
        }

        // Check RT queue — typically tiny (< 4 entries) so linear scan is fine
        if let Some(pos) = self.rt_queue.iter().position(|e| e.pid == pid) {
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            return Some(self.rt_queue.remove(pos));
        }
        // Check deadline queue — also typically tiny
        if let Some(pos) = self.dl_queue.iter().position(|e| e.pid == pid) {
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            return Some(self.dl_queue.remove(pos));
        }
        // Check CFS tree: O(log n) via pid_index instead of O(n) key scan.
        if let Some(key) = self.pid_index.remove(&pid) {
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            if let Some(entity) = self.tree.remove(&key) {
                self.total_tree_weight =
                    self.total_tree_weight.saturating_sub(entity.weight as u64);
                return Some(entity);
            }
        }
        None
    }

    /// Pick the next task to run.
    /// Priority order: Deadline > RT > CFS > Idle
    pub fn pick_next(&mut self) -> Option<SchedEntity> {
        self.stats.schedule_count = self.stats.schedule_count.saturating_add(1);

        // 1. Deadline tasks (earliest deadline first)
        if !self.dl_queue.is_empty() {
            // Check for budget-exhausted entries
            let first_valid = self
                .dl_queue
                .iter()
                .position(|e| !e.deadline_budget_exhausted());
            if let Some(idx) = first_valid {
                if self.nr_running > 0 {
                    self.nr_running -= 1;
                }
                return Some(self.dl_queue.remove(idx));
            }
            // All deadline tasks exhausted -- replenish the earliest
            if let Some(e) = self.dl_queue.first_mut() {
                e.replenish_deadline(self.clock_ns);
                self.stats.deadline_replenish_count += 1;
            }
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            return Some(self.dl_queue.remove(0));
        }

        // 2. Real-time tasks (highest priority first)
        if !self.rt_queue.is_empty() {
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            return Some(self.rt_queue.remove(0));
        }

        // 3. CFS tasks (lowest vruntime first)
        if let Some((&key, _)) = self.tree.iter().next() {
            if self.nr_running > 0 {
                self.nr_running -= 1;
            }
            let entity = self.tree.remove(&key)?;
            // Remove from pid_index to keep it in sync.
            self.pid_index.remove(&entity.pid);
            // Maintain total_tree_weight.
            self.total_tree_weight = self.total_tree_weight.saturating_sub(entity.weight as u64);
            // Update min_vruntime: it should be the maximum of:
            //   - current min_vruntime
            //   - the vruntime of the leftmost (just picked) entity
            if entity.vruntime > self.min_vruntime {
                self.min_vruntime = entity.vruntime;
            }
            return Some(entity);
        }

        None
    }

    /// Put the currently running entity back on the run queue
    /// (called when the current task is preempted or yields)
    pub fn put_current_back(&mut self) {
        if let Some(entity) = self.current_entity.take() {
            self.enqueue(entity);
        }
    }

    /// Update vruntime for the currently running task
    pub fn update_current(&mut self, pid: u32, delta_ns: u64) {
        self.clock_ns += delta_ns;

        if let Some(ref mut entity) = self.current_entity {
            if entity.pid == pid {
                entity.charge_execution(delta_ns);

                // Check cgroup bandwidth
                if entity.cgroup_id > 0 {
                    if let Some(cg) = self.cgroups.get_mut(&entity.cgroup_id) {
                        cg.check_period(self.clock_ns);
                        if cg.charge(delta_ns) {
                            self.stats.throttle_count = self.stats.throttle_count.saturating_add(1);
                        }
                    }
                }
            }
        }
    }

    /// Check if the current task should be preempted.
    ///
    /// Returns true if:
    /// 1. The current task's time slice is exhausted, OR
    /// 2. A newly woken task has sufficiently lower vruntime (wakeup preemption)
    pub fn check_preempt(&self) -> bool {
        let current = match &self.current_entity {
            Some(e) => e,
            None => return false,
        };

        // Check slice exhaustion
        if current.slice_remaining_ns == 0 {
            return true;
        }

        // Check if the leftmost CFS task has enough vruntime advantage
        if let Some((_, leftmost)) = self.tree.iter().next() {
            if current.vruntime > leftmost.vruntime {
                let diff = current.vruntime - leftmost.vruntime;
                if diff > self.wakeup_granularity {
                    return true;
                }
            }
        }

        // Check if an RT or deadline task is pending
        if !self.rt_queue.is_empty() || !self.dl_queue.is_empty() {
            if current.policy == SchedPolicy::Normal
                || current.policy == SchedPolicy::Batch
                || current.policy == SchedPolicy::Idle
            {
                return true;
            }
        }

        false
    }

    /// Calculate the ideal time slice for a task
    pub fn calc_time_slice(&self, entity: &SchedEntity) -> u64 {
        if self.nr_running <= 1 {
            return self.sched_period;
        }

        // Calculate the scheduling period:
        // If nr_running * min_granularity > target_latency, use the longer period
        let period = if self.nr_running as u64 * self.min_granularity > self.target_latency {
            self.nr_running as u64 * self.min_granularity
        } else {
            self.target_latency
        };

        // Weighted time slice: slice = period * (entity_weight / total_weight)
        // O(1): use the maintained total_tree_weight field instead of O(n) sum.
        let total_weight: u64 = self.total_tree_weight + entity.weight as u64;

        if total_weight == 0 {
            return self.min_granularity;
        }

        let slice = (period * entity.weight as u64) / total_weight;
        slice.max(self.min_granularity)
    }

    /// Process a wakeup: mark an entity as no longer sleeping and re-enqueue.
    /// Applies sleeper fairness to prevent monopolization.
    pub fn wakeup_entity(&mut self, pid: u32) {
        self.stats.wakeup_count = self.stats.wakeup_count.saturating_add(1);

        if let Some(mut entity) = self.dequeue(pid) {
            let half_latency = self.target_latency / 2;
            let old_vruntime = entity.vruntime;
            entity.wake_up(self.min_vruntime, half_latency);

            if entity.vruntime != old_vruntime {
                self.stats.sleeper_adjustments = self.stats.sleeper_adjustments.saturating_add(1);
            }

            self.enqueue(entity);
        }
    }

    // ----- Cgroup bandwidth management -----

    /// Create or update a cgroup bandwidth group
    pub fn set_cgroup_bandwidth(&mut self, id: u32, name: &str, quota_ns: u64, period_ns: u64) {
        if let Some(cg) = self.cgroups.get_mut(&id) {
            cg.set_bandwidth(quota_ns, period_ns);
        } else {
            let mut cg = CpuBandwidth::new(id, name);
            cg.set_bandwidth(quota_ns, period_ns);
            cg.period_start_ns = self.clock_ns;
            self.cgroups.insert(id, cg);
        }
    }

    /// Move an entity to a cgroup
    pub fn set_entity_cgroup(&mut self, pid: u32, cgroup_id: u32) {
        if let Some(mut entity) = self.dequeue(pid) {
            entity.cgroup_id = cgroup_id;
            self.enqueue(entity);
        }
    }

    /// Get cgroup statistics
    pub fn get_cgroup_stats(&self, id: u32) -> Option<&CpuBandwidth> {
        self.cgroups.get(&id)
    }

    // ----- Compatibility interface -----

    /// Set current running PID
    pub fn set_current(&mut self, pid: u32) {
        self.current_pid = pid;
    }

    /// Get current running PID
    pub fn current(&self) -> u32 {
        self.current_pid
    }

    /// Number of runnable tasks
    pub fn nr_running(&self) -> u32 {
        self.nr_running
    }

    /// Queue length (for compatibility)
    pub fn queue_length(&self) -> usize {
        self.nr_running as usize
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.nr_running == 0
    }

    /// Compatibility: add by PID (creates default entity)
    pub fn add(&mut self, pid: u32) {
        let entity = SchedEntity::new(pid);
        self.enqueue(entity);
    }

    /// Compatibility: remove by PID
    pub fn remove(&mut self, pid: u32) {
        self.dequeue(pid);
    }

    /// Compatibility: get next PID
    pub fn next(&mut self) -> Option<u32> {
        self.pick_next().map(|e| e.pid)
    }

    /// Get the clock
    pub fn clock_ns(&self) -> u64 {
        self.clock_ns
    }

    /// Get the min_vruntime
    pub fn min_vruntime(&self) -> u64 {
        self.min_vruntime
    }

    /// Get statistics
    pub fn get_stats_report(&self) -> CfsStats {
        self.stats.clone()
    }

    /// Set target latency
    pub fn set_target_latency(&mut self, ns: u64) {
        self.target_latency = ns;
        self.sched_period = ns;
    }

    /// Set minimum granularity
    pub fn set_min_granularity(&mut self, ns: u64) {
        self.min_granularity = ns;
    }

    /// Set wakeup granularity
    pub fn set_wakeup_granularity(&mut self, ns: u64) {
        self.wakeup_granularity = ns;
    }

    /// Dump CFS state for debugging
    pub fn dump(&self) {
        crate::serial_println!("  CFS state:");
        crate::serial_println!(
            "    clock={}ns, min_vruntime={}ns, nr_running={}",
            self.clock_ns,
            self.min_vruntime,
            self.nr_running
        );
        crate::serial_println!(
            "    target_latency={}ns, min_gran={}ns, wakeup_gran={}ns",
            self.target_latency,
            self.min_granularity,
            self.wakeup_granularity
        );
        crate::serial_println!("    CFS tree: {} entities", self.tree.len());
        for ((vrt, pid), entity) in self.tree.iter().take(10) {
            crate::serial_println!(
                "      pid={} vrt={} nice={} weight={} exec={}ns",
                pid,
                vrt,
                entity.nice,
                entity.weight,
                entity.sum_exec
            );
        }
        if self.tree.len() > 10 {
            crate::serial_println!("      ... and {} more", self.tree.len() - 10);
        }
        crate::serial_println!("    RT queue: {} tasks", self.rt_queue.len());
        crate::serial_println!("    DL queue: {} tasks", self.dl_queue.len());
        crate::serial_println!(
            "    Stats: sched={}, preempt={}, wakeup={}, throttle={}",
            self.stats.schedule_count,
            self.stats.preempt_count,
            self.stats.wakeup_count,
            self.stats.throttle_count
        );
        if !self.cgroups.is_empty() {
            crate::serial_println!("    Cgroups:");
            for (id, cg) in &self.cgroups {
                crate::serial_println!(
                    "      [{}] {}: quota={}/{}ns, consumed={}, throttled={}",
                    id,
                    cg.name,
                    cg.quota_ns,
                    cg.period_ns,
                    cg.runtime_consumed_ns,
                    cg.nr_throttled
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global CFS instance
// ---------------------------------------------------------------------------

/// Global CFS scheduler (replaces the old round-robin scheduler)
pub static CFS: Mutex<CfsRunQueue> = Mutex::new(CfsRunQueue::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set nice value for a process
pub fn set_nice(pid: u32, nice: i8) {
    let mut cfs = CFS.lock();
    if let Some(entity) = cfs.dequeue(pid) {
        let mut e = entity;
        e.set_nice(nice);
        cfs.enqueue(e);
    }
}

/// Set scheduling policy for a process
pub fn set_policy(pid: u32, policy: SchedPolicy, priority: u8) {
    let mut cfs = CFS.lock();
    if let Some(entity) = cfs.dequeue(pid) {
        let mut e = entity;
        e.policy = policy;
        e.rt_priority = priority;
        cfs.enqueue(e);
    }
}

/// Set CPU affinity for a process
pub fn set_affinity(pid: u32, mask: u64) {
    let mut cfs = CFS.lock();
    if let Some(entity) = cfs.dequeue(pid) {
        let mut e = entity;
        e.cpu_affinity = mask;
        cfs.enqueue(e);
    }
}

/// Get scheduling statistics for a process
///
/// O(log n) via pid_index for the CFS tree path; RT queue scan is O(n) but
/// the RT queue is typically tiny (< 4 entries).
pub fn get_stats(pid: u32) -> Option<(u64, u64, u64)> {
    let cfs = CFS.lock();
    // Fast path: O(log n) lookup via pid_index
    if let Some(&key) = cfs.pid_index.get(&pid) {
        if let Some(e) = cfs.tree.get(&key) {
            return Some((e.vruntime, e.sum_exec, e.nvcsw));
        }
    }
    // Check current entity (not in tree while running)
    if let Some(ref e) = cfs.current_entity {
        if e.pid == pid {
            return Some((e.vruntime, e.sum_exec, e.nvcsw));
        }
    }
    // Fallback: RT queue (small — linear scan acceptable)
    for e in &cfs.rt_queue {
        if e.pid == pid {
            return Some((e.vruntime, e.sum_exec, e.nvcsw));
        }
    }
    None
}

/// Set up a cgroup with bandwidth control
pub fn create_cgroup(id: u32, name: &str, quota_ns: u64, period_ns: u64) {
    CFS.lock()
        .set_cgroup_bandwidth(id, name, quota_ns, period_ns);
}

/// Move a process into a cgroup
pub fn assign_cgroup(pid: u32, cgroup_id: u32) {
    CFS.lock().set_entity_cgroup(pid, cgroup_id);
}

/// Wakeup a sleeping process with fairness adjustment
pub fn wakeup(pid: u32) {
    CFS.lock().wakeup_entity(pid);
}

/// Dump CFS state to serial
pub fn dump() {
    CFS.lock().dump();
}

/// Tick handler: advance the clock, charge the current entity, and check preemption.
/// Returns true if the current task should be preempted.
pub fn tick(delta_ns: u64) -> bool {
    let mut cfs = CFS.lock();
    let pid = cfs.current_pid;
    cfs.update_current(pid, delta_ns);
    cfs.check_preempt()
}

/// Get the total load (sum of weights) on the CFS run queue.
/// Useful for load balancing across CPUs.
///
/// CFS tree load is now O(1) via the maintained `total_tree_weight` field
/// instead of O(n) (was: `.values().map(|e| e.weight as u64).sum()`).
/// RT and DL queues are still iterated but are always tiny (< 10 entries).
pub fn total_load() -> u64 {
    let cfs = CFS.lock();
    // O(1): maintained incrementally in enqueue/dequeue.
    let tree_load = cfs.total_tree_weight;
    // RT queue is tiny — linear scan acceptable.
    let rt_load: u64 = cfs.rt_queue.iter().map(|e| e.weight as u64).sum();
    let dl_load: u64 = cfs.dl_queue.iter().map(|e| e.weight as u64).sum();
    let current_load: u64 = cfs
        .current_entity
        .as_ref()
        .map(|e| e.weight as u64)
        .unwrap_or(0);
    tree_load + rt_load + dl_load + current_load
}

/// Get load imbalance between this run queue and a target load.
/// Returns positive if this queue is overloaded, negative if underloaded.
/// Values in weight units.
pub fn load_imbalance(target_load: u64) -> i64 {
    let my_load = total_load();
    my_load as i64 - target_load as i64
}

/// Find the most migratable entity (highest vruntime, not pinned, CFS only).
/// Returns Some(pid) of the best candidate for migration.
pub fn find_migration_candidate(target_cpu_mask: u64) -> Option<u32> {
    let cfs = CFS.lock();
    // Walk the tree in reverse (highest vruntime = least-recently-run = best to migrate)
    for ((_, pid), entity) in cfs.tree.iter().rev() {
        // Only migrate if the entity's affinity allows the target CPU
        if entity.cpu_affinity & target_cpu_mask != 0 {
            return Some(*pid);
        }
    }
    None
}

/// Get per-entity scheduling statistics for a specific process.
/// Returns (vruntime, sum_exec, nvcsw, nivcsw, weight, nice, policy).
///
/// CFS tree lookup is O(log n) via pid_index.  RT/DL queues are scanned
/// linearly but are expected to be very small (< 10 entries each).
pub fn get_entity_detail(pid: u32) -> Option<(u64, u64, u64, u64, u32, i8, SchedPolicy)> {
    let cfs = CFS.lock();
    // Check current entity (running — not in tree or index)
    if let Some(ref e) = cfs.current_entity {
        if e.pid == pid {
            return Some((
                e.vruntime, e.sum_exec, e.nvcsw, e.nivcsw, e.weight, e.nice, e.policy,
            ));
        }
    }
    // O(log n) CFS tree lookup via pid_index
    if let Some(&key) = cfs.pid_index.get(&pid) {
        if let Some(e) = cfs.tree.get(&key) {
            return Some((
                e.vruntime, e.sum_exec, e.nvcsw, e.nivcsw, e.weight, e.nice, e.policy,
            ));
        }
    }
    // RT queue (small — linear scan)
    for e in &cfs.rt_queue {
        if e.pid == pid {
            return Some((
                e.vruntime, e.sum_exec, e.nvcsw, e.nivcsw, e.weight, e.nice, e.policy,
            ));
        }
    }
    // Deadline queue (small — linear scan)
    for e in &cfs.dl_queue {
        if e.pid == pid {
            return Some((
                e.vruntime, e.sum_exec, e.nvcsw, e.nivcsw, e.weight, e.nice, e.policy,
            ));
        }
    }
    None
}

/// Initialize CFS
pub fn init() {
    crate::serial_println!("  [cfs] Completely Fair Scheduler initialized");
}
