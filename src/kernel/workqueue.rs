/// Deferred work execution and per-CPU work queues.
///
/// Part of the AIOS kernel.
use alloc::boxed::Box;
use alloc::collections::VecDeque;

/// A unit of deferred work.
pub struct WorkItem {
    /// Callback to execute.
    pub func: Box<dyn FnOnce() + Send>,
}

/// Per-CPU work queue that drains deferred work items.
pub struct WorkQueue {
    pub pending: VecDeque<WorkItem>,
    pub cpu_id: usize,
}

/// Maximum number of items allowed in one work queue to bound memory use.
const MAX_WORK_ITEMS: usize = 1024;

impl WorkQueue {
    pub fn new(cpu_id: usize) -> Self {
        WorkQueue {
            pending: VecDeque::new(),
            cpu_id,
        }
    }

    /// Push a work item onto the queue. Drops the item if the queue is full.
    pub fn enqueue(&mut self, item: WorkItem) {
        if self.pending.len() >= MAX_WORK_ITEMS {
            // Drop item rather than panic — log the overflow.
            crate::serial_println!(
                "workqueue[cpu={}]: queue full, dropping work item",
                self.cpu_id
            );
            return;
        }
        self.pending.push_back(item);
    }

    /// Drain and execute all pending work items in FIFO order.
    pub fn drain(&mut self) {
        while let Some(item) = self.pending.pop_front() {
            (item.func)();
        }
    }
}

/// Initialize the workqueue subsystem.
pub fn init() {
    // TODO: Create per-CPU work queues
}
