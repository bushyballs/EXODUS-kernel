/// I/O Scheduling — CFQ, deadline, BFQ, priorities, request merging
///
/// Manages block I/O request ordering and dispatch:
///   - CFQ (Completely Fair Queuing): per-process fair bandwidth
///   - Deadline: latency guarantee with read/write deadlines
///   - BFQ (Budget Fair Queuing): proportional bandwidth + latency
///   - Priority classes: real-time, best-effort, idle
///   - Request merging: back-merge, front-merge, bio coalescing
///   - Queue depth management for NVMe/SCSI devices
///
/// All code is original. Built from scratch for Hoags Inc.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

#[inline]
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> Q16_SHIFT) as i32
}

#[inline]
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_QUEUE_DEPTH: usize = 256;
const DEADLINE_READ_MS: u64 = 500;
const DEADLINE_WRITE_MS: u64 = 5000;
const BFQ_DEFAULT_BUDGET: u32 = 16;    // sectors per budget slice
const CFQ_TIMESLICE_MS: u64 = 100;
const MERGE_WINDOW_SECTORS: u64 = 256; // max gap for merge consideration

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Scheduling algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoScheduler {
    /// No reordering (FIFO)
    None,
    /// Completely Fair Queuing — per-process fair bandwidth
    Cfq,
    /// Deadline — read/write latency guarantees
    Deadline,
    /// Budget Fair Queuing — proportional bandwidth + low latency
    Bfq,
}

/// I/O priority class (aligned with Linux ioprio)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IoPriorityClass {
    /// Highest priority, guaranteed latency
    RealTime,
    /// Normal priority, fair share
    BestEffort,
    /// Lowest priority, only serviced when device is idle
    Idle,
}

/// Request direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoDirection {
    Read,
    Write,
    Flush,
    Discard,
}

/// Request state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestState {
    Pending,
    Merged,
    Dispatched,
    Completed,
    Error,
}

/// Merge result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeResult {
    BackMerge,
    FrontMerge,
    NoMerge,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A single block I/O request
#[derive(Debug, Clone)]
pub struct IoRequest {
    pub id: u64,
    pub direction: IoDirection,
    pub sector_start: u64,
    pub sector_count: u32,
    pub priority_class: IoPriorityClass,
    pub priority_level: u8,         // 0-7 within class
    pub process_id: u32,
    pub submit_time: u64,           // timestamp when submitted
    pub deadline: u64,              // absolute deadline timestamp
    pub state: RequestState,
    pub data_ptr: u64,              // pointer to buffer (virtual address)
    pub merged_count: u32,          // how many requests merged into this one
}

/// Per-process CFQ queue
#[derive(Debug, Clone)]
pub struct CfqProcessQueue {
    pub process_id: u32,
    pub requests: Vec<usize>,       // indices into the main request array
    pub timeslice_start: u64,
    pub timeslice_remaining_ms: u64,
    pub sectors_dispatched: u64,
    pub weight: u32,                // scheduling weight (default 100)
    pub priority: IoPriorityClass,
}

/// BFQ per-process budget tracking
#[derive(Debug, Clone)]
pub struct BfqEntity {
    pub process_id: u32,
    pub budget: u32,                // sectors remaining in current budget
    pub max_budget: u32,
    pub weight: u32,
    pub priority: IoPriorityClass,
    pub requests: Vec<usize>,
    pub virtual_time: i32,          // Q16 virtual finish time
    pub total_sectors: u64,
}

/// Device queue configuration
#[derive(Debug, Clone)]
pub struct DeviceQueue {
    pub device_id: u32,
    pub max_depth: usize,
    pub current_depth: usize,
    pub nr_hw_queues: u32,          // hardware queues (for NVMe multi-queue)
    pub rotational: bool,           // true = HDD (seek-sensitive), false = SSD
    pub max_sectors: u32,           // max sectors per request
    pub optimal_io_size: u32,       // optimal I/O size in sectors
}

/// I/O scheduler subsystem
pub struct IoSchedulerState {
    pub algorithm: IoScheduler,
    pub requests: Vec<IoRequest>,
    pub next_id: u64,
    pub device: DeviceQueue,

    // CFQ state
    pub cfq_queues: Vec<CfqProcessQueue>,
    pub cfq_active_queue: Option<usize>,    // index into cfq_queues
    pub cfq_current_time: u64,

