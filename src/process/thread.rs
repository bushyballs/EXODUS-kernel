use crate::sync::Mutex;
use alloc::collections::VecDeque;
/// Kernel thread creation and lifecycle management.
///
/// Part of the AIOS kernel.
use alloc::string::String;

// ---------------------------------------------------------------------------
// Pcb shim — the thread subsystem allocates raw PCB-sized heap blocks.
// We represent a "Pcb" as an opaque struct whose layout matches the kernel
// Process struct enough to be useful as a zero-initialised allocation unit.
// ---------------------------------------------------------------------------

/// Opaque, zero-initialised PCB allocation unit (16 KiB kernel-stack + header).
pub struct Pcb {
    _opaque: u8,
}

/// Allocate a zeroed PCB on the heap and return a raw pointer to it.
///
/// The caller is responsible for casting the pointer to the appropriate
/// process struct type and initialising all fields before use.
pub fn alloc_thread_pcb() -> *mut Pcb {
    use alloc::alloc::{alloc_zeroed, Layout};
    // SAFETY: Layout::new::<Pcb>() is non-zero sized (Pcb contains at least
    // one byte) and the allocation is never used through the *mut Pcb directly
    // — callers cast it to the real process type.
    let layout = Layout::new::<Pcb>();
    let ptr = unsafe { alloc_zeroed(layout) };
    ptr as *mut Pcb
}

// ---------------------------------------------------------------------------
// kthreadd work queue
// ---------------------------------------------------------------------------

/// A single unit of deferred kernel-thread work.
pub struct KthreadWork {
    /// Function to call when this work item is processed.
    pub func: fn(),
    /// Human-readable description for logging.
    pub name: &'static str,
}

/// Static work queue drained by the kthreadd daemon (PID 2).
pub static KTHREAD_WORK_QUEUE: Mutex<VecDeque<KthreadWork>> = Mutex::new(VecDeque::new());

/// Enqueue a work item for execution by kthreadd.
pub fn kthread_run(func: fn(), name: &'static str) {
    let work = KthreadWork { func, name };
    KTHREAD_WORK_QUEUE.lock().push_back(work);
    crate::serial_println!("kthreadd: enqueued work \"{}\"", name);
}

// ---------------------------------------------------------------------------
// kthreadd daemon main loop
// ---------------------------------------------------------------------------

/// Main loop for the kthreadd kernel-thread daemon (PID 2).
///
/// Drains the `KTHREAD_WORK_QUEUE` on every iteration, calling each work
/// item's function, then yields to avoid spinning the CPU.
pub fn kthreadd_main() -> ! {
    crate::serial_println!("kthreadd: daemon started (PID 2)");
    loop {
        // Drain all pending work items under a minimal critical section.
        loop {
            let work = KTHREAD_WORK_QUEUE.lock().pop_front();
            match work {
                Some(item) => {
                    crate::serial_println!("kthreadd: running work \"{}\"", item.name);
                    (item.func)();
                }
                None => break,
            }
        }
        // Yield so other threads can run before we spin again.
        crate::process::yield_now();
    }
}

// ---------------------------------------------------------------------------
// KernelThread descriptor
// ---------------------------------------------------------------------------

/// Kernel thread descriptor.
pub struct KernelThread {
    pub pid: u32,
    pub name: String,
    pub should_stop: bool,
}

impl KernelThread {
    pub fn new(pid: u32, name: &str) -> Self {
        KernelThread {
            pid,
            name: String::from(name),
            should_stop: false,
        }
    }

    /// Request this kernel thread to stop.
    ///
    /// Sets `should_stop = true` and wakes the thread via the scheduler
    /// wait-queue so it can observe the flag and exit its main loop promptly.
    pub fn stop(&mut self) {
        self.should_stop = true;
        // Wake the thread so it sees should_stop without waiting for a timer tick.
        // Use the PID as the channel: kthread_create() sleeps on `pid as u64`.
        crate::process::sched_core::wake_up(self.pid as u64);
        crate::serial_println!(
            "kthread: stop requested for pid={} name={}",
            self.pid,
            self.name
        );
    }

    /// Check if this thread has been asked to stop.
    pub fn should_stop(&self) -> bool {
        self.should_stop
    }
}

// ---------------------------------------------------------------------------
// wake_up_thread
// ---------------------------------------------------------------------------

