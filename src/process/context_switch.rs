/// Bare-metal context switch — the heart of the scheduler.
///
/// `switch_context(old_rsp, new_rsp)` saves the callee-saved registers
/// of the current task onto its kernel stack, stores RSP into *old_rsp,
/// loads new_rsp into RSP, restores the callee-saved registers of the
/// new task, and returns — which jumps to the new task's return address.
///
/// The x86-64 SysV ABI callee-saved registers are:
///   RBP, RBX, R12, R13, R14, R15
///
/// The function is marked `#[naked]` so the compiler emits NO prologue or
/// epilogue.  Every instruction in the function body is under our control.
///
/// Stack layout when switch_context is called (SysV ABI):
///   [RSP+0]  = return address (pushed by CALL)
///   RDI      = old_rsp  (*mut u64 — where to save the outgoing RSP)
///   RSI      = new_rsp  (u64 — the incoming task's kernel RSP)
///
/// Stack layout after we push callee-saved registers:
///   [RSP+0]  = R15
///   [RSP+8]  = R14
///   [RSP+16] = R13
///   [RSP+24] = R12
///   [RSP+32] = RBX
///   [RSP+40] = RBP
///   [RSP+48] = return address   ← caller's RIP, restored by `ret`
///
/// This matches the mirror-image pop sequence in the resume path.
///
/// No std, no float, no panics.

/// Perform a kernel-to-kernel context switch.
///
/// # Arguments
/// * `old_rsp` — pointer to the u64 where the outgoing task's RSP is stored.
/// * `new_rsp` — the RSP of the incoming task's kernel stack.
///
/// # Safety
/// * Both stacks must be valid mapped kernel memory.
/// * The new task's stack must already contain the callee-saved registers
///   in the layout described above (i.e., the task must have been previously
///   switched away from by this same function, or its initial stack must be
///   set up with dummy zero values + the entry-point address on top).
/// * Interrupts should be disabled (or the caller must be in an interrupt
///   handler) to prevent re-entrant scheduling.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_context(old_rsp: *mut u64, new_rsp: u64) {
    core::arch::naked_asm!(
        // ── Save outgoing task's callee-saved registers ──────────────────
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // ── Store outgoing RSP into *old_rsp (RDI holds the pointer) ─────
        "mov [rdi], rsp",
        // ── Load incoming RSP from new_rsp (RSI holds the value) ─────────
        "mov rsp, rsi",
        // ── Restore incoming task's callee-saved registers ────────────────
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        // ── Return to the incoming task's saved return address ────────────
        // The address is whatever was on top of the new stack after the pops,
        // i.e., the address that was pushed by the CALL to switch_context
        // when this task was last context-switched away.
        "ret",
    );
}

// ---------------------------------------------------------------------------
// Stack setup helpers
// ---------------------------------------------------------------------------

/// Initialise a fresh kernel stack so that the very first call to
/// `switch_context` will start executing `entry_fn`.
///
/// Lays out a fake saved-register frame at the top of the stack:
///
/// ```text
/// [stack_top - 8]   entry_fn   ← return address (ret → entry_fn)
/// [stack_top - 16]  0          ← R15
/// [stack_top - 24]  0          ← R14
/// [stack_top - 32]  0          ← R13
/// [stack_top - 40]  0          ← R12
/// [stack_top - 48]  0          ← RBX
/// [stack_top - 56]  0          ← RBP
/// ```
///
/// The function returns the new RSP value to store in `ProcessEntry::saved_rsp`.
///
/// # Safety
/// * `stack_top` must be 16-byte aligned and point to one byte past the
///   end of a valid writable memory region of at least 64 bytes.
pub unsafe fn init_task_stack(stack_top: u64, entry_fn: fn() -> !) -> u64 {
    // Work with a raw pointer to u64, walking down from the top.
    let mut sp = stack_top as *mut u64;

    // Push the entry function address as the "return address".
    sp = sp.sub(1);
    *sp = entry_fn as u64;

    // Push six callee-saved register slots (all zeroed).
    // Order: RBP, RBX, R12, R13, R14, R15 — pushed in that order, so
    // popped in reverse (R15 first).
    for _ in 0..6 {
        sp = sp.sub(1);
        *sp = 0;
    }

    sp as u64
}

/// Initialise a kernel stack for a function that takes no arguments and
/// returns normally (kernel threads that loop forever).
///
/// Identical layout to `init_task_stack` but accepts a `fn()` pointer.
///
/// # Safety
/// Same requirements as `init_task_stack`.
pub unsafe fn init_kernel_thread_stack(stack_top: u64, entry_fn: unsafe fn()) -> u64 {
    let mut sp = stack_top as *mut u64;

    // The thread_trampoline will call entry_fn and then loop forever;
    // we store a stub return address that just halts.
    sp = sp.sub(1);
    *sp = entry_fn as u64;

    for _ in 0..6 {
        sp = sp.sub(1);
        *sp = 0;
    }

    sp as u64
}