    // Deadline state
    pub deadline_read_queue: Vec<usize>,    // sorted by sector
    pub deadline_write_queue: Vec<usize>,   // sorted by sector
    pub deadline_read_fifo: Vec<usize>,     // sorted by deadline
    pub deadline_write_fifo: Vec<usize>,    // sorted by deadline
    pub deadline_writes_starved: u32,
    pub deadline_write_starvation_limit: u32,
    pub deadline_last_direction: IoDirection,

    // BFQ state
    pub bfq_entities: Vec<BfqEntity>,
    pub bfq_active: Option<usize>,
    pub bfq_virtual_time: i32,             // Q16 global virtual time

    // Statistics
    pub total_submitted: u64,
    pub total_completed: u64,
    pub total_merged: u64,
    pub total_read_sectors: u64,
    pub total_write_sectors: u64,
    pub read_latency_sum_ms: u64,
    pub write_latency_sum_ms: u64,
    pub tick_count: u64,
}

impl IoSchedulerState {
    const fn new() -> Self {
        IoSchedulerState {
            algorithm: IoScheduler::Bfq,
            requests: Vec::new(),
            next_id: 1,
            device: DeviceQueue {
                device_id: 0,
                max_depth: MAX_QUEUE_DEPTH,
                current_depth: 0,
                nr_hw_queues: 1,
                rotational: false,
                max_sectors: 1024,
                optimal_io_size: 8,
            },
            cfq_queues: Vec::new(),
            cfq_active_queue: None,
            cfq_current_time: 0,
            deadline_read_queue: Vec::new(),
            deadline_write_queue: Vec::new(),
            deadline_read_fifo: Vec::new(),
            deadline_write_fifo: Vec::new(),
            deadline_writes_starved: 0,
            deadline_write_starvation_limit: 2,
            deadline_last_direction: IoDirection::Read,
            bfq_entities: Vec::new(),
            bfq_active: None,
            bfq_virtual_time: 0,
            total_submitted: 0,
            total_completed: 0,
            total_merged: 0,
            total_read_sectors: 0,
            total_write_sectors: 0,
            read_latency_sum_ms: 0,
            write_latency_sum_ms: 0,
            tick_count: 0,
        }
    }

    /// Submit a new I/O request
    pub fn submit(&mut self, direction: IoDirection, sector_start: u64, sector_count: u32,
                  process_id: u32, priority_class: IoPriorityClass, priority_level: u8) -> u64 {
        let now = self.tick_count;
        let deadline = now + match direction {
            IoDirection::Read => DEADLINE_READ_MS,
            IoDirection::Write => DEADLINE_WRITE_MS,
            _ => DEADLINE_WRITE_MS,
        };

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let req = IoRequest {
            id,
            direction,
            sector_start,
            sector_count,
            priority_class,
            priority_level,
            process_id,
            submit_time: now,
            deadline,
            state: RequestState::Pending,
            data_ptr: 0,
            merged_count: 0,
        };

        // Try to merge with existing pending requests
        let merge = self.try_merge(&req);
        match merge {
            MergeResult::BackMerge | MergeResult::FrontMerge => {
                self.total_merged = self.total_merged.saturating_add(1);
                self.total_submitted = self.total_submitted.saturating_add(1);
                return id;
            }
            MergeResult::NoMerge => {}
        }

        let req_idx = self.requests.len();
        self.requests.push(req);
        self.total_submitted = self.total_submitted.saturating_add(1);

        // Insert into scheduler-specific queues
        match self.algorithm {
            IoScheduler::None => {}
            IoScheduler::Cfq => self.cfq_insert(req_idx, process_id, priority_class),
            IoScheduler::Deadline => self.deadline_insert(req_idx),
            IoScheduler::Bfq => self.bfq_insert(req_idx, process_id, priority_class),
        }

        id
    }

