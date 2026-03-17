/// Multi-Level Feedback Queue (MLFQ) scheduler for Genesis
///
/// Replaces the simple round-robin with a priority-based MLFQ scheduler.
/// Features:
///   - 4 priority levels (0=highest, 3=lowest) with decreasing time quanta
///   - Priority boost: periodic reset to prevent starvation
///   - Real-time priority band (FIFO and round-robin policies)
///   - Idle task selection when all queues empty
///   - Per-CPU ready queue concept (for SMP preparation)
///   - CPU usage tracking per process (tick accounting)
///   - Load average calculation (1-min, 5-min, 15-min using fixed-point)
///   - Scheduler statistics (context switches, migrations, runnable count)
///
/// Inspired by: Linux O(1) scheduler, FreeBSD ULE, OSTEP MLFQ chapter.
/// All code is original.
use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of MLFQ priority levels for normal tasks
pub const NUM_PRIORITY_LEVELS: usize = 4;

/// Time quantum per priority level (in ticks). Higher priority = shorter quantum.
/// Level 0: 2 ticks (interactive), Level 1: 4 ticks, Level 2: 8 ticks, Level 3: 16 ticks
const TIME_QUANTA: [u32; NUM_PRIORITY_LEVELS] = [2, 4, 8, 16];

/// Ticks between priority boosts (prevents starvation).
/// Every BOOST_INTERVAL ticks, all processes are moved to the highest priority.
const BOOST_INTERVAL: u64 = 200;

/// Maximum number of CPUs (for SMP preparation)
pub const MAX_CPUS: usize = 8;

/// Fixed-point shift for load average calculations (12 bits of fraction)
const LOAD_AVG_SHIFT: u32 = 12;

/// Decay factors for load average (fixed-point, multiplied by 1<<LOAD_AVG_SHIFT)
/// exp(-5/60) * 4096  for 1-min average (tick every 5 seconds)
/// exp(-5/300) * 4096 for 5-min average
/// exp(-5/900) * 4096 for 15-min average
const LOAD_AVG_DECAY_1: u64 = 3756; // ~0.9170 * 4096
const LOAD_AVG_DECAY_5: u64 = 4028; // ~0.9835 * 4096
const LOAD_AVG_DECAY_15: u64 = 4083; // ~0.9945 * 4096
const LOAD_AVG_SCALE: u64 = 1 << LOAD_AVG_SHIFT; // 4096

// ---------------------------------------------------------------------------
// Per-process scheduling state tracked by the MLFQ
// ---------------------------------------------------------------------------

/// MLFQ-tracked state per process
#[derive(Debug, Clone)]
pub struct MlfqEntry {
    /// Process ID
    pub pid: u32,
    /// Current MLFQ priority level (0=highest, NUM_PRIORITY_LEVELS-1=lowest)
    pub level: usize,
    /// Remaining ticks in the current time quantum
    pub ticks_remaining: u32,
    /// Total ticks consumed at the current level (for demotion decisions)
    pub ticks_at_level: u32,
    /// Total CPU ticks ever consumed by this process
    pub total_ticks: u64,
    /// Whether this is a real-time (FIFO/RR) task
    pub is_realtime: bool,
    /// Real-time priority (higher = more important, 1-99)
    pub rt_priority: u8,
    /// Real-time policy: 0=FIFO, 1=RR
    pub rt_policy: u8,
    /// Tick when last scheduled
    pub last_run_tick: u64,
    /// Tick when added to the run queue
    pub enqueue_tick: u64,
    /// Number of times this entry has been boosted
    pub boost_count: u64,
}

impl MlfqEntry {
    pub fn new(pid: u32) -> Self {
        MlfqEntry {
            pid,
            level: 0,
            ticks_remaining: TIME_QUANTA[0],
            ticks_at_level: 0,
            total_ticks: 0,
            is_realtime: false,
            rt_priority: 0,
            rt_policy: 0,
            last_run_tick: 0,
            enqueue_tick: 0,
            boost_count: 0,
        }
    }

