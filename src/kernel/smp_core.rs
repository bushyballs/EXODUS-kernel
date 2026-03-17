/// SMP Core — Advanced SMP infrastructure for Genesis kernel
///
/// Extends the base SMP support with:
/// - Per-CPU run queue abstraction for the scheduler
/// - IPI message passing framework (typed inter-processor messages)
/// - Per-CPU work queues (deferred work items)
/// - Cross-CPU function calls (run a closure on another CPU)
/// - CPU load balancing helpers
/// - TLB shootdown protocol
///
/// The base SMP module (crate::smp) provides LAPIC, IPI delivery,
/// per-CPU data, and AP boot. This module builds higher-level
/// abstractions on top of that foundation.
///
/// Inspired by: Linux kernel/smp.c, kernel/sched/core.c. All code original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum CPUs (must match crate::smp::MAX_CPUS)
const MAX_CPUS: usize = 16;

/// Maximum pending IPI messages per CPU
const MAX_IPI_QUEUE: usize = 64;

/// Maximum work items per CPU work queue
const MAX_WORK_ITEMS: usize = 128;

/// Maximum pending function call requests per CPU
const MAX_CALL_QUEUE: usize = 32;

/// IPI vector for inter-processor messages
const IPI_VECTOR_MSG: u8 = 0xF0;

/// IPI vector for TLB shootdown
const IPI_VECTOR_TLB: u8 = 0xF1;

/// IPI vector for scheduler rebalance
const IPI_VECTOR_RESCHED: u8 = 0xF2;

/// IPI vector for remote function call
const IPI_VECTOR_CALL: u8 = 0xF3;

/// IPI vector for stop CPU (halt)
const IPI_VECTOR_STOP: u8 = 0xF4;

// ---------------------------------------------------------------------------
// IPI Message types
// ---------------------------------------------------------------------------

/// Types of inter-processor interrupt messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpiMessageType {
    /// Request target CPU to reschedule
    Reschedule,
    /// TLB shootdown: invalidate page at given address
    TlbShootdown,
    /// TLB shootdown: invalidate all TLB entries
    TlbFlushAll,
    /// Stop the target CPU (for shutdown or crash handling)
    StopCpu,
    /// Execute a function on the target CPU
    CallFunction,
    /// Timer synchronization
    TimerSync,
    /// Performance counter sampling request
    PerfSample,
    /// Generic message with u64 payload
    Generic,
}

/// An IPI message queued for delivery
#[derive(Clone)]
pub struct IpiMessage {
    /// Source CPU index
    pub from_cpu: u32,
    /// Message type
    pub msg_type: IpiMessageType,
    /// Payload (address, value, or opaque data depending on message type)
    pub arg0: u64,
    pub arg1: u64,
    /// Sequence number for ordering
    pub seq: u64,
    /// Timestamp when message was sent (ms)
    pub sent_ms: u64,
    /// Whether sender is waiting for completion
    pub needs_ack: bool,
}

// ---------------------------------------------------------------------------
// Per-CPU run queue entry
// ---------------------------------------------------------------------------

/// A process entry in the per-CPU run queue
#[derive(Debug, Clone)]
pub struct RunQueueEntry {
    /// Process ID
    pub pid: u32,
    /// Priority (lower = higher priority, 0 = highest)
    pub priority: u32,
    /// Virtual runtime for CFS-like scheduling (nanoseconds)
    pub vruntime: u64,
    /// Time slice remaining (microseconds)
    pub time_slice_us: u64,
    /// Whether the task is runnable (not blocked)
    pub runnable: bool,
    /// cgroup CPU weight (from cgroups controller)
    pub cgroup_weight: u32,
    /// Last time this task was scheduled (ms)
    pub last_scheduled_ms: u64,
}

// ---------------------------------------------------------------------------
// Per-CPU work item
// ---------------------------------------------------------------------------

/// A deferred work item for per-CPU processing
#[derive(Clone)]
pub struct WorkItem {
    /// Unique work ID
    pub id: u32,
    /// Name/description
    pub name: String,
    /// Function pointer to execute
    pub func_addr: usize,
    /// Argument to pass to the function
    pub arg: u64,
    /// Whether this is a one-shot or recurring item
    pub recurring: bool,
    /// Interval for recurring work (ms), 0 = one-shot
    pub interval_ms: u64,
    /// Next scheduled execution time (ms)
    pub next_run_ms: u64,
    /// Number of times executed
    pub run_count: u64,
}