    /// Try to merge a new request with an existing pending request
    fn try_merge(&mut self, new_req: &IoRequest) -> MergeResult {
        for existing in &mut self.requests {
            if existing.state != RequestState::Pending { continue; }
            if existing.direction != new_req.direction { continue; }
            if existing.process_id != new_req.process_id { continue; }

            let existing_end = existing.sector_start + existing.sector_count as u64;
            let new_end = new_req.sector_start + new_req.sector_count as u64;

            // Back merge: new request follows existing
            if new_req.sector_start == existing_end {
                let combined = existing.sector_count as u64 + new_req.sector_count as u64;
                if combined <= self.device.max_sectors as u64 {
                    existing.sector_count = combined as u32;
                    existing.merged_count = existing.merged_count.saturating_add(1);
                    return MergeResult::BackMerge;
                }
            }

            // Front merge: new request precedes existing
            if new_end == existing.sector_start {
                let combined = existing.sector_count as u64 + new_req.sector_count as u64;
                if combined <= self.device.max_sectors as u64 {
                    existing.sector_start = new_req.sector_start;
                    existing.sector_count = combined as u32;
                    existing.merged_count = existing.merged_count.saturating_add(1);
                    return MergeResult::FrontMerge;
                }
            }

            // Proximity merge: within merge window
            let gap = if new_req.sector_start > existing_end {
                new_req.sector_start - existing_end
            } else if existing.sector_start > new_end {
                existing.sector_start - new_end
            } else {
                continue; // overlapping, skip
            };

            if gap <= MERGE_WINDOW_SECTORS {
                // Expand existing to cover both regions
                let merged_start = existing.sector_start.min(new_req.sector_start);
                let merged_end = existing_end.max(new_end);
                let merged_count = merged_end - merged_start;
                if merged_count <= self.device.max_sectors as u64 {
                    existing.sector_start = merged_start;
                    existing.sector_count = merged_count as u32;
                    existing.merged_count = existing.merged_count.saturating_add(1);
                    return MergeResult::BackMerge;
                }
            }
        }

        MergeResult::NoMerge
    }

    // --- CFQ ---

    fn cfq_insert(&mut self, req_idx: usize, pid: u32, prio: IoPriorityClass) {
        // Find or create per-process queue
        let queue_idx = self.cfq_queues.iter().position(|q| q.process_id == pid);
        match queue_idx {
            Some(idx) => {
                self.cfq_queues[idx].requests.push(req_idx);
            }
            None => {
                self.cfq_queues.push(CfqProcessQueue {
                    process_id: pid,
                    requests: vec![req_idx],
                    timeslice_start: 0,
                    timeslice_remaining_ms: CFQ_TIMESLICE_MS,
                    sectors_dispatched: 0,
                    weight: 100,
                    priority: prio,
                });
            }
        }
    }

    /// CFQ dispatch: round-robin per-process with timeslices
    fn cfq_dispatch(&mut self) -> Option<usize> {
        if self.cfq_queues.is_empty() { return None; }

        // Serve RT class first
        for queue in &mut self.cfq_queues {
            if queue.priority == IoPriorityClass::RealTime && !queue.requests.is_empty() {
                let req_idx = queue.requests.remove(0);
                return Some(req_idx);
            }
        }

        // Round-robin through best-effort queues
        let start = self.cfq_active_queue.unwrap_or(0);
        let n = self.cfq_queues.len();
        for i in 0..n {
            let idx = (start + i) % n;
            let queue = &mut self.cfq_queues[idx];
            if queue.priority == IoPriorityClass::Idle { continue; }
            if queue.requests.is_empty() { continue; }

            if queue.timeslice_remaining_ms > 0 {
                let req_idx = queue.requests.remove(0);
                queue.timeslice_remaining_ms = queue.timeslice_remaining_ms.saturating_sub(1);
                self.cfq_active_queue = Some(idx);
                return Some(req_idx);
            } else {
                // Timeslice expired, refill and move to next
                queue.timeslice_remaining_ms = CFQ_TIMESLICE_MS;
                queue.sectors_dispatched = 0;
            }
        }

        // Idle class: only when all others are empty
        for queue in &mut self.cfq_queues {
            if queue.priority == IoPriorityClass::Idle && !queue.requests.is_empty() {
                return Some(queue.requests.remove(0));
            }
        }

        None
    }

    // --- Deadline ---

