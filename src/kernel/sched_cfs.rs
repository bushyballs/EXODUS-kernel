use crate::sync::Mutex;
/// Completely Fair Scheduler (CFS)
///
/// Implements a fair queuing scheduler using vruntime tracking and a fixed-size
/// priority array. Tasks are selected based on their virtual runtime, ensuring
/// each task gets CPU time proportional to its weight (determined by nice level).
///
/// No heap; uses fixed-size static array for max 256 tasks.
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const CFS_MAX_TASKS: usize = 256;
const WEIGHT_TABLE_SIZE: usize = 40; // nice -20 to +19

/// CFS task structure (must be Copy and have const fn empty())
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CfsTask {
    pub pid: u32,
    pub vruntime: u64,       // Virtual runtime in nanoseconds
    pub weight: u32,         // Weight derived from nice level
    pub nice: i8,            // Nice level (-20 to +19)
    pub cpu_affinity: u64,   // CPU affinity bitmask
    pub runtime_ns: u64,     // Actual runtime in nanoseconds
    pub sleep_start_ns: u64, // When task went to sleep
    pub active: bool,        // Is task in the run queue
}

impl CfsTask {
    const fn empty() -> Self {
        CfsTask {
            pid: 0,
            vruntime: 0,
            weight: 0,
            nice: 0,
            cpu_affinity: 0,
            runtime_ns: 0,
            sleep_start_ns: 0,
            active: false,
        }
    }
}

/// Global CFS state
static CFS_TASKS: Mutex<[CfsTask; CFS_MAX_TASKS]> = Mutex::new([CfsTask::empty(); CFS_MAX_TASKS]);
static CFS_CURRENT_PID: AtomicU32 = AtomicU32::new(0);
static CFS_MIN_VRUNTIME: AtomicU64 = AtomicU64::new(0);

/// Linux sched_prio_to_weight lookup table
/// Maps nice levels -20..19 to relative weights
const NICE_TO_WEIGHT: [u32; 40] = [
    88761, 71755, 56483, 46273, 36291, // -20 to -16
    29154, 23254, 18705, 14949, 11916, // -15 to -11
    9548, 7620, 6100, 4904, 3906, // -10 to -6
    3121, 2501, 1991, 1586, 1277, // -5 to -1
    1024, 820, 655, 526, 423, // 0 to 4
    335, 272, 215, 172, 137, // 5 to 9
    110, 87, 70, 56, 45, // 10 to 14
    36, 29, 23, 18, 15, // 15 to 19
];

/// Convert nice level to weight using lookup table
pub fn nice_to_weight(nice: i8) -> u32 {
    // nice ranges from -20 to +19
    if nice < -20 || nice > 19 {
        return 1024; // Default weight for invalid nice
    }
    let idx = (nice + 20) as usize;
    if idx < NICE_TO_WEIGHT.len() {
        NICE_TO_WEIGHT[idx]
    } else {
        1024 // Fallback
    }
}

/// Enqueue a task into the CFS run queue
pub fn cfs_task_enqueue(pid: u32, nice: i8) -> bool {
    if pid == 0 {
        return false; // Invalid PID
    }

    let weight = nice_to_weight(nice);
    let mut tasks = CFS_TASKS.lock();

    // Find a free slot
    for i in 0..CFS_MAX_TASKS {
        if !tasks[i].active {
            let min_vruntime = CFS_MIN_VRUNTIME.load(Ordering::Relaxed);
            tasks[i] = CfsTask {
                pid,
                vruntime: min_vruntime,
                weight,
                nice,
                cpu_affinity: 0,
                runtime_ns: 0,
                sleep_start_ns: 0,
                active: true,
            };
            return true;
        }
    }

    false // No free slots
}

/// Remove a task from the CFS run queue
pub fn cfs_task_dequeue(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let mut tasks = CFS_TASKS.lock();
    for i in 0..CFS_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            tasks[i].active = false;
            return true;
        }
    }
    false
}

/// Pick the next task to run (minimum vruntime)
pub fn cfs_pick_next() -> Option<u32> {
    let tasks = CFS_TASKS.lock();

    let mut min_vruntime = u64::MAX;
    let mut selected_pid = None;

    for i in 0..CFS_MAX_TASKS {
        if tasks[i].active && tasks[i].vruntime < min_vruntime {
            min_vruntime = tasks[i].vruntime;
            selected_pid = Some(tasks[i].pid);
        }
    }

    selected_pid
}

/// Update virtual runtime for a task
/// vruntime += delta_ns * 1024 / weight
pub fn cfs_update_vruntime(pid: u32, delta_ns: u64) -> bool {
    if pid == 0 || delta_ns == 0 {
        return false;
    }

    let mut tasks = CFS_TASKS.lock();
    for i in 0..CFS_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            let weight = tasks[i].weight;
            if weight == 0 {
                return true; // Skip if weight is 0
            }

            // vruntime += delta_ns * 1024 / weight
            // Using integer division to avoid floats
            let scaled_delta = (delta_ns.saturating_mul(1024)) / (weight as u64);
            tasks[i].vruntime = tasks[i].vruntime.saturating_add(scaled_delta);
            tasks[i].runtime_ns = tasks[i].runtime_ns.saturating_add(delta_ns);

            return true;
        }
    }

    false
}

/// Set CPU affinity for a task
pub fn cfs_set_affinity(pid: u32, cpu_mask: u64) -> bool {
    if pid == 0 {
        return false;
    }

    let mut tasks = CFS_TASKS.lock();
    for i in 0..CFS_MAX_TASKS {
        if tasks[i].active && tasks[i].pid == pid {
            tasks[i].cpu_affinity = cpu_mask;
            return true;
        }
    }

    false
}

/// Process one CFS tick (1ms = 1_000_000 nanoseconds)
pub fn cfs_tick(current_ns: u64) {
    let current_pid = CFS_CURRENT_PID.load(Ordering::Relaxed);
    if current_pid == 0 {
        return; // No current task
    }

    // Update current task's vruntime for one tick
    cfs_update_vruntime(current_pid, 1_000_000);

    // Update min_vruntime to minimum active task's vruntime
    let tasks = CFS_TASKS.lock();
    let mut min_vruntime = u64::MAX;
    for i in 0..CFS_MAX_TASKS {
        if tasks[i].active && tasks[i].vruntime < min_vruntime {
            min_vruntime = tasks[i].vruntime;
        }
    }

    if min_vruntime != u64::MAX {
        CFS_MIN_VRUNTIME.store(min_vruntime, Ordering::Relaxed);
    }
}

/// Initialize CFS scheduler
pub fn init() {
    let mut tasks = CFS_TASKS.lock();

    // Enqueue synthetic init task (pid=1, nice=0)
    tasks[0] = CfsTask {
        pid: 1,
        vruntime: 0,
        weight: nice_to_weight(0),
        nice: 0,
        cpu_affinity: 0,
        runtime_ns: 0,
        sleep_start_ns: 0,
        active: true,
    };

    CFS_CURRENT_PID.store(1, Ordering::Relaxed);
    CFS_MIN_VRUNTIME.store(0, Ordering::Relaxed);

    crate::serial_println!("[sched_cfs] CFS scheduler initialized");
}
