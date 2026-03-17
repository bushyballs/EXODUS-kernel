/// Block I/O scheduler for Genesis
///
/// Manages I/O request ordering, merging, and scheduling for block devices.
/// Implements multiple scheduling policies:
///   - Noop: FIFO, no reordering
///   - Deadline: latency-focused, separate read/write queues with deadlines
///   - CFQ/BFQ: fairness-focused, round-robin between processes
///   - Elevator (SCAN): sort requests by sector for minimal seek
///
/// Features:
///   - BIO request struct with device, sector, count, buffer, direction, callback
///   - Per-device request queues
///   - Request merging (combine adjacent sector requests)
///   - Priority levels (sync reads > async writes)
///   - Plugging/unplugging (batch requests before submitting)
///   - Request completion callback dispatch
///   - Statistics tracking
///
/// Inspired by: Linux block layer (block/). All code is original.
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Block I/O request priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BioPriority {
    /// Highest priority: synchronous reads (user is waiting)
    SyncRead = 0,
    /// High priority: synchronous writes (fsync, journaling)
    SyncWrite = 1,
    /// Normal priority: async reads (readahead, prefetch)
    AsyncRead = 2,
    /// Low priority: async writes (writeback, background flush)
    AsyncWrite = 3,
    /// Lowest priority: maintenance I/O (scrub, trim)
    Idle = 4,
}

impl BioPriority {
    fn from_params(write: bool, sync: bool) -> Self {
        match (write, sync) {
            (false, true) => BioPriority::SyncRead,
            (true, true) => BioPriority::SyncWrite,
            (false, false) => BioPriority::AsyncRead,
            (true, false) => BioPriority::AsyncWrite,
        }
    }
}

/// Block I/O request direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BioDirection {
    Read,
    Write,
}

/// Completion callback function type
/// Takes the request ID and whether it succeeded
pub type BioCallback = fn(u64, bool);

/// Block I/O request
#[derive(Clone)]
pub struct BioRequest {
    pub id: u64,
    pub device: u32,
    pub sector: u64,
    pub count: u32,
    pub write: bool,
    pub buffer: usize,
    pub priority: BioPriority,
    pub submit_time: u64,
    pub deadline_ms: u64,
    pub pid: u32,
    pub completed: bool,
    pub sync: bool,
    pub callback: Option<BioCallback>,
}

/// I/O scheduler type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoScheduler {
    /// No reordering, FIFO
    Noop,
    /// Latency-focused, separate read/write queues with deadlines
    Deadline,
    /// Completely Fair Queuing (process fairness)
    Cfq,
    /// Budget Fair Queuing (improved CFQ)
    Bfq,
    /// Multi-Queue Deadline (for NVMe/SSD)
    MqDeadline,
}

/// Per-device request queue
struct DeviceQueue {
    device_id: u32,
    read_queue: Vec<BioRequest>,
    write_queue: Vec<BioRequest>,
    plugged: bool,
    plug_count: u32,
    /// Elevator direction: true = ascending, false = descending
    elevator_ascending: bool,
    /// Last dispatched sector (for elevator scheduling)
    last_sector: u64,
}

impl DeviceQueue {
    fn new(device_id: u32) -> Self {
        DeviceQueue {
            device_id,
            read_queue: Vec::new(),
            write_queue: Vec::new(),
            plugged: false,
            plug_count: 0,
            elevator_ascending: true,
            last_sector: 0,
        }
    }

    /// Insert a request into the appropriate sorted queue
    fn insert_sorted(&mut self, req: BioRequest) {
        let queue = if req.write {
            &mut self.write_queue
        } else {
            &mut self.read_queue
        };

        // Find insertion point (sorted by sector for elevator scheduling)
        let pos = queue
            .iter()
            .position(|r| r.sector > req.sector)
            .unwrap_or(queue.len());
        queue.insert(pos, req);
    }