    fn deadline_insert(&mut self, req_idx: usize) {
        let direction = self.requests[req_idx].direction;
        let sector = self.requests[req_idx].sector_start;
        let deadline = self.requests[req_idx].deadline;

        match direction {
            IoDirection::Read => {
                // Insert sorted by sector
                let pos = self.deadline_read_queue.iter()
                    .position(|&i| self.requests[i].sector_start > sector)
                    .unwrap_or(self.deadline_read_queue.len());
                self.deadline_read_queue.insert(pos, req_idx);

                // Insert sorted by deadline
                let pos = self.deadline_read_fifo.iter()
                    .position(|&i| self.requests[i].deadline > deadline)
                    .unwrap_or(self.deadline_read_fifo.len());
                self.deadline_read_fifo.insert(pos, req_idx);
            }
            IoDirection::Write | _ => {
                let pos = self.deadline_write_queue.iter()
                    .position(|&i| self.requests[i].sector_start > sector)
                    .unwrap_or(self.deadline_write_queue.len());
                self.deadline_write_queue.insert(pos, req_idx);

                let pos = self.deadline_write_fifo.iter()
                    .position(|&i| self.requests[i].deadline > deadline)
                    .unwrap_or(self.deadline_write_fifo.len());
                self.deadline_write_fifo.insert(pos, req_idx);
            }
        }
    }

    /// Deadline dispatch: check for expired deadlines, then serve in sector order
    fn deadline_dispatch(&mut self) -> Option<usize> {
        let now = self.tick_count;

        // Check for expired read deadlines
        if let Some(&req_idx) = self.deadline_read_fifo.first() {
            if self.requests[req_idx].deadline <= now {
                self.deadline_read_fifo.remove(0);
                self.deadline_read_queue.retain(|&i| i != req_idx);
                self.deadline_last_direction = IoDirection::Read;
                self.deadline_writes_starved = 0;
                return Some(req_idx);
            }
        }

        // Check for expired write deadlines
        if let Some(&req_idx) = self.deadline_write_fifo.first() {
            if self.requests[req_idx].deadline <= now {
                self.deadline_write_fifo.remove(0);
                self.deadline_write_queue.retain(|&i| i != req_idx);
                self.deadline_last_direction = IoDirection::Write;
                return Some(req_idx);
            }
        }

        // No deadlines expired: serve in sector order, alternating direction
        // Prevent write starvation
        if self.deadline_writes_starved >= self.deadline_write_starvation_limit {
            if let Some(req_idx) = self.deadline_write_queue.first().copied() {
                self.deadline_write_queue.remove(0);
                self.deadline_write_fifo.retain(|&i| i != req_idx);
                self.deadline_last_direction = IoDirection::Write;
                self.deadline_writes_starved = 0;
                return Some(req_idx);
            }
        }

        // Prefer reads
        if let Some(req_idx) = self.deadline_read_queue.first().copied() {
            self.deadline_read_queue.remove(0);
            self.deadline_read_fifo.retain(|&i| i != req_idx);
            self.deadline_last_direction = IoDirection::Read;
            self.deadline_writes_starved = self.deadline_writes_starved.saturating_add(1);
            return Some(req_idx);
        }

        // Fall back to writes
        if let Some(req_idx) = self.deadline_write_queue.first().copied() {
            self.deadline_write_queue.remove(0);
            self.deadline_write_fifo.retain(|&i| i != req_idx);
            self.deadline_last_direction = IoDirection::Write;
            self.deadline_writes_starved = 0;
            return Some(req_idx);
        }

        None
    }

    // --- BFQ ---

    fn bfq_insert(&mut self, req_idx: usize, pid: u32, prio: IoPriorityClass) {
        let entity_idx = self.bfq_entities.iter().position(|e| e.process_id == pid);
        match entity_idx {
            Some(idx) => {
                self.bfq_entities[idx].requests.push(req_idx);
            }
            None => {
                self.bfq_entities.push(BfqEntity {
                    process_id: pid,
                    budget: BFQ_DEFAULT_BUDGET,
                    max_budget: BFQ_DEFAULT_BUDGET,
                    weight: 100,
                    priority: prio,
                    requests: vec![req_idx],
                    virtual_time: self.bfq_virtual_time,
                    total_sectors: 0,
                });
            }
        }
    }

