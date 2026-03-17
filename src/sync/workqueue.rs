/// Workqueue — deferred work execution for Genesis
///
/// Workqueues allow kernel code to defer work to be executed later in
/// process context (not interrupt context). Supports per-CPU queues,
/// delayed work, and different priority levels.
///
/// Inspired by: Linux workqueue (kernel/workqueue.c). All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Work item priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkPriority {
    High,
    Normal,
    Low,
}

/// A work item
pub struct WorkItem {
    /// Function to execute
    pub func: fn(usize),
    /// Argument
    pub data: usize,
    /// Priority
    pub priority: WorkPriority,
    /// Delay (ms, 0 = immediate)
    pub delay_ms: u64,
    /// Time when queued
    pub queued_at: u64,
    /// Name (for debugging)
    pub name: String,
}

/// A workqueue
pub struct Workqueue {
    /// Name
    pub name: String,
    /// Pending work items
    pending: Vec<WorkItem>,
    /// Whether this is a per-CPU queue
    pub per_cpu: bool,
    /// Maximum concurrent workers
    pub max_workers: u32,
    /// Active workers
    pub active_workers: u32,
    /// Statistics
    pub stats: WqStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WqStats {
    pub items_queued: u64,
    pub items_executed: u64,
    pub items_cancelled: u64,
    pub max_pending: u64,
}

impl Workqueue {
    pub fn new(name: &str, per_cpu: bool, max_workers: u32) -> Self {
        Workqueue {
            name: String::from(name),
            pending: Vec::new(),
            per_cpu,
            max_workers,
            active_workers: 0,
            stats: WqStats::default(),
        }
    }

    /// Queue a work item
    pub fn queue_work(&mut self, func: fn(usize), data: usize, name: &str) {
        self.pending.push(WorkItem {
            func,
            data,
            priority: WorkPriority::Normal,
            delay_ms: 0,
            queued_at: crate::time::clock::uptime_ms(),
            name: String::from(name),
        });
        self.stats.items_queued = self.stats.items_queued.saturating_add(1);
        let pending = self.pending.len() as u64;
        if pending > self.stats.max_pending {
            self.stats.max_pending = pending;
        }
    }

    /// Queue delayed work
    pub fn queue_delayed_work(&mut self, func: fn(usize), data: usize, delay_ms: u64, name: &str) {
        self.pending.push(WorkItem {
            func,
            data,
            priority: WorkPriority::Normal,
            delay_ms,
            queued_at: crate::time::clock::uptime_ms(),
            name: String::from(name),
        });
        self.stats.items_queued = self.stats.items_queued.saturating_add(1);
    }

    /// Queue high-priority work
    pub fn queue_work_high(&mut self, func: fn(usize), data: usize, name: &str) {
        self.pending.insert(
            0,
            WorkItem {
                func,
                data,
                priority: WorkPriority::High,
                delay_ms: 0,
                queued_at: crate::time::clock::uptime_ms(),
                name: String::from(name),
            },
        );
        self.stats.items_queued = self.stats.items_queued.saturating_add(1);
    }

    /// Process pending work items. Returns number of items executed.
    pub fn process(&mut self) -> usize {
        let now = crate::time::clock::uptime_ms();
        let mut executed: usize = 0;
        let mut remaining = Vec::new();

        for item in self.pending.drain(..) {
            if item.delay_ms > 0 && now < item.queued_at + item.delay_ms {
                remaining.push(item);
                continue;
            }
            (item.func)(item.data);
            executed = executed.saturating_add(1);
            self.stats.items_executed = self.stats.items_executed.saturating_add(1);
        }

        self.pending = remaining;
        // Sort remaining by priority
        self.pending.sort_by(|a, b| a.priority.cmp(&b.priority));
        executed
    }

    /// Cancel all pending work
    pub fn flush(&mut self) -> usize {
        let cancelled = self.pending.len();
        self.pending.clear();
        self.stats.items_cancelled = self.stats.items_cancelled.saturating_add(cancelled as u64);
        cancelled
    }

    /// Number of pending items
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

/// Maximum number of workqueues
const MAX_WORKQUEUES: usize = 16;

/// System workqueues
struct WorkqueueSystem {
    queues: [Option<Workqueue>; MAX_WORKQUEUES],
    count: usize,
}

impl WorkqueueSystem {
    const fn new() -> Self {
        const NONE: Option<Workqueue> = None;
        WorkqueueSystem {
            queues: [NONE; MAX_WORKQUEUES],
            count: 0,
        }
    }
}

static WQ_SYSTEM: Mutex<WorkqueueSystem> = Mutex::new(WorkqueueSystem::new());

/// Well-known workqueue indices
pub const WQ_SYSTEM_DEFAULT: usize = 0;
pub const WQ_SYSTEM_HIGHPRI: usize = 1;
pub const WQ_SYSTEM_LONG: usize = 2;
pub const WQ_SYSTEM_UNBOUND: usize = 3;

/// Create a workqueue. Returns index.
pub fn create_workqueue(name: &str, per_cpu: bool, max_workers: u32) -> Option<usize> {
    let mut sys = WQ_SYSTEM.lock();
    if sys.count >= MAX_WORKQUEUES {
        return None;
    }

    let idx = sys.count;
    sys.queues[idx] = Some(Workqueue::new(name, per_cpu, max_workers));
    sys.count = sys.count.saturating_add(1);
    Some(idx)
}

/// Queue work on a workqueue
pub fn queue_work(wq_idx: usize, func: fn(usize), data: usize, name: &str) {
    if wq_idx >= MAX_WORKQUEUES {
        return;
    }
    let mut sys = WQ_SYSTEM.lock();
    if let Some(ref mut wq) = sys.queues[wq_idx] {
        wq.queue_work(func, data, name);
    }
}

/// Queue delayed work
pub fn queue_delayed_work(wq_idx: usize, func: fn(usize), data: usize, delay_ms: u64, name: &str) {
    if wq_idx >= MAX_WORKQUEUES {
        return;
    }
    let mut sys = WQ_SYSTEM.lock();
    if let Some(ref mut wq) = sys.queues[wq_idx] {
        wq.queue_delayed_work(func, data, delay_ms, name);
    }
}

/// Process all workqueues (called from kernel main loop or worker threads)
pub fn process_all() -> usize {
    let mut total = 0;
    let mut sys = WQ_SYSTEM.lock();
    for i in 0..sys.count {
        if let Some(ref mut wq) = sys.queues[i] {
            total += wq.process();
        }
    }
    total
}

/// Schedule work on the default system workqueue
pub fn schedule_work(func: fn(usize), data: usize, name: &str) {
    queue_work(WQ_SYSTEM_DEFAULT, func, data, name);
}

/// Schedule delayed work on the default system workqueue
pub fn schedule_delayed_work(func: fn(usize), data: usize, delay_ms: u64, name: &str) {
    queue_delayed_work(WQ_SYSTEM_DEFAULT, func, data, delay_ms, name);
}

/// Initialize the workqueue system
pub fn init() {
    let mut sys = WQ_SYSTEM.lock();
    sys.queues[0] = Some(Workqueue::new("events", true, 4));
    sys.queues[1] = Some(Workqueue::new("events_highpri", true, 4));
    sys.queues[2] = Some(Workqueue::new("events_long", false, 2));
    sys.queues[3] = Some(Workqueue::new("events_unbound", false, 8));
    sys.count = 4;

    crate::serial_println!("  [workqueue] 4 system workqueues initialized");
}