    /// Try to merge a request with existing requests in the queue
    fn try_merge(&mut self, req: &BioRequest) -> bool {
        let queue = if req.write {
            &mut self.write_queue
        } else {
            &mut self.read_queue
        };

        for pending in queue.iter_mut() {
            if pending.device != req.device {
                continue;
            }

            // Back merge: new request immediately follows existing
            let pending_end = pending.sector.saturating_add(pending.count as u64);
            if pending_end == req.sector {
                pending.count = pending.count.saturating_add(req.count);
                return true;
            }

            // Front merge: new request immediately precedes existing
            let req_end = req.sector.saturating_add(req.count as u64);
            if req_end == pending.sector {
                pending.sector = req.sector;
                pending.count = pending.count.saturating_add(req.count);
                return true;
            }
        }

        false
    }

    /// Total pending requests
    fn pending_count(&self) -> usize {
        self.read_queue.len() + self.write_queue.len()
    }

    /// Pick the next request using elevator (SCAN) algorithm
    fn dispatch_elevator(&mut self) -> Option<BioRequest> {
        // Reads have higher priority than writes
        if !self.read_queue.is_empty() {
            return Self::pick_from_sorted_queue(
                &mut self.read_queue,
                self.last_sector,
                &mut self.elevator_ascending,
            );
        }
        if !self.write_queue.is_empty() {
            return Self::pick_from_sorted_queue(
                &mut self.write_queue,
                self.last_sector,
                &mut self.elevator_ascending,
            );
        }
        None
    }

    /// Pick next request from a sorted queue using SCAN algorithm
    /// Searches for the nearest request in the current elevator direction
    fn pick_from_sorted_queue(
        queue: &mut Vec<BioRequest>,
        last_sector: u64,
        ascending: &mut bool,
    ) -> Option<BioRequest> {
        if queue.is_empty() {
            return None;
        }

        if *ascending {
            // Find first request at or above last_sector
            let idx = queue.iter().position(|r| r.sector >= last_sector);
            match idx {
                Some(i) => Some(queue.remove(i)),
                None => {
                    // No requests above: reverse direction, take from end
                    *ascending = false;
                    Some(queue.remove(queue.len() - 1))
                }
            }
        } else {
            // Find last request at or below last_sector
            let idx = queue.iter().rposition(|r| r.sector <= last_sector);
            match idx {
                Some(i) => Some(queue.remove(i)),
                None => {
                    // No requests below: reverse direction, take from start
                    *ascending = true;
                    Some(queue.remove(0))
                }
            }
        }
    }

    /// Pick next request using deadline algorithm
    fn dispatch_deadline(&mut self, now: u64) -> Option<BioRequest> {
        // Check for expired read deadlines first (reads are latency-sensitive)
        if let Some(idx) = self
            .read_queue
            .iter()
            .position(|r| now >= r.submit_time + r.deadline_ms)
        {
            return Some(self.read_queue.remove(idx));
        }

        // Check for expired write deadlines
        if let Some(idx) = self
            .write_queue
            .iter()
            .position(|r| now >= r.submit_time + r.deadline_ms)
        {
            return Some(self.write_queue.remove(idx));
        }

        // No deadlines expired: use elevator ordering, reads first
        if !self.read_queue.is_empty() {
            return Some(self.dispatch_next_in_order(&mut true));
        }
        if !self.write_queue.is_empty() {
            return Some(self.dispatch_next_in_order(&mut false));
        }

        None
    }

    /// Dispatch next request in elevator order
    fn dispatch_next_in_order(&mut self, is_read: &mut bool) -> BioRequest {
        let queue = if *is_read {
            &mut self.read_queue
        } else {
            &mut self.write_queue
        };

        if self.elevator_ascending {
            // Find nearest request at or above last_sector
            let idx = queue
                .iter()
                .position(|r| r.sector >= self.last_sector)
                .unwrap_or(0);

            if idx >= queue.len() {
                // Reverse direction
                self.elevator_ascending = false;
                let last = queue.len() - 1;
                let req = queue.remove(last);
                self.last_sector = req.sector;
                req
            } else {
                let req = queue.remove(idx);
                self.last_sector = req.sector;
                req
            }
        } else {
            // Find nearest request at or below last_sector (search backward)
            let idx = queue.iter().rposition(|r| r.sector <= self.last_sector);
            match idx {
                Some(i) => {
                    let req = queue.remove(i);
                    self.last_sector = req.sector;
                    req
                }
                None => {
                    // Reverse direction
                    self.elevator_ascending = true;
                    let req = queue.remove(0);
                    self.last_sector = req.sector;
                    req
                }
            }
        }
    }
}