    /// Create a real-time entry
    pub fn new_realtime(pid: u32, priority: u8, policy: u8) -> Self {
        MlfqEntry {
            pid,
            level: 0,
            ticks_remaining: TIME_QUANTA[0],
            ticks_at_level: 0,
            total_ticks: 0,
            is_realtime: true,
            rt_priority: priority,
            rt_policy: policy,
            last_run_tick: 0,
            enqueue_tick: 0,
            boost_count: 0,
        }
    }

    /// Consume a tick. Returns true if quantum is exhausted.
    pub fn tick(&mut self) -> bool {
        self.total_ticks = self.total_ticks.saturating_add(1);
        self.ticks_at_level += 1;
        if self.ticks_remaining > 0 {
            self.ticks_remaining -= 1;
        }
        self.ticks_remaining == 0
    }

    /// Demote to the next lower priority level
    pub fn demote(&mut self) {
        if self.level < NUM_PRIORITY_LEVELS - 1 {
            self.level += 1;
        }
        self.ticks_remaining = TIME_QUANTA[self.level];
        self.ticks_at_level = 0;
    }

    /// Boost to the highest priority level
    pub fn boost(&mut self) {
        self.level = 0;
        self.ticks_remaining = TIME_QUANTA[0];
        self.ticks_at_level = 0;
        self.boost_count = self.boost_count.saturating_add(1);
    }

    /// Reset quantum for the current level (e.g., after blocking then waking)
    pub fn reset_quantum(&mut self) {
        self.ticks_remaining = TIME_QUANTA[self.level];
    }
}

// ---------------------------------------------------------------------------
// Scheduler statistics
// ---------------------------------------------------------------------------

/// Global scheduler statistics
#[derive(Debug, Clone)]
pub struct SchedulerStats {
    /// Total number of context switches performed
    pub context_switches: u64,
    /// Total number of voluntary context switches (process yielded/blocked)
    pub voluntary_switches: u64,
    /// Total number of involuntary context switches (preempted)
    pub involuntary_switches: u64,
    /// Total number of priority boosts performed
    pub total_boosts: u64,
    /// Total number of demotions
    pub total_demotions: u64,
    /// Current number of runnable processes
    pub nr_runnable: u32,
    /// Peak runnable count
    pub peak_runnable: u32,
    /// Total scheduler invocations (pick_next calls)
    pub schedule_count: u64,
    /// Number of times the idle task was selected
    pub idle_selections: u64,
    /// Current tick counter
    pub current_tick: u64,
    /// 1-minute load average (fixed-point, shift LOAD_AVG_SHIFT)
    pub load_avg_1: u64,
    /// 5-minute load average (fixed-point)
    pub load_avg_5: u64,
    /// 15-minute load average (fixed-point)
    pub load_avg_15: u64,
    /// Tick of last load average update
    pub load_avg_last_update: u64,
}

impl SchedulerStats {
    pub const fn new() -> Self {
        SchedulerStats {
            context_switches: 0,
            voluntary_switches: 0,
            involuntary_switches: 0,
            total_boosts: 0,
            total_demotions: 0,
            nr_runnable: 0,
            peak_runnable: 0,
            schedule_count: 0,
            idle_selections: 0,
            current_tick: 0,
            load_avg_1: 0,
            load_avg_5: 0,
            load_avg_15: 0,
            load_avg_last_update: 0,
        }
    }

    /// Get load averages as integer parts and fractions (for display).
    /// Returns (int_part, frac_hundredths) for each average.
    pub fn load_averages(&self) -> [(u64, u64); 3] {
        let extract = |val: u64| {
            let int_part = val >> LOAD_AVG_SHIFT;
            let frac = val & (LOAD_AVG_SCALE - 1);
            let frac_hundredths = (frac * 100) >> LOAD_AVG_SHIFT;
            (int_part, frac_hundredths)
        };
        [
            extract(self.load_avg_1),
            extract(self.load_avg_5),
            extract(self.load_avg_15),
        ]
    }
}

