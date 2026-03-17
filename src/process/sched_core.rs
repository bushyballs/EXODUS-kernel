use crate::smp::{self, MAX_CPUS};
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
/// Scheduler core — schedule(), idle_task(), sleep_on(), wake_up(), tick()
///
/// Connects the CFS run-queue (cfs_simple.rs), the process table
/// (proc_table.rs), and the context-switch primitive (context_switch.rs)
/// into a working preemptive, multi-core scheduler.
///
/// Design:
///   - Per-CPU CFS run-queue array: each CPU has its own `CfsRunQueue`
///     protected by a per-CPU `Mutex`.  Schedule() picks from the local
///     queue first; if empty it tries to steal from the busiest peer.
///   - Per-CPU current PID: stored in the `PerCpuData` atomic (`current_pid`).
///     This avoids any cross-CPU atomic traffic on the hot path.
///   - Work stealing: when a CPU's local queue is empty, it scans all other
///     queues and moves the lowest-vruntime task from the busiest queue.
///     Steal only happens when the source has >= 2 tasks (to preserve locality).
///   - Timer IRQ calls tick() every 10 000 000 ns (10 ms); tick() increments
///     accumulated time and calls schedule() when the quantum expires.
///   - sleep_on() sets the process to Sleeping and calls schedule().
///   - wake_up() moves waiters back to Runnable without calling schedule();
///     a reschedule IPI (or the next timer tick) delivers the switch.
///
/// No std, no float, no panics.  All arithmetic is saturating.
use core::sync::atomic::{AtomicU64, Ordering};

use super::cfs_simple::{CfsRunQueue, CfsTask};
use super::context_switch::switch_context;
use super::proc_table::{self, ProcessState};

// Cgroup bandwidth throttle gate.  Imported here to keep the hot path
// check self-contained; the actual state lives in kernel::cgroups.
use crate::kernel::cgroups::cgroup_throttle_check;

// ---------------------------------------------------------------------------
// Per-CPU run-queue array
// ---------------------------------------------------------------------------

/// One CFS run-queue per logical CPU, each protected by its own Mutex.
///
/// Aligned to 64 bytes (one cache line) so adjacent CPU queue entries
/// in the `CPU_RQ` array don't share a cache line.  Without alignment,
/// CPUs 0 and 1 could share a line containing both their `Mutex::locked`
/// booleans, causing false sharing on every lock acquire/release.
// hot struct: locked ~1K/s per CPU on the scheduler hot path
#[repr(C, align(64))]
struct CpuRunQueue {
    rq: Mutex<CfsRunQueue>,
}

impl CpuRunQueue {
    const fn new() -> Self {
        CpuRunQueue {
            rq: Mutex::new(CfsRunQueue::new()),
        }
    }
}

// SAFETY: CpuRunQueue is accessed only through its Mutex.
unsafe impl Sync for CpuRunQueue {}

const RQ_INIT: CpuRunQueue = CpuRunQueue::new();
static CPU_RQ: [CpuRunQueue; MAX_CPUS] = [RQ_INIT; MAX_CPUS];

/// Accumulated CPU-time (ns) for the currently running task on each CPU.
/// Reset to 0 on every schedule(); incremented by tick().
static CURRENT_NS_USED: [AtomicU64; MAX_CPUS] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; MAX_CPUS]
};

/// Scheduling quantum: 10 ms.
const QUANTUM_NS: u64 = 10_000_000;

// ---------------------------------------------------------------------------
// Per-CPU current-PID helpers
// ---------------------------------------------------------------------------

/// Return the PID currently running on the calling CPU.
#[inline]
pub fn current_pid() -> u32 {
    smp::this_cpu().current_pid.load(Ordering::Relaxed)
}