/// I/O statistics
#[derive(Default, Clone)]
pub struct IoStats {
    pub reads: u64,
    pub writes: u64,
    pub sectors_read: u64,
    pub sectors_written: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub merges: u64,
    pub requests_queued: u64,
    pub requests_completed: u64,
    pub requests_dispatched: u64,
    pub queue_depth_max: u32,
    pub plug_count: u64,
    pub unplug_count: u64,
}

/// Block I/O layer
pub struct BlockIoLayer {
    scheduler: IoScheduler,
    next_id: u64,
    /// Per-device request queues
    device_queues: BTreeMap<u32, DeviceQueue>,
    /// Legacy single pending queue (for backward compat)
    pending: Vec<BioRequest>,
    stats: IoStats,
    /// Global plug state: when plugged, requests accumulate without dispatching
    global_plugged: bool,
    /// Maximum requests to batch before auto-unplugging
    plug_threshold: u32,
    /// Completed request IDs awaiting callback dispatch
    completed_ids: Vec<(u64, bool)>,
    /// Active callbacks
    callbacks: BTreeMap<u64, BioCallback>,
}

impl BlockIoLayer {
    const fn new() -> Self {
        BlockIoLayer {
            scheduler: IoScheduler::Deadline,
            next_id: 1,
            device_queues: BTreeMap::new(),
            pending: Vec::new(),
            stats: IoStats {
                reads: 0,
                writes: 0,
                sectors_read: 0,
                sectors_written: 0,
                bytes_read: 0,
                bytes_written: 0,
                merges: 0,
                requests_queued: 0,
                requests_completed: 0,
                requests_dispatched: 0,
                queue_depth_max: 0,
                plug_count: 0,
                unplug_count: 0,
            },
            global_plugged: false,
            plug_threshold: 16,
            completed_ids: Vec::new(),
            callbacks: BTreeMap::new(),
        }
    }

    /// Get or create a device queue
    fn get_or_create_queue(&mut self, device: u32) -> &mut DeviceQueue {
        self.device_queues
            .entry(device)
            .or_insert_with(|| DeviceQueue::new(device))
    }

    /// Submit a block I/O request
    pub fn submit(
        &mut self,
        device: u32,
        sector: u64,
        count: u32,
        write: bool,
        buffer: usize,
        pid: u32,
    ) -> u64 {
        self.submit_full(device, sector, count, write, buffer, pid, false, None)
    }

    /// Submit with full options (sync flag, callback)
    pub fn submit_full(
        &mut self,
        device: u32,
        sector: u64,
        count: u32,
        write: bool,
        buffer: usize,
        pid: u32,
        sync: bool,
        callback: Option<BioCallback>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let priority = BioPriority::from_params(write, sync);

        // Deadline depends on priority
        let deadline = match priority {
            BioPriority::SyncRead => 100,
            BioPriority::SyncWrite => 250,
            BioPriority::AsyncRead => 500,
            BioPriority::AsyncWrite => 5000,
            BioPriority::Idle => 30000,
        };

        let req = BioRequest {
            id,
            device,
            sector,
            count,
            write,
            buffer,
            priority,
            submit_time: crate::time::clock::uptime_ms(),
            deadline_ms: deadline,
            pid,
            completed: false,
            sync,
            callback,
        };

        // Register callback if provided
        if let Some(cb) = callback {
            self.callbacks.insert(id, cb);
        }

        // Try to merge with existing request in the device queue
        let dq = self.get_or_create_queue(device);
        if dq.try_merge(&req) {
            self.stats.merges = self.stats.merges.saturating_add(1);
            return id;
        }

        // Insert into device-specific sorted queue
        dq.insert_sorted(req.clone());

        // Also maintain legacy pending list
        self.pending.push(req);

        self.stats.requests_queued = self.stats.requests_queued.saturating_add(1);

        if write {
            self.stats.writes = self.stats.writes.saturating_add(1);
            self.stats.sectors_written = self.stats.sectors_written.saturating_add(count as u64);
            self.stats.bytes_written = self
                .stats
                .bytes_written
                .saturating_add((count as u64).saturating_mul(512));
        } else {
            self.stats.reads = self.stats.reads.saturating_add(1);
            self.stats.sectors_read = self.stats.sectors_read.saturating_add(count as u64);
            self.stats.bytes_read = self
                .stats
                .bytes_read
                .saturating_add((count as u64).saturating_mul(512));
        }

        let depth = self.total_pending() as u32;
        if depth > self.stats.queue_depth_max {
            self.stats.queue_depth_max = depth;
        }

        // Auto-unplug if we hit the threshold
        if self.global_plugged {
            if let Some(dq) = self.device_queues.get(&device) {
                if dq.pending_count() as u32 >= self.plug_threshold {
                    self.global_plugged = false;
                    self.stats.unplug_count = self.stats.unplug_count.saturating_add(1);
                }
            }
        }

        id
    }

