/// fork/clone implementation with copy-on-write page table setup.
///
/// Part of the AIOS kernel.
///
/// Integration summary
/// -------------------
/// `do_fork` is now fully wired to:
///   - `pcb::PROCESS_TABLE`         — allocate child PCB slot
///   - `pcb::Process::fork()`       — deep-copy parent PCB
///   - `scheduler::SCHEDULER`       — read current PID, enqueue child
///   - `memory::vmm::clone_page_table` — COW page-table duplication
///
/// `copy_page_tables_cow` delegates to `memory::vmm::clone_page_table`
/// which walks the parent PML4 and marks writable pages read-only/COW in
/// both parent and child.
use core::sync::atomic::{AtomicU32, Ordering};

use super::pcb::{ProcessState, PROCESS_TABLE};
use super::scheduler::SCHEDULER;

// ---------------------------------------------------------------------------
// Global PID counter
// ---------------------------------------------------------------------------

/// Next PID to hand out.  PID 0 is the idle process (created at boot);
/// we start real allocations from 1 upward.
static NEXT_PID: AtomicU32 = AtomicU32::new(2); // 0=idle, 1=init reserved

/// Allocate the next available PID.
///
/// Scans the process table to find a free slot.  On wraparound we
/// restart from 2 (skipping idle=0 and init=1).  Returns `None` if
/// the table is completely full.
fn alloc_pid() -> Option<u32> {
    let table = PROCESS_TABLE.lock();
    // Fast path: try the atomic counter first.
    let start = NEXT_PID.load(Ordering::Relaxed) as usize;
    let max = table.len();
    for delta in 0..max {
        let candidate = (start + delta) % max;
        // Skip PID 0 and 1 (idle and init).
        if candidate < 2 {
            continue;
        }
        if table[candidate].is_none() {
            // Reserve it immediately.
            let next = ((candidate + 1) % max).max(2) as u32;
            NEXT_PID.store(next, Ordering::Relaxed);
            return Some(candidate as u32);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CloneFlags
// ---------------------------------------------------------------------------

/// Flags that control what the child inherits from the parent (clone(2)).
pub struct CloneFlags {
    /// Share virtual address space with parent (thread semantics).
    pub share_vm: bool,
    /// Share the filesystem context (cwd, umask, root).
    pub share_fs: bool,
    /// Share the open file-descriptor table.
    pub share_files: bool,
    /// Share signal handlers (do NOT reset to SIG_DFL on exec).
    pub share_sighand: bool,
    /// Create the child in a new PID namespace.
    pub new_pid_ns: bool,
}

impl CloneFlags {
    /// Classic fork: share nothing.
    pub fn fork() -> Self {
        CloneFlags {
            share_vm: false,
            share_fs: false,
            share_files: false,
            share_sighand: false,
            new_pid_ns: false,
        }
    }

    /// Thread creation: share everything.
    pub fn thread() -> Self {
        CloneFlags {
            share_vm: true,
            share_fs: true,
            share_files: true,
            share_sighand: true,
            new_pid_ns: false,
        }
    }
}

// ---------------------------------------------------------------------------
// COW page-table duplication
// ---------------------------------------------------------------------------

/// Duplicate the parent's page tables into the child with full COW semantics.
///
/// Calls `memory::paging::clone_page_table()` which:
///   1. Allocates a new PML4 for the child.
///   2. Copies the kernel half (entries 256..511) directly.
///   3. For all user-space present PTEs: marks both parent and child
///      as read-only + COW_BIT, increments physical frame refcounts.
///
/// On a subsequent write fault the page-fault handler in `paging.rs`
/// performs the actual copy (or in-place promote if sole owner).
///
/// Returns `Some(child_pml4_phys_addr)` on success, `None` on OOM.
fn copy_page_tables_cow(parent_pid: u32, child_pid: u32) -> Option<usize> {
    crate::serial_println!(
        "[fork] COW page-table duplication: parent={} child={}",
        parent_pid,
        child_pid
    );

    // Retrieve the parent's PML4 physical address from the PCB.
    let parent_pml4_phys = {
        let table = PROCESS_TABLE.lock();
        table[parent_pid as usize]
            .as_ref()
            .map(|p| p.page_table)
            .unwrap_or(0)
    };

    if parent_pml4_phys == 0 {
        // Process has no dedicated page table (e.g., early-boot kernel process).
        // Fall back to the current CR3 so the child has a valid PML4.
        let cr3 = crate::memory::paging::read_cr3();
        crate::serial_println!(
            "[fork] parent PML4 is 0 — using current CR3 {:#x} for child {}",
            cr3,
            child_pid
        );
        return Some(cr3);
    }

    crate::serial_println!(
        "[fork] calling paging::clone_page_table() for parent PML4={:#x}",
        parent_pml4_phys
    );

    // Save CR3, temporarily switch to parent PML4 so clone_page_table()
    // operates on the correct address space, then restore.
    //
    // Note: clone_page_table() reads CR3 internally.  We must ensure it
    // reads the *parent's* PML4 rather than the current kernel PML4.
    let saved_cr3 = crate::memory::paging::read_cr3();
    if saved_cr3 != parent_pml4_phys {
        unsafe {
            crate::memory::paging::write_cr3(parent_pml4_phys);
        }
    }

    let result = crate::memory::paging::clone_page_table();

    if saved_cr3 != parent_pml4_phys {
        unsafe {
            crate::memory::paging::write_cr3(saved_cr3);
        }
    }

    match result {
        Ok(child_pml4) => {
            crate::serial_println!(
                "[fork] child {} page table created at {:#x}",
                child_pid,
                child_pml4
            );
            Some(child_pml4)
        }
        Err(()) => {
            crate::serial_println!("[fork] OOM in clone_page_table for child {}", child_pid);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// do_fork — main entry point
// ---------------------------------------------------------------------------

/// Perform fork/clone.
///
/// Returns the child PID on success (in the parent).  The child's PCB is
/// inserted into the process table with `context.rax = 0` so the child
/// syscall return path sees 0.
///
/// Steps:
///   1. Allocate a new PID.
///   2. Deep-copy the parent PCB via `Process::fork()`.
///   3. Set child `context.rax = 0` (fork return value in child).
///   4. If `!flags.share_vm`: duplicate page tables with COW.
///   5. If `flags.share_vm` (thread): share the parent's page_table pointer.
///   6. If `!flags.share_files`: the FD table was already deep-copied by fork().
///   7. Record child PID in parent's children list.
///   8. Insert child PCB into the process table.
///   9. Enqueue child in the scheduler run queue.
///  10. Return child PID.
pub fn do_fork(flags: CloneFlags) -> Result<u32, &'static str> {
    let parent_pid = SCHEDULER.lock().current();
    crate::serial_println!(
        "[fork] do_fork: parent={} share_vm={} share_files={}",
        parent_pid,
        flags.share_vm,
        flags.share_files
    );

    // ---- Step 1: allocate PID -------------------------------------------
    let child_pid = alloc_pid().ok_or("fork: process table full — no free PID")?;
    crate::serial_println!("[fork] allocated child PID {}", child_pid);

    // ---- Steps 2-3: deep-copy parent PCB ---------------------------------
    let mut child = {
        let table = PROCESS_TABLE.lock();
        let parent = table[parent_pid as usize]
            .as_ref()
            .ok_or("fork: parent PCB not found")?;
        parent.fork(child_pid)
    };

    // POSIX fork contract: child sees 0 as the fork() return value.
    child.context.rax = 0;

    // ---- Step 4/5: page tables -------------------------------------------
    if flags.share_vm {
        // Thread: inherit parent's page table pointer unchanged.
        // child.page_table is already set by Process::fork() to parent's value.
        crate::serial_println!(
            "[fork] child {} shares VM with parent {} (thread semantics)",
            child_pid,
            parent_pid
        );
    } else {
        // Process: COW duplicate.
        match copy_page_tables_cow(parent_pid, child_pid) {
            Some(child_pml4) => {
                child.page_table = child_pml4;
                crate::serial_println!("[fork] child {} page table: 0x{:x}", child_pid, child_pml4);
            }
            None => {
                crate::serial_println!("[fork] OOM during COW page table copy — fork aborted");
                return Err("fork: out of memory during page table duplication");
            }
        }
    }

    // ---- Step 6: file descriptors ----------------------------------------
    // Process::fork() already deep-cloned the FD table.  For share_files
    // (thread), both parent and child point to independent copies of the
    // same FDs which is acceptable at this stage.
    // TODO(ipc): implement shared FD table reference counting for true threads.

    // ---- Step 7: record child in parent's children list ------------------
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(parent) = table[parent_pid as usize].as_mut() {
            parent.children.push(child_pid);
            // Parent sees the child PID as the fork() return value.
            parent.context.rax = child_pid as u64;
        }
    }

    // ---- Steps 8-9: insert child and enqueue ----------------------------
    {
        let mut table = PROCESS_TABLE.lock();
        child.state = ProcessState::Ready;
        table[child_pid as usize] = Some(child);
    }
    SCHEDULER.lock().add(child_pid);

    crate::serial_println!(
        "[fork] fork complete: parent={} child={}",
        parent_pid,
        child_pid
    );
    Ok(child_pid)
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the fork subsystem.
pub fn init() {
    // Reset PID counter; PID 0 (idle) is created by process::init() before
    // this runs, and PID 1 (init) is reserved.  Real allocations start at 2.
    NEXT_PID.store(2, Ordering::Relaxed);
    crate::serial_println!("  fork: subsystem ready (PID counter reset to 2)");
}