// ---------------------------------------------------------------------------
// Per-CPU run queue (for SMP preparation)
// ---------------------------------------------------------------------------

/// Per-CPU scheduler state
pub struct CpuRunQueue {
    /// CPU identifier
    pub cpu_id: u32,
    /// Whether this CPU is online
    pub online: bool,
    /// The currently running PID on this CPU
    pub current_pid: u32,
    /// Real-time run queue (sorted by priority, highest first)
    pub rt_queue: VecDeque<MlfqEntry>,
    /// MLFQ priority queues (index 0 = highest priority)
    pub mlfq_queues: [VecDeque<u32>; NUM_PRIORITY_LEVELS],
    /// Map from PID to MLFQ entry data
    pub entries: alloc::collections::BTreeMap<u32, MlfqEntry>,
    /// Idle flag: true if this CPU is running the idle task
    pub idle: bool,
    /// Tick counter for this CPU
    pub local_tick: u64,
    /// Tick of last priority boost on this CPU
    pub last_boost_tick: u64,
}

impl CpuRunQueue {
    pub const fn new(cpu_id: u32) -> Self {
        CpuRunQueue {
            cpu_id,
            online: false,
            current_pid: 0,
            rt_queue: VecDeque::new(),
            mlfq_queues: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            entries: alloc::collections::BTreeMap::new(),
            idle: true,
            local_tick: 0,
            last_boost_tick: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// The MLFQ Scheduler
// ---------------------------------------------------------------------------

/// The main scheduler state
pub struct Scheduler {
    /// Currently running process PID
    current_pid: u32,

    /// Real-time run queue (FIFO and RR tasks, sorted by priority)
    rt_queue: VecDeque<MlfqEntry>,

    /// MLFQ priority queues for normal tasks
    /// Index 0 = highest priority, index 3 = lowest
    mlfq_queues: [VecDeque<u32>; NUM_PRIORITY_LEVELS],

    /// Per-PID tracking data
    entries: alloc::collections::BTreeMap<u32, MlfqEntry>,

    /// Scheduler statistics
    stats: SchedulerStats,

    /// Tick of last priority boost
    last_boost_tick: u64,

    /// The currently running entry (removed from queues while running)
    current_entry: Option<MlfqEntry>,

    /// Legacy FIFO run queue (for backward compatibility with simple add/next)
    run_queue: VecDeque<u32>,
}

impl Scheduler {
    pub const fn new() -> Self {
        Scheduler {
            current_pid: 0,
            rt_queue: VecDeque::new(),
            mlfq_queues: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            entries: alloc::collections::BTreeMap::new(),
            stats: SchedulerStats::new(),
            last_boost_tick: 0,
            current_entry: None,
            run_queue: VecDeque::new(),
        }
    }

    // ----- basic interface (backward compatible) -----

    /// Set the currently running process
    pub fn set_current(&mut self, pid: u32) {
        self.current_pid = pid;
    }

    /// Get the currently running process PID
    pub fn current(&self) -> u32 {
        self.current_pid
    }

    /// Add a process to the run queue (backward compatible, uses MLFQ level 0)
    pub fn add(&mut self, pid: u32) {
        // Don't add duplicates
        if self.entries.contains_key(&pid) {
            return;
        }
        if self.run_queue.contains(&pid) {
            return;
        }

        let entry = MlfqEntry::new(pid);
        let level = entry.level;
        self.entries.insert(pid, entry);
        self.mlfq_queues[level].push_back(pid);
        self.run_queue.push_back(pid);
        self.stats.nr_runnable = self.stats.nr_runnable.saturating_add(1);
        if self.stats.nr_runnable > self.stats.peak_runnable {
            self.stats.peak_runnable = self.stats.nr_runnable;
        }
    }

    /// Remove a process from the run queue
    pub fn remove(&mut self, pid: u32) {
        // Remove from MLFQ queues
        for queue in self.mlfq_queues.iter_mut() {
            queue.retain(|&p| p != pid);
        }
        // Remove from RT queue
        self.rt_queue.retain(|e| e.pid != pid);
        // Remove from legacy queue
        self.run_queue.retain(|&p| p != pid);
        // Remove entry
        self.entries.remove(&pid);
        // Update runnable count
        if self.stats.nr_runnable > 0 {
            self.stats.nr_runnable -= 1;
        }
    }

    /// Get the next process to run (removes from front of queue)
    /// Uses MLFQ priority: RT first, then level 0, 1, 2, 3
    pub fn next(&mut self) -> Option<u32> {
        self.stats.schedule_count = self.stats.schedule_count.saturating_add(1);

        // 1. Check real-time queue (highest priority first)
        if let Some(rt_entry) = self.rt_queue.pop_front() {
            let pid = rt_entry.pid;
            self.current_entry = Some(rt_entry);
            self.stats.context_switches += 1;
            return Some(pid);
        }

        // 2. Check MLFQ queues from highest to lowest priority
        for level in 0..NUM_PRIORITY_LEVELS {
            if let Some(pid) = self.mlfq_queues[level].pop_front() {
                // Also remove from legacy run queue
                self.run_queue.retain(|&p| p != pid);
                self.stats.context_switches = self.stats.context_switches.saturating_add(1);
                return Some(pid);
            }
        }

        // 3. Fall back to legacy round-robin queue
        if let Some(pid) = self.run_queue.pop_front() {
            self.stats.context_switches += 1;
            return Some(pid);
        }

        // 4. Nothing to run -- idle
        self.stats.idle_selections = self.stats.idle_selections.saturating_add(1);
        None
    }

    /// Number of processes waiting to run
    pub fn queue_length(&self) -> usize {
        self.stats.nr_runnable as usize
    }

    /// Check if the run queue is empty
    pub fn is_empty(&self) -> bool {
        self.stats.nr_runnable == 0
    }

    // ----- MLFQ-specific interface -----

    /// Add a process with a specific MLFQ priority level
    pub fn add_at_level(&mut self, pid: u32, level: usize) {
        if self.entries.contains_key(&pid) {
            return;
        }

        let clamped_level = if level >= NUM_PRIORITY_LEVELS {
            NUM_PRIORITY_LEVELS - 1
        } else {
            level
        };

        let mut entry = MlfqEntry::new(pid);
        entry.level = clamped_level;
        entry.ticks_remaining = TIME_QUANTA[clamped_level];
        entry.enqueue_tick = self.stats.current_tick;

        self.entries.insert(pid, entry);
        self.mlfq_queues[clamped_level].push_back(pid);
        self.run_queue.push_back(pid);
        self.stats.nr_runnable = self.stats.nr_runnable.saturating_add(1);
        if self.stats.nr_runnable > self.stats.peak_runnable {
            self.stats.peak_runnable = self.stats.nr_runnable;
        }
    }

    /// Add a real-time process
    pub fn add_realtime(&mut self, pid: u32, priority: u8, policy: u8) {
        // Remove if already present
        self.remove(pid);

        let entry = MlfqEntry::new_realtime(pid, priority, policy);
        // Insert sorted by priority (highest first)
        let pos = self
            .rt_queue
            .iter()
            .position(|e| e.rt_priority < priority)
            .unwrap_or(self.rt_queue.len());
        self.rt_queue.insert(pos, entry);
        self.stats.nr_runnable = self.stats.nr_runnable.saturating_add(1);
        if self.stats.nr_runnable > self.stats.peak_runnable {
            self.stats.peak_runnable = self.stats.nr_runnable;
        }
    }

    /// Called on each timer tick for the currently running process.
    /// Returns true if the process should be preempted (quantum exhausted).
    pub fn tick(&mut self, pid: u32) -> bool {
        self.stats.current_tick = self.stats.current_tick.saturating_add(1);

        // Check if we need to do a priority boost
        if self.stats.current_tick - self.last_boost_tick >= BOOST_INTERVAL {
            self.priority_boost();
        }

        // Update the running process's tracking
        if let Some(entry) = self.entries.get_mut(&pid) {
            let exhausted = entry.tick();
            entry.last_run_tick = self.stats.current_tick;

            if exhausted {
                // Quantum used up: demote and preempt
                entry.demote();
                self.stats.total_demotions = self.stats.total_demotions.saturating_add(1);
                self.stats.involuntary_switches = self.stats.involuntary_switches.saturating_add(1);
                return true;
            }
        }

        // Also check if a higher-priority task has appeared
        // (e.g., a just-woken RT task should preempt a normal task)
        if !self.rt_queue.is_empty() {
            if let Some(entry) = self.entries.get(&pid) {
                if !entry.is_realtime {
                    return true; // preempt normal task for RT task
                }
            }
        }

        false
    }

    /// Perform a priority boost: move all normal tasks to level 0
    pub fn priority_boost(&mut self) {
        self.last_boost_tick = self.stats.current_tick;
        self.stats.total_boosts = self.stats.total_boosts.saturating_add(1);

        // Collect all PIDs from lower-priority queues
        let mut all_pids = Vec::new();
        for level in 1..NUM_PRIORITY_LEVELS {
            while let Some(pid) = self.mlfq_queues[level].pop_front() {
                all_pids.push(pid);
            }
        }

        // Move them all to level 0
        for pid in all_pids {
            if let Some(entry) = self.entries.get_mut(&pid) {
                entry.boost();
            }
            self.mlfq_queues[0].push_back(pid);
        }
    }

    /// Record a voluntary context switch (process yielded or blocked)
    pub fn record_voluntary_switch(&mut self) {
        self.stats.voluntary_switches = self.stats.voluntary_switches.saturating_add(1);
        self.stats.context_switches += 1;
    }

    /// When a process wakes from sleep, give it a fresh quantum at its
    /// current priority (don't demote sleeping processes).
    pub fn wake_up(&mut self, pid: u32) {
        if let Some(entry) = self.entries.get_mut(&pid) {
            entry.reset_quantum();
            let level = entry.level;
            // Add back to the appropriate queue if not already there
            if !self.mlfq_queues[level].contains(&pid) {
                self.mlfq_queues[level].push_back(pid);
                if !self.run_queue.contains(&pid) {
                    self.run_queue.push_back(pid);
                }
                self.stats.nr_runnable = self.stats.nr_runnable.saturating_add(1);
                if self.stats.nr_runnable > self.stats.peak_runnable {
                    self.stats.peak_runnable = self.stats.nr_runnable;
                }
            }
        } else {
            // No entry -- add fresh
            self.add(pid);
        }
    }

    /// Get the time quantum for a process at a given priority level
    pub fn quantum_for_level(level: usize) -> u32 {
        if level < NUM_PRIORITY_LEVELS {
            TIME_QUANTA[level]
        } else {
            TIME_QUANTA[NUM_PRIORITY_LEVELS - 1]
        }
    }

    /// Update load averages. Should be called every ~5 seconds (50 ticks at 10Hz).
    ///
    /// Uses exponentially-weighted moving average:
    ///   load_avg = decay * old_load_avg + (1 - decay) * nr_runnable
    ///
    /// All arithmetic is fixed-point with LOAD_AVG_SHIFT bits of fraction.
    pub fn update_load_average(&mut self) {
        let nr = self.stats.nr_runnable as u64;
        let nr_scaled = nr << LOAD_AVG_SHIFT;

        // load_avg_1 = decay * load_avg_1 + (1-decay) * nr_running
        self.stats.load_avg_1 = (LOAD_AVG_DECAY_1 * self.stats.load_avg_1
            + (LOAD_AVG_SCALE - LOAD_AVG_DECAY_1) * nr_scaled)
            / LOAD_AVG_SCALE;

        self.stats.load_avg_5 = (LOAD_AVG_DECAY_5 * self.stats.load_avg_5
            + (LOAD_AVG_SCALE - LOAD_AVG_DECAY_5) * nr_scaled)
            / LOAD_AVG_SCALE;

        self.stats.load_avg_15 = (LOAD_AVG_DECAY_15 * self.stats.load_avg_15
            + (LOAD_AVG_SCALE - LOAD_AVG_DECAY_15) * nr_scaled)
            / LOAD_AVG_SCALE;

        self.stats.load_avg_last_update = self.stats.current_tick;
    }

    /// Get a copy of the scheduler statistics
    pub fn get_stats(&self) -> SchedulerStats {
        self.stats.clone()
    }

    /// Get the MLFQ level of a process
    pub fn get_level(&self, pid: u32) -> Option<usize> {
        self.entries.get(&pid).map(|e| e.level)
    }

    /// Get queue lengths per level
    pub fn queue_lengths(&self) -> [usize; NUM_PRIORITY_LEVELS] {
        [
            self.mlfq_queues[0].len(),
            self.mlfq_queues[1].len(),
            self.mlfq_queues[2].len(),
            self.mlfq_queues[3].len(),
        ]
    }

    /// Get the number of real-time tasks
    pub fn rt_count(&self) -> usize {
        self.rt_queue.len()
    }

    /// Dump scheduler state for debugging
    pub fn dump(&self) {
        crate::serial_println!("  Scheduler state:");
        crate::serial_println!("    Current PID: {}", self.current_pid);
        crate::serial_println!("    Runnable: {}", self.stats.nr_runnable);
        crate::serial_println!("    RT queue: {} tasks", self.rt_queue.len());
        for (i, q) in self.mlfq_queues.iter().enumerate() {
            crate::serial_println!(
                "    MLFQ level {} (quantum={}): {} tasks",
                i,
                TIME_QUANTA[i],
                q.len()
            );
        }
        let la = self.stats.load_averages();
        crate::serial_println!(
            "    Load avg: {}.{:02}, {}.{:02}, {}.{:02}",
            la[0].0,
            la[0].1,
            la[1].0,
            la[1].1,
            la[2].0,
            la[2].1
        );
        crate::serial_println!(
            "    Context switches: {} (vol: {}, invol: {})",
            self.stats.context_switches,
            self.stats.voluntary_switches,
            self.stats.involuntary_switches
        );
        crate::serial_println!(
            "    Boosts: {}, Demotions: {}",
            self.stats.total_boosts,
            self.stats.total_demotions
        );
        crate::serial_println!("    Tick: {}", self.stats.current_tick);
    }
}

// ---------------------------------------------------------------------------
// Global scheduler instance
// ---------------------------------------------------------------------------

/// Global scheduler instance
pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

// ---------------------------------------------------------------------------
// Convenience functions
// ---------------------------------------------------------------------------

/// Add a process to the scheduler with a specific priority level
pub fn add_at_level(pid: u32, level: usize) {
    SCHEDULER.lock().add_at_level(pid, level);
}

/// Add a real-time process to the scheduler
pub fn add_realtime(pid: u32, priority: u8, policy: u8) {
    SCHEDULER.lock().add_realtime(pid, priority, policy);
}

/// Process a timer tick and return true if the current process should be preempted
pub fn timer_tick() -> bool {
    let mut sched = SCHEDULER.lock();
    let pid = sched.current();
    sched.tick(pid)
}

/// Update load averages (call every ~5 seconds)
pub fn update_load_average() {
    SCHEDULER.lock().update_load_average();
}

/// Get scheduler statistics
pub fn get_stats() -> SchedulerStats {
    SCHEDULER.lock().get_stats()
}

/// Get queue lengths per MLFQ level
pub fn queue_lengths() -> [usize; NUM_PRIORITY_LEVELS] {
    SCHEDULER.lock().queue_lengths()
}

/// Dump scheduler state to serial
pub fn dump() {
    SCHEDULER.lock().dump();
}

/// Wake a process and add it back to the run queue with a fresh quantum
pub fn wake_up(pid: u32) {
    SCHEDULER.lock().wake_up(pid);
}