    /// BFQ dispatch: proportional bandwidth via virtual time + budgets
    fn bfq_dispatch(&mut self) -> Option<usize> {
        if self.bfq_entities.is_empty() { return None; }

        // RT class first
        for entity in &mut self.bfq_entities {
            if entity.priority == IoPriorityClass::RealTime && !entity.requests.is_empty() {
                let req_idx = entity.requests.remove(0);
                let sectors = self.requests[req_idx].sector_count;
                entity.total_sectors += sectors as u64;
                return Some(req_idx);
            }
        }

        // Find entity with lowest virtual time that has budget remaining
        let mut best_idx: Option<usize> = None;
        let mut best_vt = i32::MAX;

        for (i, entity) in self.bfq_entities.iter().enumerate() {
            if entity.requests.is_empty() { continue; }
            if entity.priority == IoPriorityClass::Idle { continue; }
            if entity.budget == 0 { continue; }
            if entity.virtual_time < best_vt {
                best_vt = entity.virtual_time;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            let req_idx = self.bfq_entities[idx].requests.remove(0);
            let sectors = self.requests[req_idx].sector_count;
            let entity = &mut self.bfq_entities[idx];

            entity.budget = entity.budget.saturating_sub(sectors);
            entity.total_sectors += sectors as u64;

            // Advance virtual time: cost = sectors / weight (in Q16)
            let cost = q16_div(sectors as i32, entity.weight as i32);
            entity.virtual_time += cost;

            // Replenish budget if exhausted
            if entity.budget == 0 {
                entity.budget = entity.max_budget;
                // Adaptive budget: increase if entity consistently uses full budget
                if entity.total_sectors > entity.max_budget as u64 * 4 {
                    entity.max_budget = (entity.max_budget * 3 / 2).min(256);
                }
            }

            // Update global virtual time
            self.bfq_virtual_time = self.bfq_virtual_time.max(entity.virtual_time);

            return Some(req_idx);
        }

        // Idle class fallback
        for entity in &mut self.bfq_entities {
            if entity.priority == IoPriorityClass::Idle && !entity.requests.is_empty() {
                let req_idx = entity.requests.remove(0);
                entity.total_sectors += self.requests[req_idx].sector_count as u64;
                return Some(req_idx);
            }
        }

        None
    }

    /// Dispatch the next request according to the active scheduler
    pub fn dispatch(&mut self) -> Option<u64> {
        let req_idx = match self.algorithm {
            IoScheduler::None => {
                self.requests.iter().position(|r| r.state == RequestState::Pending)
            }
            IoScheduler::Cfq => self.cfq_dispatch(),
            IoScheduler::Deadline => self.deadline_dispatch(),
            IoScheduler::Bfq => self.bfq_dispatch(),
        };

        if let Some(idx) = req_idx {
            if idx < self.requests.len() {
                self.requests[idx].state = RequestState::Dispatched;
                self.device.current_depth = self.device.current_depth.saturating_add(1);
                let id = self.requests[idx].id;

                match self.requests[idx].direction {
                    IoDirection::Read => self.total_read_sectors += self.requests[idx].sector_count as u64,
                    IoDirection::Write => self.total_write_sectors += self.requests[idx].sector_count as u64,
                    _ => {}
                }

                return Some(id);
            }
        }
        None
    }

    /// Mark a request as completed
    pub fn complete(&mut self, request_id: u64) {
        if let Some(req) = self.requests.iter_mut().find(|r| r.id == request_id) {
            let latency = self.tick_count.saturating_sub(req.submit_time);
            match req.direction {
                IoDirection::Read => self.read_latency_sum_ms += latency,
                IoDirection::Write => self.write_latency_sum_ms += latency,
                _ => {}
            }
            req.state = RequestState::Completed;
            self.total_completed = self.total_completed.saturating_add(1);
            self.device.current_depth = self.device.current_depth.saturating_sub(1);
        }

        // Garbage collect completed requests periodically
        if self.total_completed % 128 == 0 {
            self.requests.retain(|r| r.state != RequestState::Completed);
        }
    }

    /// Set the scheduling algorithm
    pub fn set_algorithm(&mut self, algo: IoScheduler) {
        serial_println!("    [io_sched] Switching from {:?} to {:?}", self.algorithm, algo);
        self.algorithm = algo;

        // Clear scheduler-specific queues
        self.cfq_queues.clear();
        self.cfq_active_queue = None;
        self.deadline_read_queue.clear();
        self.deadline_write_queue.clear();
        self.deadline_read_fifo.clear();
        self.deadline_write_fifo.clear();
        self.bfq_entities.clear();
        self.bfq_active = None;

        // Re-insert pending requests into new scheduler
        let pending_indices: Vec<usize> = self.requests.iter()
            .enumerate()
            .filter(|(_, r)| r.state == RequestState::Pending)
            .map(|(i, _)| i)
            .collect();

        for idx in pending_indices {
            let pid = self.requests[idx].process_id;
            let prio = self.requests[idx].priority_class;
            match algo {
                IoScheduler::None => {}
                IoScheduler::Cfq => self.cfq_insert(idx, pid, prio),
                IoScheduler::Deadline => self.deadline_insert(idx),
                IoScheduler::Bfq => self.bfq_insert(idx, pid, prio),
            }
        }
    }

    /// Get statistics summary
    pub fn summary(&self) -> IoSchedSummary {
        let avg_read_lat = if self.total_read_sectors > 0 {
            self.read_latency_sum_ms / self.total_completed.max(1)
        } else { 0 };
        let avg_write_lat = if self.total_write_sectors > 0 {
            self.write_latency_sum_ms / self.total_completed.max(1)
        } else { 0 };

        IoSchedSummary {
            algorithm: self.algorithm,
            pending: self.requests.iter().filter(|r| r.state == RequestState::Pending).count() as u32,
            dispatched: self.device.current_depth as u32,
            total_submitted: self.total_submitted,
            total_completed: self.total_completed,
            total_merged: self.total_merged,
            avg_read_latency_ms: avg_read_lat,
            avg_write_latency_ms: avg_write_lat,
            queue_depth: self.device.current_depth as u32,
            max_queue_depth: self.device.max_depth as u32,
        }
    }

    /// Tick: advance time
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);
    }
}