/// Store the PID running on the calling CPU.
#[inline]
fn set_current_pid(pid: u32) {
    smp::this_cpu().current_pid.store(pid, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Run-queue length query (for work stealing)
// ---------------------------------------------------------------------------

fn cpu_rq_len(cpu: usize) -> usize {
    if cpu >= MAX_CPUS {
        return 0;
    }
    CPU_RQ[cpu].rq.lock().len()
}

/// Public accessor for the per-CPU run-queue length.
/// Used by the NUMA scheduler (`process::numa`) to select the least-loaded
/// CPU in a target NUMA node without exposing internal run-queue structures.
pub fn cpu_rq_len_pub(cpu: usize) -> usize {
    cpu_rq_len(cpu)
}

// ---------------------------------------------------------------------------
// Enqueue / dequeue helpers
// ---------------------------------------------------------------------------

/// Enqueue a process on the calling CPU's local run queue.
pub fn enqueue(pid: u32, weight: u32) {
    let cpu = smp::current_cpu() as usize;
    enqueue_on(cpu, pid, weight);
}

/// Enqueue a process on a specific CPU's run queue.
/// Used by wake_up() to pin a task to the CPU that woke it (optional) and
/// by fork() to target an idle CPU.
pub fn enqueue_on(cpu: usize, pid: u32, weight: u32) {
    if cpu >= MAX_CPUS {
        return;
    }
    let mut rq = CPU_RQ[cpu].rq.lock();
    let min_vrt = rq.min_vruntime;
    let vruntime = {
        let entry = proc_table::get_process(pid);
        let recorded = entry.map(|e| e.vruntime).unwrap_or(0);
        // New tasks start at min_vruntime to prevent starvation.
        recorded.max(min_vrt)
    };
    rq.enqueue(CfsTask::new(pid, vruntime, weight));
}

/// Remove a process from the calling CPU's run queue.
pub fn dequeue(pid: u32) {
    let cpu = smp::current_cpu() as usize;
    if cpu >= MAX_CPUS {
        return;
    }
    CPU_RQ[cpu].rq.lock().dequeue(pid);
}

// ---------------------------------------------------------------------------
// Work stealing
// ---------------------------------------------------------------------------

/// Try to steal one task from the busiest peer CPU.
///
/// We look for the peer with the highest queue length.  If it has >= 2
/// tasks we lock its queue and take the lowest-vruntime task.
/// Returns `Some(CfsTask)` on success, `None` if no suitable source exists.
fn steal_from_busiest_cpu(my_cpu: usize) -> Option<CfsTask> {
    let num = smp::num_cpus() as usize;
    let limit = num.min(MAX_CPUS);

    let mut busiest = usize::MAX;
    let mut max_tasks = 0usize;

    for i in 0..limit {
        if i == my_cpu {
            continue;
        }
        let n = cpu_rq_len(i);
        if n > max_tasks {
            max_tasks = n;
            busiest = i;
        }
    }

    // Do not steal from a queue with only 1 task — not worth the coherence traffic.
    if busiest == usize::MAX || max_tasks < 2 {
        return None;
    }

    CPU_RQ[busiest].rq.lock().dequeue_min()
}

// ---------------------------------------------------------------------------
// schedule() — the main scheduling decision point
// ---------------------------------------------------------------------------

/// Perform a scheduling decision on the calling CPU.
///
/// Must be called with interrupts **disabled** (or from within a hardware
/// interrupt handler where the CPU already masks interrupts).
///
/// Returns immediately without switching if:
///   - Preemption is currently disabled (`preempt_count > 0`).
///   - We are inside an interrupt handler and the scheduler was not invoked
///     explicitly.
///   - No task is runnable (idle continues).
pub fn schedule() {
    // Honour preemption-disable nesting.
    if !smp::preemptible() {
        return;
    }

    let my_cpu = smp::current_cpu() as usize;
    let current = current_pid();

    // 1. Charge accumulated time to the current task's vruntime.
    let ns_idx = my_cpu.min(MAX_CPUS.saturating_sub(1));
    let ns_used = CURRENT_NS_USED[ns_idx].swap(0, Ordering::Relaxed);
    {
        let mut rq = CPU_RQ[my_cpu].rq.lock();
        rq.update_vruntime(current, ns_used);
    }

    // 2. Check the SCHED_DEADLINE (EDF) run-queue first.
    //    Deadline tasks have strict priority over CFS tasks.
    if let Some(dl_pid) = super::sched_deadline::dl_schedule() {
        if dl_pid != current {
            // Deduct time used so far against the DL task's budget.
            super::sched_deadline::dl_update_runtime(dl_pid, 0); // no charge yet

            // Re-enqueue current if still Runnable/Running.
            let cur_state = {
                let tbl = proc_table::PROCESS_TABLE.lock();
                tbl.slots
                    .get(current as usize)
                    .and_then(|s| s.as_ref())
                    .map(|e| e.state)
            };
            if cur_state == Some(ProcessState::Running) {
                proc_table::set_state(current, ProcessState::Runnable);
                let weight = {
                    let tbl = proc_table::PROCESS_TABLE.lock();
                    tbl.slots
                        .get(current as usize)
                        .and_then(|s| s.as_ref())
                        .map(|e| e.weight)
                        .unwrap_or(1024)
                };
                enqueue(current, weight);
            }

            proc_table::set_state(dl_pid, ProcessState::Running);
            set_current_pid(dl_pid);

            let old_rsp_ptr: *mut u64 = {
                let mut tbl = proc_table::PROCESS_TABLE.lock();
                tbl.slots
                    .get_mut(current as usize)
                    .and_then(|s| s.as_mut())
                    .map(|e| &mut e.saved_rsp as *mut u64)
                    .unwrap_or(core::ptr::null_mut())
            };
            let new_rsp: u64 = {
                let tbl = proc_table::PROCESS_TABLE.lock();
                tbl.slots
                    .get(dl_pid as usize)
                    .and_then(|s| s.as_ref())
                    .map(|e| e.saved_rsp)
                    .unwrap_or(0)
            };
            if !old_rsp_ptr.is_null() && new_rsp != 0 {
                unsafe {
                    switch_context(old_rsp_ptr, new_rsp);
                }
            }
            return;
        }
        // dl_pid == current: we are already the highest-priority task; continue.
    }

    // 3. Dequeue the next CFS task: local first, then steal.
    //    Skip tasks whose cgroup has exhausted its CPU bandwidth quota.
    let sched_time_ns = crate::time::clock::uptime_ms().saturating_mul(1_000_000);

    let next_task: Option<CfsTask> = {
        let mut rq = CPU_RQ[my_cpu].rq.lock();
        // Try up to 8 candidates to skip throttled cgroup tasks without
        // looping forever.  Skipped tasks are re-enqueued at the back so
        // they will be retried on the next tick.
        let mut skipped: Vec<CfsTask> = Vec::new();
        let mut chosen: Option<CfsTask> = None;

        for _ in 0..8 {
            let candidate = rq.dequeue_min();
            match candidate {
                None => break,
                Some(t) => {
                    if cgroup_throttle_check(t.pid, sched_time_ns) {
                        // This task is throttled — defer it and try the next.
                        skipped.push(t);
                    } else {
                        chosen = Some(t);
                        break;
                    }
                }
            }
        }
        // Re-enqueue any tasks we skipped so they are not lost.
        for t in skipped {
            rq.enqueue(t);
        }

        if chosen.is_none() {
            // No unthrottled task in local queue — try work-stealing.
            drop(rq);
            steal_from_busiest_cpu(my_cpu)
        } else {
            chosen
        }
    };

    let next_task = match next_task {
        Some(t) => t,
        None => return, // nothing else runnable; keep current running
    };

    let next = next_task.pid;

    if next == current {
        // The local queue was empty; we stole the same task we're already
        // running (shouldn't happen normally but guard defensively).
        CPU_RQ[my_cpu].rq.lock().enqueue(next_task);
        return;
    }

    // 3. Re-enqueue current if it is still Runnable/Running.
    let current_state = {
        let tbl = proc_table::PROCESS_TABLE.lock();
        tbl.slots
            .get(current as usize)
            .and_then(|s| s.as_ref())
            .map(|e| e.state)
    };

    if current_state == Some(ProcessState::Running) {
        proc_table::set_state(current, ProcessState::Runnable);
        let weight = {
            let tbl = proc_table::PROCESS_TABLE.lock();
            tbl.slots
                .get(current as usize)
                .and_then(|s| s.as_ref())
                .map(|e| e.weight)
                .unwrap_or(1024)
        };
        enqueue(current, weight);
    }

    // 4. Mark next task as Running.
    proc_table::set_state(next, ProcessState::Running);
    set_current_pid(next);

    // 5. Obtain RSP values for the naked context switch.
    //    We must NOT hold any lock across the actual switch instruction.
    let old_rsp_ptr: *mut u64 = {
        let mut tbl = proc_table::PROCESS_TABLE.lock();
        tbl.slots
            .get_mut(current as usize)
            .and_then(|s| s.as_mut())
            .map(|e| &mut e.saved_rsp as *mut u64)
            .unwrap_or(core::ptr::null_mut())
    };

    let new_rsp: u64 = {
        let tbl = proc_table::PROCESS_TABLE.lock();
        tbl.slots
            .get(next as usize)
            .and_then(|s| s.as_ref())
            .map(|e| e.saved_rsp)
            .unwrap_or(0)
    };

    if old_rsp_ptr.is_null() || new_rsp == 0 {
        return;
    }

    // 6. Perform the naked context switch.
    //    After this the CPU runs as `next`.  When `current` is rescheduled
    //    it resumes at the instruction immediately following this call.
    unsafe {
        switch_context(old_rsp_ptr, new_rsp);
    }
    // <<< `current` resumes here after being switched back to. >>>
}

// ---------------------------------------------------------------------------
// tick() — called from the timer interrupt handler
// ---------------------------------------------------------------------------

/// Accumulate elapsed time for the current task and preempt if quantum used.
///
/// `elapsed_ns` is the number of nanoseconds since the last tick.
/// At 100 Hz (10 ms PIT) this is 10_000_000.  Runs on the CPU that received
/// the timer IRQ.
// hot path: called from timer IRQ handler at 100 Hz per CPU
#[inline]
pub fn tick(elapsed_ns: u64) {
    let cpu = smp::current_cpu() as usize;
    let ns_idx = cpu.min(MAX_CPUS.saturating_sub(1));
    let prev = CURRENT_NS_USED[ns_idx].fetch_add(elapsed_ns, Ordering::Relaxed);
    let total = prev.saturating_add(elapsed_ns);

    // Charge elapsed CPU time to the current task's cgroup (nanosecond resolution).
    // This keeps the cgroup bandwidth counters up-to-date between schedule() calls.
    let pid = current_pid();
    crate::kernel::cgroups::account_cpu_time(pid, elapsed_ns);

    // Charge elapsed CPU time against any running SCHED_DEADLINE task.
    super::sched_deadline::dl_update_runtime(pid, elapsed_ns);

    // Replenish throttled deadline tasks whose period has rolled over.
    let current_ns = crate::time::clock::uptime_ms().saturating_mul(1_000_000);
    super::sched_deadline::dl_replenish_tick(current_ns);

    // Advance all armed timerfds and increment their expiration counters
    // for any that have fired since the last tick.
    crate::ipc::timerfd::timerfd_tick(current_ns);

    if total >= QUANTUM_NS {
        schedule();
    }
}

// ---------------------------------------------------------------------------
// idle_task() — runs on a CPU when no other task is runnable
// ---------------------------------------------------------------------------

/// The idle task.
///
/// Enables interrupts, halts, then calls schedule() on every wakeup so a
/// newly enqueued task is picked up immediately.
pub fn idle_task() -> ! {
    loop {
        unsafe {
            core::arch::asm!("sti", "hlt", options(nomem, nostack));
        }
        schedule();
    }
}

// ---------------------------------------------------------------------------
// Wait queues — sleep_on() / wake_up()
// ---------------------------------------------------------------------------

/// Wait queues indexed by "channel" (any stable kernel address used as an
/// event identifier, e.g. `&some_mutex as *const _ as u64`).
static WAIT_QUEUES: Mutex<WaitQueues> = Mutex::new(WaitQueues::new());

struct WaitQueues {
    inner: BTreeMap<u64, Vec<u32>>,
}

impl WaitQueues {
    const fn new() -> Self {
        WaitQueues {
            inner: BTreeMap::new(),
        }
    }
}

/// Block the current process on `channel`.
///
/// The caller must ensure the condition being waited on will eventually be
/// signalled via `wake_up(channel)`.
pub fn sleep_on(channel: u64, current_pid_arg: u32) {
    {
        let mut wq = WAIT_QUEUES.lock();
        wq.inner
            .entry(channel)
            .or_insert_with(Vec::new)
            .push(current_pid_arg);
    }
    proc_table::set_state(current_pid_arg, ProcessState::Sleeping);
    dequeue(current_pid_arg);
    schedule();
    // Returns here when the process is woken and rescheduled.
}

/// Wake all processes sleeping on `channel`.
///
/// Enqueues them on the calling CPU's local queue.  Work stealing will
/// redistribute them if another CPU is idle.
pub fn wake_up(channel: u64) {
    let waiters: Vec<u32> = {
        let mut wq = WAIT_QUEUES.lock();
        wq.inner.remove(&channel).unwrap_or_default()
    };

    for pid in waiters {
        proc_table::set_state(pid, ProcessState::Runnable);
        let weight = proc_table::get_process(pid)
            .map(|e| e.weight)
            .unwrap_or(1024);
        enqueue(pid, weight);
    }
}

/// Wake a single specific process sleeping on `channel`.
pub fn wake_up_one(channel: u64, target_pid: u32) {
    {
        let mut wq = WAIT_QUEUES.lock();
        if let Some(list) = wq.inner.get_mut(&channel) {
            list.retain(|&p| p != target_pid);
            if list.is_empty() {
                wq.inner.remove(&channel);
            }
        }
    }
    proc_table::set_state(target_pid, ProcessState::Runnable);
    let weight = proc_table::get_process(target_pid)
        .map(|e| e.weight)
        .unwrap_or(1024);
    enqueue(target_pid, weight);
}

// ---------------------------------------------------------------------------
// Legacy compatibility shim
// ---------------------------------------------------------------------------

/// Global run-queue alias kept for source-compatibility with code that
/// referenced the old single-queue `RUN_QUEUE` static.
/// New code should use `enqueue()` / `dequeue()` directly.
pub static RUN_QUEUE: Mutex<CfsRunQueue> = Mutex::new(CfsRunQueue::new());

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the scheduler core.
///
/// Must be called once during kernel boot, after the process table and
/// per-CPU data (smp::init) have been initialised.
pub fn init() {
    crate::serial_println!("  [sched-dbg] set_current_pid");
    set_current_pid(0);
    crate::serial_println!("  [sched-dbg] CURRENT_NS_USED store");
    CURRENT_NS_USED[0].store(0, Ordering::Relaxed);
    crate::serial_println!(
        "  [sched_core] multi-core CFS scheduler initialised ({} CPU slots, work-stealing enabled)",
        MAX_CPUS
    );
}