    /// Get next request to dispatch (based on scheduler policy)
    pub fn dispatch(&mut self) -> Option<BioRequest> {
        // If globally plugged, don't dispatch
        if self.global_plugged {
            return None;
        }

        if self.pending.is_empty() {
            return None;
        }

        let result = match self.scheduler {
            IoScheduler::Noop => {
                // Simple FIFO
                Some(self.pending.remove(0))
            }
            IoScheduler::Deadline | IoScheduler::MqDeadline => {
                let now = crate::time::clock::uptime_ms();
                // Try each device queue
                let mut best: Option<BioRequest> = None;

                // Collect device IDs to iterate
                let device_ids: Vec<u32> = self.device_queues.keys().cloned().collect();

                for dev_id in device_ids {
                    if let Some(dq) = self.device_queues.get_mut(&dev_id) {
                        if let Some(req) = dq.dispatch_deadline(now) {
                            // Remove from legacy pending list too
                            self.pending.retain(|r| r.id != req.id);
                            best = Some(req);
                            break;
                        }
                    }
                }

                if best.is_none() {
                    // Fallback: deadline on the legacy pending list
                    let now = crate::time::clock::uptime_ms();
                    // Expired reads first
                    let expired_read = self
                        .pending
                        .iter()
                        .position(|r| !r.write && now >= r.submit_time + r.deadline_ms);
                    if let Some(idx) = expired_read {
                        best = Some(self.pending.remove(idx));
                    } else {
                        let expired_write = self
                            .pending
                            .iter()
                            .position(|r| r.write && now >= r.submit_time + r.deadline_ms);
                        if let Some(idx) = expired_write {
                            best = Some(self.pending.remove(idx));
                        } else {
                            // Elevator: lowest sector read first
                            let read_idx = self
                                .pending
                                .iter()
                                .enumerate()
                                .filter(|(_, r)| !r.write)
                                .min_by_key(|(_, r)| r.sector)
                                .map(|(i, _)| i);
                            if let Some(idx) = read_idx {
                                best = Some(self.pending.remove(idx));
                            } else {
                                let write_idx = self
                                    .pending
                                    .iter()
                                    .enumerate()
                                    .min_by_key(|(_, r)| r.sector)
                                    .map(|(i, _)| i);
                                best = write_idx.map(|idx| self.pending.remove(idx));
                            }
                        }
                    }
                }

                best
            }
            IoScheduler::Cfq | IoScheduler::Bfq => {
                // Round-robin between PIDs
                // Group by PID and take one from each in turn
                if self.pending.is_empty() {
                    None
                } else {
                    // Find the PID with the oldest request
                    let mut pid_map: BTreeMap<u32, usize> = BTreeMap::new();
                    for (i, req) in self.pending.iter().enumerate() {
                        pid_map.entry(req.pid).or_insert(i);
                    }

                    // Take the request from the PID with the oldest submission
                    let oldest_idx = pid_map.values().copied().min();
                    oldest_idx.map(|idx| self.pending.remove(idx))
                }
            }
        };

        if result.is_some() {
            self.stats.requests_dispatched = self.stats.requests_dispatched.saturating_add(1);
        }

        result
    }