/// I/O scheduler summary
#[derive(Debug, Clone)]
pub struct IoSchedSummary {
    pub algorithm: IoScheduler,
    pub pending: u32,
    pub dispatched: u32,
    pub total_submitted: u64,
    pub total_completed: u64,
    pub total_merged: u64,
    pub avg_read_latency_ms: u64,
    pub avg_write_latency_ms: u64,
    pub queue_depth: u32,
    pub max_queue_depth: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static IO_SCHED: Mutex<Option<IoSchedulerState>> = Mutex::new(None);

pub fn init() {
    let mut sched = IoSchedulerState::new();

    // Auto-select algorithm based on device type
    // SSDs benefit from BFQ or none; HDDs benefit from CFQ or deadline
    if sched.device.rotational {
        sched.algorithm = IoScheduler::Deadline;
        serial_println!("    [io_sched] Rotational device -> Deadline scheduler");
    } else {
        sched.algorithm = IoScheduler::Bfq;
        serial_println!("    [io_sched] SSD device -> BFQ scheduler");
    }

    serial_println!("    [io_sched] Queue depth: {}, HW queues: {}, max sectors: {}",
        sched.device.max_depth, sched.device.nr_hw_queues, sched.device.max_sectors);

    *IO_SCHED.lock() = Some(sched);
    serial_println!("    [io_sched] I/O scheduler ready (CFQ, Deadline, BFQ, merging)");
}

/// Submit an I/O request
pub fn submit(dir: IoDirection, sector: u64, count: u32, pid: u32,
              prio: IoPriorityClass, level: u8) -> Option<u64> {
    IO_SCHED.lock().as_mut().map(|s| s.submit(dir, sector, count, pid, prio, level))
}

/// Dispatch next request
pub fn dispatch() -> Option<u64> {
    IO_SCHED.lock().as_mut().and_then(|s| s.dispatch())
}

/// Complete a request
pub fn complete(id: u64) {
    if let Some(ref mut s) = *IO_SCHED.lock() {
        s.complete(id);
    }
}

/// Set scheduler algorithm
pub fn set_algorithm(algo: IoScheduler) {
    if let Some(ref mut s) = *IO_SCHED.lock() {
        s.set_algorithm(algo);
    }
}

/// Tick
pub fn tick() {
    if let Some(ref mut s) = *IO_SCHED.lock() {
        s.tick();
    }
}

/// Get summary
pub fn summary() -> Option<IoSchedSummary> {
    IO_SCHED.lock().as_ref().map(|s| s.summary())
}