/// Find the thread identified by `tid` in the process table, set its state
/// to `Runnable`, and re-enqueue it on the scheduler run queue.
pub fn wake_up_thread(tid: u32) {
    use crate::process::proc_table::{ProcessState, PROCESS_TABLE};
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(slot) = table.slots.get_mut(tid as usize) {
            if let Some(entry) = slot.as_mut() {
                entry.state = ProcessState::Runnable;
            } else {
                crate::serial_println!("wake_up_thread: TID {} not found in process table", tid);
                return;
            }
        } else {
            crate::serial_println!("wake_up_thread: TID {} out of bounds", tid);
            return;
        }
    }
    // Retrieve the weight before enqueuing.
    let weight = {
        let table = PROCESS_TABLE.lock();
        table
            .slots
            .get(tid as usize)
            .and_then(|s| s.as_ref())
            .map(|e| e.weight)
            .unwrap_or(1024)
    };
    crate::process::sched_core::enqueue(tid, weight);
    crate::serial_println!("wake_up_thread: TID {} set Runnable and enqueued", tid);
}

// ---------------------------------------------------------------------------
// kthread_create
// ---------------------------------------------------------------------------

/// Spawn a new kernel thread with the given entry function.
///
/// Allocates a PID, creates a kernel-mode `Process` entry in the process
/// table, sets the initial kernel-stack RSP so the context-switch will
/// jump to `entry`, and enqueues the new thread on the scheduler run queue.
pub fn kthread_create(name: &str, entry: fn()) -> Result<u32, &'static str> {
    use crate::process::pcb::{Process, ProcessState, PROCESS_TABLE};
    use crate::process::scheduler::SCHEDULER;
    use crate::process::MAX_PROCESSES;

    // Find a free PID in the process table.
    let pid: u32 = {
        let table = PROCESS_TABLE.lock();
        let mut found = None;
        for i in 2..MAX_PROCESSES {
            if table[i].is_none() {
                found = Some(i as u32);
                break;
            }
        }
        found.ok_or("kthread_create: no free PID")?
    };

    // Build the kernel-mode process entry.
    let mut proc = Process::new_kernel(pid, name);

    // Set the initial instruction pointer so the thread starts at `entry`.
    proc.context.rip = entry as u64;
    proc.context.rflags = 0x200; // interrupts enabled
    proc.context.cs = 0x08; // kernel code segment
    proc.context.ss = 0x10; // kernel data segment

    // Align the stack pointer to a 16-byte boundary and push the entry
    // address as a return address so `ret` in the trampoline jumps to it.
    let stack_top = proc.kernel_stack_top();
    unsafe {
        let sp = (stack_top as *mut u64).sub(1);
        *sp = entry as u64;
        proc.context.rsp = sp as u64;
    }

    proc.state = ProcessState::Ready;

    // Insert into the process table and add to scheduler.
    {
        let mut table = PROCESS_TABLE.lock();
        table[pid as usize] = Some(proc);
    }
    SCHEDULER.lock().add(pid);

    crate::serial_println!("kthread_create: spawned \"{}\" as PID {}", name, pid);
    Ok(pid)
}

// ---------------------------------------------------------------------------
// kthreadd_init — create the kthreadd daemon (PID 2)
// ---------------------------------------------------------------------------

/// Create the kthreadd kernel-thread daemon.
///
/// Spawns a kernel thread at PID 2 whose entry point is `kthreadd_main`.
/// kthreadd drains the `KTHREAD_WORK_QUEUE` on each iteration and delegates
/// to `kthread_create` for any new thread creation requests.
pub fn kthreadd_init() {
    match kthread_create("kthreadd", kthreadd_main_wrapper) {
        Ok(pid) => crate::serial_println!("kthreadd_init: kthreadd started as PID {}", pid),
        Err(e) => crate::serial_println!("kthreadd_init: failed to start kthreadd — {}", e),
    }
}

/// Thin wrapper so `kthreadd_main` (which returns `!`) fits the `fn()` type.
fn kthreadd_main_wrapper() {
    kthreadd_main();
}

// ---------------------------------------------------------------------------
// Module initialiser
// ---------------------------------------------------------------------------

/// Initialize the kernel threading subsystem.
pub fn init() {
    kthreadd_init();
}