    /// Mark a request as completed and dispatch its callback
    pub fn complete(&mut self, request_id: u64, success: bool) {
        self.stats.requests_completed = self.stats.requests_completed.saturating_add(1);

        // Remove from pending if still there
        self.pending.retain(|r| r.id != request_id);

        // Dispatch callback
        if let Some(cb) = self.callbacks.remove(&request_id) {
            cb(request_id, success);
        }

        self.completed_ids.push((request_id, success));

        // Keep completed list bounded
        if self.completed_ids.len() > 1024 {
            self.completed_ids.drain(0..512);
        }
    }

    /// Plug the I/O queue (batch requests)
    pub fn plug(&mut self) {
        if !self.global_plugged {
            self.global_plugged = true;
            self.stats.plug_count = self.stats.plug_count.saturating_add(1);
        }
    }

    /// Unplug the I/O queue (allow dispatching)
    pub fn unplug(&mut self) {
        if self.global_plugged {
            self.global_plugged = false;
            self.stats.unplug_count = self.stats.unplug_count.saturating_add(1);
        }
    }

    /// Check if plugged
    pub fn is_plugged(&self) -> bool {
        self.global_plugged
    }

    /// Set the plug threshold (auto-unplug after this many requests)
    pub fn set_plug_threshold(&mut self, threshold: u32) {
        self.plug_threshold = threshold;
    }

    /// Set scheduler policy
    pub fn set_scheduler(&mut self, sched: IoScheduler) {
        self.scheduler = sched;
        serial_println!("  [block_io] Scheduler changed to {:?}", sched);
    }

    /// Get current scheduler
    pub fn scheduler(&self) -> IoScheduler {
        self.scheduler
    }

    /// Total pending requests across all queues
    fn total_pending(&self) -> usize {
        self.pending.len()
    }

    /// Get pending count
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Get per-device pending count
    pub fn device_pending_count(&self, device: u32) -> usize {
        self.device_queues
            .get(&device)
            .map(|dq| dq.pending_count())
            .unwrap_or(0)
    }

    /// Get statistics
    pub fn stats(&self) -> &IoStats {
        &self.stats
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = IoStats::default();
    }

    /// Drain all pending requests for a device (e.g., on device removal)
    pub fn drain_device(&mut self, device: u32) -> Vec<BioRequest> {
        let mut drained = Vec::new();

        if let Some(dq) = self.device_queues.remove(&device) {
            drained.extend(dq.read_queue);
            drained.extend(dq.write_queue);
        }

        let mut legacy_drained = Vec::new();
        self.pending.retain(|r| {
            if r.device == device {
                legacy_drained.push(r.clone());
                false
            } else {
                true
            }
        });

        // Merge (device queue requests are authoritative)
        if drained.is_empty() {
            drained = legacy_drained;
        }

        drained
    }

    /// Flush all pending writes for a device (dispatch write requests immediately)
    pub fn flush_device(&mut self, device: u32) -> Vec<BioRequest> {
        let mut flushed = Vec::new();

        if let Some(dq) = self.device_queues.get_mut(&device) {
            flushed.append(&mut dq.write_queue);
        }

        self.pending.retain(|r| !(r.device == device && r.write));

        self.stats.requests_dispatched = self
            .stats
            .requests_dispatched
            .saturating_add(flushed.len() as u64);
        flushed
    }
}

static BLOCK_IO: Mutex<BlockIoLayer> = Mutex::new(BlockIoLayer::new());

/// Initialize the block I/O subsystem
pub fn init() {
    crate::serial_println!("  [block_io] Block I/O scheduler initialized (deadline)");
}

/// Submit a basic block I/O request
pub fn submit(device: u32, sector: u64, count: u32, write: bool, buffer: usize, pid: u32) -> u64 {
    BLOCK_IO
        .lock()
        .submit(device, sector, count, write, buffer, pid)
}

