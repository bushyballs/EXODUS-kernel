/*
 * Genesis OS — Cooperative Scheduler
 *
 * Simple round-robin task scheduler with per-CPU run queues.
 * Supports preemption via timer interrupts.
 */

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_TASKS: usize = 1024;
const TIME_SLICE_MS: u64 = 10; // 10ms time slices

/// Task state
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum TaskState {
    Ready = 0,
    Running = 1,
    Blocked = 2,
    Zombie = 3,
}

/// Task control block
#[repr(C, align(64))]
#[derive(Copy, Clone)]
pub struct Task {
    pub tid: u64,
    pub state: TaskState,
    pub cpu_affinity: u32,
    pub priority: u8,
    pub time_slice: u64,
    pub ticks_used: u64,
    pub kernel_stack: u64,
    pub user_stack: u64,
    pub page_table: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
}

impl Task {
    const fn new() -> Self {
        Task {
            tid: 0,
            state: TaskState::Ready,
            cpu_affinity: 0,
            priority: 0,
            time_slice: TIME_SLICE_MS,
            ticks_used: 0,
            kernel_stack: 0,
            user_stack: 0,
            page_table: 0,
            rip: 0,
            rsp: 0,
            rflags: 0,
        }
    }
}

static mut TASKS: [Task; MAX_TASKS] = [Task::new(); MAX_TASKS];
static NEXT_TID: AtomicU64 = AtomicU64::new(1);
static SCHEDULER_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize scheduler
pub fn init() {
    SCHEDULER_INITIALIZED.store(true, Ordering::SeqCst);
}

/// Create a new task
pub fn spawn_task(entry_point: u64, stack: u64, page_table: u64) -> Result<u64, &'static str> {
    let tid = NEXT_TID.fetch_add(1, Ordering::SeqCst);

    // Find free task slot
    for i in 0..MAX_TASKS {
        unsafe {
            if TASKS[i].state == TaskState::Zombie || TASKS[i].tid == 0 {
                TASKS[i].tid = tid;
                TASKS[i].state = TaskState::Ready;
                TASKS[i].rip = entry_point;
                TASKS[i].rsp = stack;
                TASKS[i].kernel_stack = stack;
                TASKS[i].page_table = page_table;
                TASKS[i].time_slice = TIME_SLICE_MS;
                TASKS[i].ticks_used = 0;
                TASKS[i].priority = 0;
                TASKS[i].cpu_affinity = crate::percpu::cpu_id() as u32;
                return Ok(tid);
            }
        }
    }

    Err("No free task slots")
}

/// Get task by TID
fn find_task_mut(tid: u64) -> Option<&'static mut Task> {
    unsafe {
        for i in 0..MAX_TASKS {
            if TASKS[i].tid == tid {
                return Some(&mut TASKS[i]);
            }
        }
    }
    None
}

/// Timer tick handler (called from interrupt context)
pub fn tick() {
    if !SCHEDULER_INITIALIZED.load(Ordering::SeqCst) {
        return;
    }

    unsafe {
        let cpu = crate::percpu::current_cpu();
        let current_tid = (*cpu).current_task;

        if let Some(task) = find_task_mut(current_tid) {
            task.ticks_used += 1;

            // --- Neural Watchdog Check ---
            // Periodically check if Neural Bus signals have stalled
            if task.ticks_used % 100 == 0 {
                let bus = crate::neural_bus::BUS.lock();
                if bus.bus_active && bus.total_signals > 0 {
                    let last_ts = bus.class_last_signal.iter().max().unwrap_or(&0);
                    let now = crate::time::clock::unix_time();
                    if now - last_ts > 300 {
                        crate::serial_println!(
                            "🚨 [kernel] NEURAL WATCHDOG: Signal stall detected ({}s).",
                            now - last_ts
                        );
                    }
                }
            }

            // Check if time slice expired
            if task.ticks_used >= task.time_slice {
                // Trigger reschedule
                reschedule();
            }
        }
    }
}

/// Reschedule (switch to next ready task)
pub fn reschedule() {
    if !SCHEDULER_INITIALIZED.load(Ordering::SeqCst) {
        return;
    }

    if !crate::percpu::can_preempt() {
        return;
    }

    unsafe {
        let cpu = crate::percpu::current_cpu();
        let current_tid = (*cpu).current_task;
        let cpu_id = (*cpu).cpu_id;

        // Find next ready task for this CPU
        let mut next_task: Option<&mut Task> = None;
        let mut start_idx = 0;

        // Find current task index
        for i in 0..MAX_TASKS {
            if TASKS[i].tid == current_tid {
                start_idx = (i + 1) % MAX_TASKS;
                // Mark current task as ready (unless it's blocked)
                if TASKS[i].state == TaskState::Running {
                    TASKS[i].state = TaskState::Ready;
                }
                break;
            }
        }

        // Round-robin search for next ready task
        for offset in 0..MAX_TASKS {
            let i = (start_idx + offset) % MAX_TASKS;
            if TASKS[i].state == TaskState::Ready && TASKS[i].cpu_affinity == cpu_id {
                next_task = Some(&mut TASKS[i]);
                break;
            }
        }

        if let Some(task) = next_task {
            // Switch to next task
            task.state = TaskState::Running;
            task.ticks_used = 0;
            (*cpu).current_task = task.tid;

            // Context switch would happen here
            // For now, this is a placeholder
            context_switch(task);
        }
    }
}

/// Perform context switch to given task
unsafe fn context_switch(task: &mut Task) {
    // Load task's page table
    if task.page_table != 0 {
        crate::cpu::write_cr3(task.page_table);
    }

    // Update RSP and RIP (in a real implementation, this would
    // save/restore all registers and switch stacks)
    // For now, this is a stub
}

/// Yield CPU to next task
pub fn yield_now() {
    reschedule();
}

/// Block current task
pub fn block_current() {
    unsafe {
        let cpu = crate::percpu::current_cpu();
        let current_tid = (*cpu).current_task;

        if let Some(task) = find_task_mut(current_tid) {
            task.state = TaskState::Blocked;
        }

        reschedule();
    }
}

/// Unblock task by TID
pub fn unblock_task(tid: u64) {
    if let Some(task) = find_task_mut(tid) {
        if task.state == TaskState::Blocked {
            task.state = TaskState::Ready;
        }
    }
}

/// Exit current task
pub fn exit_task() {
    unsafe {
        let cpu = crate::percpu::current_cpu();
        let current_tid = (*cpu).current_task;

        if let Some(task) = find_task_mut(current_tid) {
            task.state = TaskState::Zombie;
        }

        reschedule();
    }
}

/// Get current task ID
pub fn current_tid() -> u64 {
    unsafe {
        let cpu = crate::percpu::current_cpu();
        (*cpu).current_task
    }
}

/// Enter idle loop (for application processors)
pub fn enter_idle() -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

/// Get number of ready tasks
pub fn ready_count() -> usize {
    let mut count = 0;
    unsafe {
        for i in 0..MAX_TASKS {
            if TASKS[i].state == TaskState::Ready {
                count += 1;
            }
        }
    }
    count
}

/// Get number of running tasks
pub fn running_count() -> usize {
    let mut count = 0;
    unsafe {
        for i in 0..MAX_TASKS {
            if TASKS[i].state == TaskState::Running {
                count += 1;
            }
        }
    }
    count
}