// ---------------------------------------------------------------------------
// Per-CPU call function request
// ---------------------------------------------------------------------------

/// A request to execute a function on a specific CPU
#[derive(Clone)]
struct CallRequest {
    /// Source CPU that requested the call
    from_cpu: u32,
    /// Function address to call
    func_addr: usize,
    /// Argument
    arg: u64,
    /// Whether the call has been completed
    completed: bool,
    /// Return value after completion
    result: u64,
}

// ---------------------------------------------------------------------------
// Per-CPU state for SMP core
// ---------------------------------------------------------------------------

/// Extended per-CPU state managed by smp_core
struct PerCpuState {
    /// IPI message inbox
    ipi_inbox: Vec<IpiMessage>,
    /// Run queue for this CPU
    run_queue: Vec<RunQueueEntry>,
    /// Deferred work queue
    work_queue: Vec<WorkItem>,
    /// Pending remote function call requests
    call_queue: Vec<CallRequest>,
    /// Number of IPIs received
    ipis_received: u64,
    /// Number of IPIs sent
    ipis_sent: u64,
    /// Number of TLB shootdowns received
    tlb_shootdowns: u64,
    /// Number of reschedule IPIs received
    reschedules: u64,
    /// Current load metric (number of runnable tasks)
    load: u32,
    /// Cumulative CPU time for load averaging (us)
    cpu_time_us: u64,
    /// Idle time (us)
    idle_time_us: u64,
    /// Whether this CPU is in idle state
    is_idle: bool,
    /// Whether this CPU needs to reschedule
    need_resched: bool,
    /// Next work item ID for this CPU
    next_work_id: u32,
}