/// Submit with full options
pub fn submit_full(
    device: u32,
    sector: u64,
    count: u32,
    write: bool,
    buffer: usize,
    pid: u32,
    sync: bool,
    callback: Option<BioCallback>,
) -> u64 {
    BLOCK_IO
        .lock()
        .submit_full(device, sector, count, write, buffer, pid, sync, callback)
}

/// Dispatch the next request
pub fn dispatch() -> Option<BioRequest> {
    BLOCK_IO.lock().dispatch()
}

/// Mark a request as completed
pub fn complete(request_id: u64, success: bool) {
    BLOCK_IO.lock().complete(request_id, success);
}

/// Set the I/O scheduler
pub fn set_scheduler(sched: IoScheduler) {
    BLOCK_IO.lock().set_scheduler(sched);
}

/// Get the current scheduler
pub fn scheduler() -> IoScheduler {
    BLOCK_IO.lock().scheduler()
}

/// Plug the I/O queue (batch requests)
pub fn plug() {
    BLOCK_IO.lock().plug();
}

/// Unplug the I/O queue (allow dispatching)
pub fn unplug() {
    BLOCK_IO.lock().unplug();
}

/// Get pending request count
pub fn pending_count() -> usize {
    BLOCK_IO.lock().pending_count()
}

/// Get I/O statistics
pub fn get_stats() -> IoStats {
    BLOCK_IO.lock().stats().clone()
}

/// Reset I/O statistics
pub fn reset_stats() {
    BLOCK_IO.lock().reset_stats();
}

/// Drain all requests for a device
pub fn drain_device(device: u32) -> Vec<BioRequest> {
    BLOCK_IO.lock().drain_device(device)
}

/// Flush writes for a device
pub fn flush_device(device: u32) -> Vec<BioRequest> {
    BLOCK_IO.lock().flush_device(device)
}

/// Get per-device pending count
pub fn device_pending(device: u32) -> usize {
    BLOCK_IO.lock().device_pending_count(device)
}

/// Check if the I/O layer is plugged
pub fn is_plugged() -> bool {
    BLOCK_IO.lock().is_plugged()
}

/// Set the plug threshold
pub fn set_plug_threshold(threshold: u32) {
    BLOCK_IO.lock().set_plug_threshold(threshold);
}

/// Register a block device with the I/O layer
/// This creates the per-device queue if it doesn't exist
pub fn register_device(device_id: u32) {
    BLOCK_IO.lock().get_or_create_queue(device_id);
}

/// Batch submission: submit multiple I/O requests atomically with plugging
/// This plugs the queue, submits all requests, then unplugs
pub fn submit_batch(requests: &[(u32, u64, u32, bool, usize, u32)]) -> Vec<u64> {
    let mut bio = BLOCK_IO.lock();
    bio.plug();
    let mut ids = Vec::with_capacity(requests.len());
    for &(device, sector, count, write, buffer, pid) in requests {
        let id = bio.submit(device, sector, count, write, buffer, pid);
        ids.push(id);
    }
    bio.unplug();
    ids
}

/// Dispatch multiple requests at once (up to `max` requests)
pub fn dispatch_batch(max: usize) -> Vec<BioRequest> {
    let mut bio = BLOCK_IO.lock();
    let mut batch = Vec::with_capacity(max);
    for _ in 0..max {
        match bio.dispatch() {
            Some(req) => batch.push(req),
            None => break,
        }
    }
    batch
}

/// Get a summary string of the I/O stats
pub fn stats_summary() -> alloc::string::String {
    let stats = BLOCK_IO.lock().stats().clone();
    alloc::format!(
        "reads={} writes={} rd_sect={} wr_sect={} merges={} queued={} dispatched={} completed={} max_depth={}",
        stats.reads, stats.writes,
        stats.sectors_read, stats.sectors_written,
        stats.merges,
        stats.requests_queued, stats.requests_dispatched,
        stats.requests_completed, stats.queue_depth_max,
    )
}