impl PerCpuState {
    const fn new() -> Self {
        PerCpuState {
            ipi_inbox: Vec::new(),
            run_queue: Vec::new(),
            work_queue: Vec::new(),
            call_queue: Vec::new(),
            ipis_received: 0,
            ipis_sent: 0,
            tlb_shootdowns: 0,
            reschedules: 0,
            load: 0,
            cpu_time_us: 0,
            idle_time_us: 0,
            is_idle: false,
            need_resched: false,
            next_work_id: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// TLB shootdown state
// ---------------------------------------------------------------------------

/// TLB shootdown tracking
struct TlbShootdownState {
    /// Address to invalidate (0 = flush all)
    addr: u64,
    /// Number of CPUs that have acknowledged
    ack_count: AtomicU32,
    /// Number of CPUs that need to acknowledge
    target_count: u32,
    /// Whether a shootdown is in progress
    in_progress: AtomicBool,
}

// ---------------------------------------------------------------------------
// SMP Core subsystem
// ---------------------------------------------------------------------------

struct SmpCoreSubsystem {
    /// Per-CPU state
    per_cpu: Vec<PerCpuState>,
    /// Number of CPUs
    num_cpus: usize,
    /// Global IPI sequence counter
    ipi_seq: u64,
    /// TLB shootdown state
    tlb_state: TlbShootdownState,
    /// Total IPIs sent across all CPUs
    total_ipis_sent: u64,
    /// Total IPIs received across all CPUs
    total_ipis_received: u64,
    /// Whether load balancing is enabled
    load_balance_enabled: bool,
    /// Load balance interval (ms)
    load_balance_interval_ms: u64,
    /// Last load balance timestamp (ms)
    last_balance_ms: u64,
}

impl SmpCoreSubsystem {
    const fn new() -> Self {
        SmpCoreSubsystem {
            per_cpu: Vec::new(),
            num_cpus: 0,
            ipi_seq: 0,
            tlb_state: TlbShootdownState {
                addr: 0,
                ack_count: AtomicU32::new(0),
                target_count: 0,
                in_progress: AtomicBool::new(false),
            },
            total_ipis_sent: 0,
            total_ipis_received: 0,
            load_balance_enabled: true,
            load_balance_interval_ms: 100, // rebalance every 100ms
            last_balance_ms: 0,
        }
    }

    fn init(&mut self, ncpus: usize) {
        self.num_cpus = ncpus;
        for _ in 0..ncpus {
            self.per_cpu.push(PerCpuState::new());
        }
    }

    // ------- IPI message sending -------

    /// Send an IPI message to a specific CPU
    fn send_ipi_message(
        &mut self,
        target_cpu: u32,
        msg_type: IpiMessageType,
        arg0: u64,
        arg1: u64,
        needs_ack: bool,
    ) -> bool {
        let target = target_cpu as usize;
        if target >= self.num_cpus {
            return false;
        }

        let from_cpu = crate::smp::current_cpu();
        let now = crate::time::clock::uptime_ms();

        self.ipi_seq += 1;
        let msg = IpiMessage {
            from_cpu,
            msg_type,
            arg0,
            arg1,
            seq: self.ipi_seq,
            sent_ms: now,
            needs_ack,
        };

        if self.per_cpu[target].ipi_inbox.len() >= MAX_IPI_QUEUE {
            return false; // inbox full
        }

        self.per_cpu[target].ipi_inbox.push(msg);
        self.total_ipis_sent = self.total_ipis_sent.saturating_add(1);

        if from_cpu as usize <= self.num_cpus {
            self.per_cpu[from_cpu as usize].ipis_sent =
                self.per_cpu[from_cpu as usize].ipis_sent.saturating_add(1);
        }

        // Send the actual hardware IPI based on message type
        let vector = match msg_type {
            IpiMessageType::Reschedule => IPI_VECTOR_RESCHED,
            IpiMessageType::TlbShootdown | IpiMessageType::TlbFlushAll => IPI_VECTOR_TLB,
            IpiMessageType::CallFunction => IPI_VECTOR_CALL,
            IpiMessageType::StopCpu => IPI_VECTOR_STOP,
            _ => IPI_VECTOR_MSG,
        };

        crate::smp::send_ipi_fixed(target_cpu, vector);
        true
    }

    /// Send an IPI message to all other CPUs
    fn send_ipi_all_others(&mut self, msg_type: IpiMessageType, arg0: u64, arg1: u64) -> u32 {
        let my_cpu = crate::smp::current_cpu();
        let mut sent: u32 = 0;

        for i in 0..self.num_cpus {
            if i as u32 != my_cpu {
                if self.send_ipi_message(i as u32, msg_type, arg0, arg1, false) {
                    sent += 1;
                }
            }
        }
        sent
    }

    /// Process pending IPI messages on the current CPU
    fn process_ipi_messages(&mut self) -> u32 {
        let cpu = crate::smp::current_cpu() as usize;
        if cpu >= self.num_cpus {
            return 0;
        }

        let messages: Vec<IpiMessage> = core::mem::take(&mut self.per_cpu[cpu].ipi_inbox);
        let count = messages.len() as u32;

        for msg in &messages {
            self.per_cpu[cpu].ipis_received = self.per_cpu[cpu].ipis_received.saturating_add(1);
            self.total_ipis_received = self.total_ipis_received.saturating_add(1);

            match msg.msg_type {
                IpiMessageType::Reschedule => {
                    self.per_cpu[cpu].reschedules = self.per_cpu[cpu].reschedules.saturating_add(1);
                    self.per_cpu[cpu].need_resched = true;
                }
                IpiMessageType::TlbShootdown => {
                    // Invalidate specific TLB entry
                    unsafe {
                        core::arch::asm!(
                            "invlpg [{}]",
                            in(reg) msg.arg0,
                            options(nostack, preserves_flags),
                        );
                    }
                    self.per_cpu[cpu].tlb_shootdowns =
                        self.per_cpu[cpu].tlb_shootdowns.saturating_add(1);
                    self.tlb_state.ack_count.fetch_add(1, Ordering::Release);
                }
                IpiMessageType::TlbFlushAll => {
                    // Flush entire TLB by reloading CR3
                    unsafe {
                        let cr3: u64;
                        core::arch::asm!("mov {}, cr3", out(reg) cr3);
                        core::arch::asm!("mov cr3, {}", in(reg) cr3);
                    }
                    self.per_cpu[cpu].tlb_shootdowns =
                        self.per_cpu[cpu].tlb_shootdowns.saturating_add(1);
                    self.tlb_state.ack_count.fetch_add(1, Ordering::Release);
                }
                IpiMessageType::StopCpu => {
                    crate::serial_println!(
                        "  [smp_core] CPU {} received STOP IPI from CPU {}",
                        cpu,
                        msg.from_cpu
                    );
                    // In a real implementation, this would halt the CPU
                }
                IpiMessageType::CallFunction => {
                    // Process pending call requests
                    self.process_call_requests(cpu);
                }
                _ => {}
            }
        }

        count
    }

    // ------- TLB shootdown -------

    /// Initiate a TLB shootdown for a specific address across all CPUs
    fn tlb_shootdown(&mut self, addr: u64) {
        // Wait for any in-progress shootdown
        while self.tlb_state.in_progress.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        self.tlb_state.in_progress.store(true, Ordering::Release);
        self.tlb_state.addr = addr;
        self.tlb_state.ack_count.store(0, Ordering::Release);
        self.tlb_state.target_count = (self.num_cpus as u32).saturating_sub(1);

        // Invalidate on current CPU first
        if addr == 0 {
            unsafe {
                let cr3: u64;
                core::arch::asm!("mov {}, cr3", out(reg) cr3);
                core::arch::asm!("mov cr3, {}", in(reg) cr3);
            }
        } else {
            unsafe {
                core::arch::asm!(
                    "invlpg [{}]",
                    in(reg) addr,
                    options(nostack, preserves_flags),
                );
            }
        }

        // Send shootdown IPI to all other CPUs
        let msg_type = if addr == 0 {
            IpiMessageType::TlbFlushAll
        } else {
            IpiMessageType::TlbShootdown
        };
        self.send_ipi_all_others(msg_type, addr, 0);

        // Wait for all CPUs to acknowledge
        let mut timeout = 10_000u32; // spin limit
        while self.tlb_state.ack_count.load(Ordering::Acquire) < self.tlb_state.target_count {
            timeout -= 1;
            if timeout == 0 {
                crate::serial_println!(
                    "  [smp_core] TLB shootdown timeout! ack={}/{}",
                    self.tlb_state.ack_count.load(Ordering::Relaxed),
                    self.tlb_state.target_count
                );
                break;
            }
            core::hint::spin_loop();
        }

        self.tlb_state.in_progress.store(false, Ordering::Release);
    }

    // ------- Run queue management -------

    /// Add a task to a CPU's run queue
    fn enqueue_task(&mut self, cpu: u32, entry: RunQueueEntry) -> bool {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return false;
        }
        self.per_cpu[idx].run_queue.push(entry);
        self.per_cpu[idx].load = self.per_cpu[idx]
            .run_queue
            .iter()
            .filter(|e| e.runnable)
            .count() as u32;
        true
    }

    /// Remove a task from a CPU's run queue
    fn dequeue_task(&mut self, cpu: u32, pid: u32) -> Option<RunQueueEntry> {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return None;
        }
        let pos = self.per_cpu[idx]
            .run_queue
            .iter()
            .position(|e| e.pid == pid)?;
        let entry = self.per_cpu[idx].run_queue.remove(pos);
        self.per_cpu[idx].load = self.per_cpu[idx]
            .run_queue
            .iter()
            .filter(|e| e.runnable)
            .count() as u32;
        Some(entry)
    }

    /// Pick the next task to run on a CPU (lowest vruntime among runnable tasks)
    fn pick_next_task(&mut self, cpu: u32) -> Option<u32> {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return None;
        }

        let rq = &self.per_cpu[idx].run_queue;
        rq.iter()
            .filter(|e| e.runnable)
            .min_by_key(|e| e.vruntime)
            .map(|e| e.pid)
    }

    /// Update a task's vruntime after running for a time slice
    fn update_vruntime(&mut self, cpu: u32, pid: u32, delta_us: u64) {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return;
        }

        if let Some(entry) = self.per_cpu[idx]
            .run_queue
            .iter_mut()
            .find(|e| e.pid == pid)
        {
            // Weight-adjusted vruntime: higher weight = slower vruntime growth
            // Default weight is 1024. Formula: vruntime += delta * (1024 / weight)
            let weight = if entry.cgroup_weight > 0 {
                entry.cgroup_weight
            } else {
                1024
            };
            let adjusted_delta = (delta_us * 1024) / weight as u64;
            entry.vruntime = entry.vruntime.wrapping_add(adjusted_delta);
            entry.time_slice_us = entry.time_slice_us.saturating_sub(delta_us);
            entry.last_scheduled_ms = crate::time::clock::uptime_ms();
        }
    }

    /// Set a task's runnable state
    fn set_runnable(&mut self, cpu: u32, pid: u32, runnable: bool) {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return;
        }
        if let Some(entry) = self.per_cpu[idx]
            .run_queue
            .iter_mut()
            .find(|e| e.pid == pid)
        {
            entry.runnable = runnable;
        }
        self.per_cpu[idx].load = self.per_cpu[idx]
            .run_queue
            .iter()
            .filter(|e| e.runnable)
            .count() as u32;
    }

    // ------- Load balancing -------

    /// Find the busiest and least busy CPUs
    fn find_busiest_and_idle(&self) -> (Option<u32>, Option<u32>) {
        let mut busiest: Option<(u32, u32)> = None; // (cpu, load)
        let mut idlest: Option<(u32, u32)> = None;

        for i in 0..self.num_cpus {
            let load = self.per_cpu[i].load;

            match busiest {
                Some((_, max_load)) if load > max_load => busiest = Some((i as u32, load)),
                None => busiest = Some((i as u32, load)),
                _ => {}
            }

            match idlest {
                Some((_, min_load)) if load < min_load => idlest = Some((i as u32, load)),
                None => idlest = Some((i as u32, load)),
                _ => {}
            }
        }

        (busiest.map(|(c, _)| c), idlest.map(|(c, _)| c))
    }

    /// Attempt to balance load by migrating tasks between CPUs
    fn balance_load(&mut self) -> u32 {
        if !self.load_balance_enabled || self.num_cpus < 2 {
            return 0;
        }

        let now = crate::time::clock::uptime_ms();
        if now.saturating_sub(self.last_balance_ms) < self.load_balance_interval_ms {
            return 0;
        }
        self.last_balance_ms = now;

        let (busiest, idlest) = self.find_busiest_and_idle();
        let (busiest_cpu, idlest_cpu) = match (busiest, idlest) {
            (Some(b), Some(i)) if b != i => (b, i),
            _ => return 0,
        };

        let b_idx = busiest_cpu as usize;
        let i_idx = idlest_cpu as usize;

        let b_load = self.per_cpu[b_idx].load;
        let i_load = self.per_cpu[i_idx].load;

        // Only balance if imbalance is significant (at least 2 tasks difference)
        if b_load <= i_load + 1 {
            return 0;
        }

        // Number of tasks to migrate (move half the imbalance)
        let to_migrate = ((b_load - i_load) / 2).max(1);
        let mut migrated: u32 = 0;

        for _ in 0..to_migrate {
            // Find a migratable task on the busiest CPU (lowest priority = most eligible)
            let task_pid = {
                let rq = &self.per_cpu[b_idx].run_queue;
                rq.iter()
                    .filter(|e| e.runnable)
                    .max_by_key(|e| e.priority) // highest priority number = lowest actual priority
                    .map(|e| e.pid)
            };

            if let Some(pid) = task_pid {
                if let Some(entry) = self.dequeue_task(busiest_cpu, pid) {
                    self.enqueue_task(idlest_cpu, entry);
                    migrated += 1;
                }
            } else {
                break;
            }
        }

        if migrated > 0 {
            // Send reschedule IPI to both CPUs
            self.send_ipi_message(busiest_cpu, IpiMessageType::Reschedule, 0, 0, false);
            self.send_ipi_message(idlest_cpu, IpiMessageType::Reschedule, 0, 0, false);
        }

        migrated
    }

    // ------- Work queue management -------

    /// Add a work item to a CPU's work queue
    fn queue_work(
        &mut self,
        cpu: u32,
        name: &str,
        func_addr: usize,
        arg: u64,
        recurring: bool,
        interval_ms: u64,
    ) -> u32 {
        let idx = cpu as usize;
        if idx >= self.num_cpus || self.per_cpu[idx].work_queue.len() >= MAX_WORK_ITEMS {
            return 0;
        }

        let id = self.per_cpu[idx].next_work_id;
        self.per_cpu[idx].next_work_id = self.per_cpu[idx].next_work_id.saturating_add(1);

        let now = crate::time::clock::uptime_ms();
        let item = WorkItem {
            id,
            name: String::from(name),
            func_addr,
            arg,
            recurring,
            interval_ms,
            next_run_ms: now + interval_ms,
            run_count: 0,
        };

        self.per_cpu[idx].work_queue.push(item);
        id
    }

    /// Cancel a work item on a CPU
    fn cancel_work(&mut self, cpu: u32, work_id: u32) -> bool {
        let idx = cpu as usize;
        if idx >= self.num_cpus {
            return false;
        }
        let pos = self.per_cpu[idx]
            .work_queue
            .iter()
            .position(|w| w.id == work_id);
        if let Some(p) = pos {
            self.per_cpu[idx].work_queue.remove(p);
            true
        } else {
            false
        }
    }

    /// Process due work items on the current CPU
    fn process_work_queue(&mut self) -> u32 {
        let cpu = crate::smp::current_cpu() as usize;
        if cpu >= self.num_cpus {
            return 0;
        }

        let now = crate::time::clock::uptime_ms();
        let mut executed: u32 = 0;
        let mut to_remove: Vec<u32> = Vec::new();

        for item in &mut self.per_cpu[cpu].work_queue {
            if now >= item.next_run_ms {
                // Execute the work function
                let func: fn(u64) = unsafe { core::mem::transmute(item.func_addr) };
                func(item.arg);

                item.run_count = item.run_count.saturating_add(1);
                executed += 1;

                if item.recurring && item.interval_ms > 0 {
                    item.next_run_ms = now + item.interval_ms;
                } else {
                    to_remove.push(item.id);
                }
            }
        }

        // Remove completed one-shot items
        for id in &to_remove {
            self.per_cpu[cpu].work_queue.retain(|w| w.id != *id);
        }

        executed
    }

    // ------- Remote function calls -------

    /// Request a function to be executed on a specific CPU
    fn call_function_on(&mut self, target_cpu: u32, func_addr: usize, arg: u64) -> bool {
        let target = target_cpu as usize;
        if target >= self.num_cpus {
            return false;
        }

        if self.per_cpu[target].call_queue.len() >= MAX_CALL_QUEUE {
            return false;
        }

        let from_cpu = crate::smp::current_cpu();
        let req = CallRequest {
            from_cpu,
            func_addr,
            arg,
            completed: false,
            result: 0,
        };

        self.per_cpu[target].call_queue.push(req);
        self.send_ipi_message(target_cpu, IpiMessageType::CallFunction, 0, 0, false);
        true
    }

    /// Process pending remote function calls on the current CPU
    fn process_call_requests(&mut self, cpu: usize) {
        if cpu >= self.num_cpus {
            return;
        }

        let requests: Vec<CallRequest> = core::mem::take(&mut self.per_cpu[cpu].call_queue);

        for req in requests {
            let func: fn(u64) -> u64 = unsafe { core::mem::transmute(req.func_addr) };
            let _result = func(req.arg);
            // In a full implementation, we'd write the result back to shared memory
            // and signal the caller. For now, fire-and-forget.
        }
    }

    // ------- CPU idle tracking -------

    /// Mark a CPU as entering idle state
    fn cpu_enter_idle(&mut self, cpu: u32) {
        let idx = cpu as usize;
        if idx < self.num_cpus {
            self.per_cpu[idx].is_idle = true;
        }
    }

    /// Mark a CPU as leaving idle state
    fn cpu_exit_idle(&mut self, cpu: u32) {
        let idx = cpu as usize;
        if idx < self.num_cpus {
            self.per_cpu[idx].is_idle = false;
        }
    }

    /// Account CPU time for load tracking
    fn account_cpu_time(&mut self, cpu: u32, active_us: u64, idle_us: u64) {
        let idx = cpu as usize;
        if idx < self.num_cpus {
            self.per_cpu[idx].cpu_time_us += active_us;
            self.per_cpu[idx].idle_time_us += idle_us;
        }
    }

    // ------- Status and statistics -------

    /// Get per-CPU load information
    fn get_loads(&self) -> Vec<(u32, u32, u64, u64, bool)> {
        self.per_cpu
            .iter()
            .enumerate()
            .map(|(i, pc)| {
                (
                    i as u32,
                    pc.load,
                    pc.cpu_time_us,
                    pc.idle_time_us,
                    pc.is_idle,
                )
            })
            .collect()
    }

    /// Get per-CPU IPI statistics
    fn get_ipi_stats(&self) -> Vec<(u32, u64, u64, u64, u64)> {
        self.per_cpu
            .iter()
            .enumerate()
            .map(|(i, pc)| {
                (
                    i as u32,
                    pc.ipis_sent,
                    pc.ipis_received,
                    pc.tlb_shootdowns,
                    pc.reschedules,
                )
            })
            .collect()
    }

    /// Get per-CPU run queue lengths
    fn get_run_queue_lengths(&self) -> Vec<(u32, usize, usize)> {
        self.per_cpu
            .iter()
            .enumerate()
            .map(|(i, pc)| {
                let total = pc.run_queue.len();
                let runnable = pc.run_queue.iter().filter(|e| e.runnable).count();
                (i as u32, total, runnable)
            })
            .collect()
    }

    /// Full status report
    fn status(&self) -> String {
        let mut s = format!("SMP Core Status\n");
        s.push_str(&format!("CPUs: {}\n", self.num_cpus));
        s.push_str(&format!("Total IPIs sent: {}\n", self.total_ipis_sent));
        s.push_str(&format!(
            "Total IPIs received: {}\n",
            self.total_ipis_received
        ));
        s.push_str(&format!(
            "Load balancing: {}\n",
            if self.load_balance_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
        s.push_str(&format!(
            "Balance interval: {} ms\n",
            self.load_balance_interval_ms
        ));

        s.push_str("\nPer-CPU:\n");
        s.push_str("CPU  Load  RunQ  WorkQ  IPIsent  IPIrecv  TLBshoot  Resched  Idle\n");
        for (i, pc) in self.per_cpu.iter().enumerate() {
            s.push_str(&format!(
                "{:>3}  {:>4}  {:>4}  {:>5}  {:>7}  {:>7}  {:>8}  {:>7}  {}\n",
                i,
                pc.load,
                pc.run_queue.len(),
                pc.work_queue.len(),
                pc.ipis_sent,
                pc.ipis_received,
                pc.tlb_shootdowns,
                pc.reschedules,
                if pc.is_idle { "yes" } else { "no" }
            ));
        }

        s
    }
}

// ---------------------------------------------------------------------------
// Global subsystem and public API
// ---------------------------------------------------------------------------

static SMP_CORE: Mutex<SmpCoreSubsystem> = Mutex::new(SmpCoreSubsystem::new());

// --- IPI message passing ---

/// Send an IPI message to a specific CPU
pub fn send_ipi_message(target_cpu: u32, msg_type: IpiMessageType, arg0: u64, arg1: u64) -> bool {
    SMP_CORE
        .lock()
        .send_ipi_message(target_cpu, msg_type, arg0, arg1, false)
}

/// Send an IPI to all other CPUs
pub fn send_ipi_all_others(msg_type: IpiMessageType, arg0: u64, arg1: u64) -> u32 {
    SMP_CORE.lock().send_ipi_all_others(msg_type, arg0, arg1)
}

/// Process pending IPI messages on the current CPU (called from IPI handler)
pub fn process_ipi_messages() -> u32 {
    SMP_CORE.lock().process_ipi_messages()
}

/// Send a reschedule IPI to a specific CPU
pub fn kick_cpu(target_cpu: u32) -> bool {
    SMP_CORE
        .lock()
        .send_ipi_message(target_cpu, IpiMessageType::Reschedule, 0, 0, false)
}

// --- TLB shootdown ---

/// Invalidate a page across all CPUs
pub fn tlb_shootdown(addr: u64) {
    SMP_CORE.lock().tlb_shootdown(addr);
}

/// Flush all TLB entries across all CPUs
pub fn tlb_flush_all() {
    SMP_CORE.lock().tlb_shootdown(0);
}

// --- Run queue ---

/// Add a task to a CPU's run queue
pub fn enqueue_task(cpu: u32, entry: RunQueueEntry) -> bool {
    SMP_CORE.lock().enqueue_task(cpu, entry)
}

/// Remove a task from a CPU's run queue
pub fn dequeue_task(cpu: u32, pid: u32) -> Option<RunQueueEntry> {
    SMP_CORE.lock().dequeue_task(cpu, pid)
}

/// Pick the next task to run on a CPU
pub fn pick_next_task(cpu: u32) -> Option<u32> {
    SMP_CORE.lock().pick_next_task(cpu)
}

/// Update a task's vruntime after running
pub fn update_vruntime(cpu: u32, pid: u32, delta_us: u64) {
    SMP_CORE.lock().update_vruntime(cpu, pid, delta_us);
}

/// Set a task's runnable state
pub fn set_runnable(cpu: u32, pid: u32, runnable: bool) {
    SMP_CORE.lock().set_runnable(cpu, pid, runnable);
}

// --- Load balancing ---

/// Trigger load balancing across CPUs
pub fn balance_load() -> u32 {
    SMP_CORE.lock().balance_load()
}

/// Enable/disable load balancing
pub fn set_load_balance(enabled: bool) {
    SMP_CORE.lock().load_balance_enabled = enabled;
}

/// Set load balance interval
pub fn set_balance_interval(ms: u64) {
    SMP_CORE.lock().load_balance_interval_ms = ms;
}

// --- Work queue ---

/// Queue a work item on a specific CPU
pub fn queue_work(
    cpu: u32,
    name: &str,
    func_addr: usize,
    arg: u64,
    recurring: bool,
    interval_ms: u64,
) -> u32 {
    SMP_CORE
        .lock()
        .queue_work(cpu, name, func_addr, arg, recurring, interval_ms)
}

/// Cancel a work item
pub fn cancel_work(cpu: u32, work_id: u32) -> bool {
    SMP_CORE.lock().cancel_work(cpu, work_id)
}

/// Process due work items on the current CPU
pub fn process_work_queue() -> u32 {
    SMP_CORE.lock().process_work_queue()
}

// --- Remote function calls ---

/// Call a function on a specific CPU
pub fn call_function_on(target_cpu: u32, func_addr: usize, arg: u64) -> bool {
    SMP_CORE.lock().call_function_on(target_cpu, func_addr, arg)
}

// --- CPU idle tracking ---

/// Enter idle on current CPU
pub fn cpu_enter_idle() {
    let cpu = crate::smp::current_cpu();
    SMP_CORE.lock().cpu_enter_idle(cpu);
}

/// Exit idle on current CPU
pub fn cpu_exit_idle() {
    let cpu = crate::smp::current_cpu();
    SMP_CORE.lock().cpu_exit_idle(cpu);
}

/// Account CPU time
pub fn account_cpu_time(cpu: u32, active_us: u64, idle_us: u64) {
    SMP_CORE.lock().account_cpu_time(cpu, active_us, idle_us);
}

// --- Status ---

/// Get per-CPU load info
pub fn get_loads() -> Vec<(u32, u32, u64, u64, bool)> {
    SMP_CORE.lock().get_loads()
}

/// Get per-CPU IPI statistics
pub fn get_ipi_stats() -> Vec<(u32, u64, u64, u64, u64)> {
    SMP_CORE.lock().get_ipi_stats()
}

/// Get per-CPU run queue lengths
pub fn get_run_queue_lengths() -> Vec<(u32, usize, usize)> {
    SMP_CORE.lock().get_run_queue_lengths()
}

/// Get full SMP core status report
pub fn status() -> String {
    SMP_CORE.lock().status()
}

/// Check if a CPU needs to be rescheduled
pub fn need_resched(cpu: u32) -> bool {
    let core = SMP_CORE.lock();
    let idx = cpu as usize;
    if idx < core.num_cpus {
        core.per_cpu[idx].need_resched
    } else {
        false
    }
}

/// Clear the need_resched flag on a CPU
pub fn clear_need_resched(cpu: u32) {
    let mut core = SMP_CORE.lock();
    let idx = cpu as usize;
    if idx < core.num_cpus {
        core.per_cpu[idx].need_resched = false;
    }
}

pub fn init() {
    let ncpus = crate::smp::num_cpus().max(1) as usize;
    let mut core = SMP_CORE.lock();
    core.init(ncpus);
    drop(core);

    crate::serial_println!(
        "  [smp_core] SMP core initialized ({} CPUs, IPI msg, TLB shootdown, work queues, load balance)",
        ncpus);
}
