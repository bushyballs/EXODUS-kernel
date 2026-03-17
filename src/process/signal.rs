use super::pcb::{signal, Process, ProcessState, SignalAction};
/// POSIX signal delivery, dispatch, and handler management.
///
/// Part of the AIOS kernel.
///
/// Architecture
/// ------------
/// Signals are stored in two complementary locations:
///
///   1. `Process::pending_signals` (bitmask in pcb.rs) — fast one-bit-per-signal
///      set used by the scheduler's deliver_signals / deliver_signals_full.
///
///   2. `SignalState` (this file) — optional queue of `QueuedSignal` records
///      with per-signal metadata (sender PID, sigval).  Used by sigqueue(3)
///      and real-time signals.  Standard (non-RT) signals collapse to one
///      pending entry.
///
/// `deliver_pending_signals` in this module bridges the two by operating on
/// a `Process` reference (from pcb.rs) rather than a standalone SignalState.
/// The higher-level `process::deliver_signals_full` (in process/mod.rs) is
/// the primary call site from the scheduler.
///
/// Signal frame layout (x86_64)
/// ----------------------------
/// When a user-space handler is invoked the kernel pushes a `SignalFrame`
/// onto the user stack.  The frame stores the interrupted register context
/// so that `rt_sigreturn` can restore it after the handler returns.
///
/// Stack layout after `build_sigframe()` (addresses grow downward):
///
///   [original RSP]
///       RSP - 8   : original RIP (return address for trampoline)
///       RSP - 16  : original RSP
///       RSP - 24  : RFLAGS
///       RSP - 32  : saved signal number (u64, for rt_sigreturn)
///       RSP - 40  : trampoline return address (SIGNAL_RETURN_TRAMPOLINE_ADDR)
///   ← new RSP returned to caller
///
/// The trampoline address is a well-known virtual address where the kernel
/// maps a two-instruction stub:
///   mov eax, SYS_rt_sigreturn (15)
///   syscall
///
/// On `rt_sigreturn` the kernel reads the frame back from the user stack and
/// restores RIP / RSP / RFLAGS before returning to user space.
///
/// `SigInfo` / siginfo_t
/// ---------------------
/// For SA_SIGINFO handlers the second argument (rsi) points to a `SigInfo`
/// built by `build_siginfo()`.  The third argument (rdx) would point to a
/// `ucontext_t`; that is left as a stub until full FPU-state save is wired.
///
/// Signal return
/// -------------
/// `restore_sigframe(frame_ptr: u64)` reads a `SignalFrame` written by
/// `build_sigframe()` and reconstructs the `CpuContext` to be loaded on the
/// IRET path.  Called from the `rt_sigreturn` syscall handler.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// SignalDisposition — lightweight alias used by older call sites
// ---------------------------------------------------------------------------

/// Simple three-way signal disposition (used by `SignalState`).
#[derive(Debug, Clone, Copy)]
pub enum SignalDisposition {
    /// Use the default action for this signal.
    Default,
    /// Ignore the signal.
    Ignore,
    /// Call a user-space handler at the given virtual address.
    Handler(usize),
}

// ---------------------------------------------------------------------------
// QueuedSignal — per-signal metadata record
// ---------------------------------------------------------------------------

/// A signal waiting to be delivered, with sender info and optional value.
pub struct QueuedSignal {
    /// Signal number (1-31 for standard signals).
    pub signo: u8,
    /// PID of the process that sent the signal (0 = kernel).
    pub sender_pid: u32,
    /// Optional integer value (for POSIX real-time signals / sigqueue).
    pub value: i32,
}

// ---------------------------------------------------------------------------
// SignalState — per-process rich signal queue
// ---------------------------------------------------------------------------

/// Maximum queued signals before drops occur (matches POSIX SIGQUEUE_MAX floor).
const MAX_PENDING_SIGNALS: usize = 32;

/// Per-process signal state (rich queue + mask + dispositions).
pub struct SignalState {
    /// Pending signal queue (ordered by arrival, not by number).
    pub pending: Vec<QueuedSignal>,
    /// Blocked signal mask — bit N set means signal N+1 is blocked.
    pub mask: u32,
    /// Per-signal dispositions (index = signal number, 1-based, index 0 unused).
    pub dispositions: [SignalDisposition; 32],
}

impl SignalState {
    /// Create a fresh SignalState with all signals at SIG_DFL and mask = 0.
    pub fn new() -> Self {
        SignalState {
            pending: Vec::new(),
            mask: 0,
            dispositions: [SignalDisposition::Default; 32],
        }
    }

    /// Add a signal to the pending queue if it is not masked.
    ///
    /// Silently drops if:
    ///   - The signal number is invalid (0 or > 31).
    ///   - The signal is blocked by `mask`.
    ///   - The queue is already at `MAX_PENDING_SIGNALS` (POSIX-compliant drop).
    pub fn enqueue(&mut self, sig: QueuedSignal) {
        let signo = sig.signo as usize;
        if signo == 0 || signo > 31 {
            return;
        }
        // Check if blocked by mask (mask bit N corresponds to signal N+1).
        if self.mask & (1u32 << (signo.saturating_sub(1))) != 0 {
            return;
        }
        if self.pending.len() >= MAX_PENDING_SIGNALS {
            return; // Queue saturated — drop (standard UNIX behaviour).
        }
        self.pending.push(sig);
    }

    /// Dequeue the highest-priority (lowest signo) unmasked pending signal.
    ///
    /// Returns `None` if the queue is empty or all pending signals are masked.
    pub fn dequeue(&mut self) -> Option<QueuedSignal> {
        let mask = self.mask;
        let mut best_idx: Option<usize> = None;
        let mut best_signo: u8 = u8::MAX;

        for (i, s) in self.pending.iter().enumerate() {
            if s.signo == 0 || s.signo > 31 {
                continue;
            }
            let bit = (s.signo.saturating_sub(1)) as u32;
            if mask & (1u32 << bit) != 0 {
                continue; // masked
            }
            if s.signo < best_signo {
                best_signo = s.signo;
                best_idx = Some(i);
            }
        }

        best_idx.map(|i| self.pending.remove(i))
    }

    /// Check whether any signals are pending and unmasked.
    pub fn has_pending(&self) -> bool {
        self.pending.iter().any(|s| {
            s.signo > 0 && s.signo <= 31 && (self.mask & (1u32 << (s.signo.saturating_sub(1)))) == 0
        })
    }

    /// Set the disposition for a specific signal.
    ///
    /// SIGKILL (9) and SIGSTOP (19) cannot be overridden; calls for those
    /// signals are silently ignored.
    pub fn set_disposition(&mut self, signo: u8, disp: SignalDisposition) {
        if signo == 0 || signo > 31 {
            return;
        }
        if signal::is_uncatchable(signo) {
            return; // SIGKILL / SIGSTOP cannot be caught or ignored.
        }
        self.dispositions[signo as usize] = disp;
    }

    /// Get the current disposition for a signal.
    pub fn get_disposition(&self, signo: u8) -> SignalDisposition {
        if signo == 0 || signo > 31 {
            return SignalDisposition::Default;
        }
        self.dispositions[signo as usize]
    }
}

// ---------------------------------------------------------------------------
// Signal frame — user-stack layout pushed before entering a handler
// ---------------------------------------------------------------------------

/// Virtual address of the signal-return trampoline page mapped into every
/// user process.  The page contains:
///   mov eax, 15   ; SYS_rt_sigreturn
///   syscall
pub const SIGNAL_RETURN_TRAMPOLINE_ADDR: u64 = 0x0000_7FFF_FFFF_F000;

/// Saved context written onto the user stack by `build_sigframe()`.
///
/// The layout is stable: `restore_sigframe()` must read fields at the same
/// offsets.  All fields are u64 so alignment is trivially 8-byte everywhere.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame {
    /// Instruction pointer at the point the signal interrupted user code.
    pub saved_rip: u64,
    /// Stack pointer at the point the signal interrupted user code.
    pub saved_rsp: u64,
    /// RFLAGS at the point the signal interrupted user code.
    pub saved_rflags: u64,
    /// Signal number — read back by `rt_sigreturn` to restore the mask.
    pub signo: u64,
    /// Return address placed at the top of the new stack frame.
    /// Points to `SIGNAL_RETURN_TRAMPOLINE_ADDR` so the handler's `ret`
    /// instruction naturally jumps into the trampoline.
    pub trampoline_return: u64,
}

impl SignalFrame {
    /// Size of the frame in bytes.
    pub const SIZE: u64 = core::mem::size_of::<SignalFrame>() as u64;
}

/// Push a `SignalFrame` onto the user stack and return the adjusted RSP.
///
/// # Arguments
/// * `stack_ptr`  — current user RSP (the frame is placed *below* this).
/// * `sig`        — signal number being delivered.
/// * `saved_rip`  — original user RIP to restore after the handler.
/// * `saved_rsp`  — original user RSP to restore after the handler.
/// * `saved_rflags` — RFLAGS to restore after the handler.
///
/// # Returns
/// The new RSP the kernel should load into the interrupted context before
/// jumping to the signal handler.  The handler's return address at `[RSP]`
/// is already set to `SIGNAL_RETURN_TRAMPOLINE_ADDR`.
///
/// # Safety
/// The caller must guarantee that `stack_ptr` is a valid user-space address
/// with at least `SignalFrame::SIZE + 8` bytes of writable space below it.
/// This function writes directly to that user memory.
pub fn build_sigframe(
    stack_ptr: u64,
    sig: u32,
    saved_rip: u64,
    saved_rsp: u64,
    saved_rflags: u64,
) -> u64 {
    // Align the new RSP to 16 bytes (ABI requirement) then carve out room
    // for the frame.
    let aligned = (stack_ptr & !0xF_u64).saturating_sub(SignalFrame::SIZE);
    // Further align to 16 after frame (the call-entry ABI expects RSP % 16 == 8
    // at the point the handler executes its first instruction).
    let new_rsp = aligned & !0xF_u64;

    let frame = SignalFrame {
        saved_rip,
        saved_rsp,
        saved_rflags,
        signo: sig as u64,
        trampoline_return: SIGNAL_RETURN_TRAMPOLINE_ADDR,
    };

    // Write the frame to user memory.
    // SAFETY: caller guarantees the address range is valid and mapped writable.
    unsafe {
        let dst = new_rsp as *mut SignalFrame;
        core::ptr::write_volatile(dst, frame);
    }

    crate::serial_println!(
        "[signal] sigframe @ 0x{:016X}: rip=0x{:X} rsp=0x{:X} sig={}",
        new_rsp,
        saved_rip,
        saved_rsp,
        sig
    );

    new_rsp
}

/// Restore the interrupted context from a `SignalFrame` on the user stack.
///
/// Called from the `rt_sigreturn` syscall handler after the signal handler
/// has returned through the trampoline.
///
/// # Arguments
/// * `frame_ptr` — user RSP pointing at the base of the `SignalFrame` that
///                 was written by `build_sigframe()`.
///
/// # Returns
/// A tuple `(rip, rsp, rflags, signo)` that the syscall return path should
/// load into the interrupted context before resuming user code.
///
/// # Safety
/// The caller must verify that `frame_ptr` is a valid, mapped, readable user
/// address.  No capability checks are performed here.
pub fn restore_sigframe(frame_ptr: u64) -> (u64, u64, u64, u32) {
    // SAFETY: caller guarantees the address is valid user memory.
    let frame: SignalFrame = unsafe { core::ptr::read_volatile(frame_ptr as *const SignalFrame) };

    crate::serial_println!(
        "[signal] rt_sigreturn: restoring rip=0x{:X} rsp=0x{:X} sig={}",
        frame.saved_rip,
        frame.saved_rsp,
        frame.signo
    );

    (
        frame.saved_rip,
        frame.saved_rsp,
        frame.saved_rflags,
        frame.signo as u32,
    )
}

// ---------------------------------------------------------------------------
// SigInfo — siginfo_t equivalent (SI_* fields)
// ---------------------------------------------------------------------------

/// Kernel-internal equivalent of POSIX `siginfo_t`.
///
/// Passed as the second argument (rsi) to SA_SIGINFO handlers.  The layout
/// matches the first three meaningful fields of the Linux `siginfo_t` ABI so
/// that a user-space libc can interpret the struct directly once more fields
/// are added.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SigInfo {
    /// Signal number (`si_signo`).
    pub si_signo: i32,
    /// Error number associated with the signal (`si_errno`), or 0.
    pub si_errno: i32,
    /// Signal code describing the origin (`si_code`).
    /// Common values: SI_USER (0), SI_KERNEL (0x80), SI_QUEUE (-1).
    pub si_code: i32,
    /// Padding to keep the struct at least 128 bytes (POSIX minimum).
    _pad: [u8; 116],
}

impl SigInfo {
    /// SI_USER — signal sent by kill(2) or raise(3).
    pub const SI_USER: i32 = 0;
    /// SI_KERNEL — signal sent by the kernel.
    pub const SI_KERNEL: i32 = 0x80;
    /// SI_QUEUE — signal sent via sigqueue(2).
    pub const SI_QUEUE: i32 = -1;
}

/// Construct a `SigInfo` with the given signo / errno / code.
///
/// All padding bytes are zeroed.  The resulting struct can be written to the
/// user stack and its address passed in `rsi` when entering a SA_SIGINFO
/// handler.
pub fn build_siginfo(sig: u32, errno: i32, code: i32) -> SigInfo {
    SigInfo {
        si_signo: sig as i32,
        si_errno: errno,
        si_code: code,
        _pad: [0u8; 116],
    }
}

// ---------------------------------------------------------------------------
// deliver_pending_signals — PCB-level delivery
// ---------------------------------------------------------------------------

/// Deliver all pending, unmasked signals for the given process.
///
/// This operates directly on a `Process` (from pcb.rs) rather than on a
/// standalone `SignalState`, because the PCB's `signal_handlers` BTreeMap
/// is the authoritative disposition table.
///
/// For each pending signal (low-priority first) that is not masked:
///
///   `SIG_DFL`:
///     - Fatal signals (SIGKILL, SIGTERM, SIGINT, SIGSEGV, …): mark Dead,
///       set exit_code = -(signo as i32).
///     - Stop signals (SIGSTOP, SIGTSTP, …): mark Stopped.
///     - Ignore-by-default signals (SIGCHLD, SIGWINCH, …): clear and continue.
///
///   `SIG_IGN`:
///     - Clear the pending bit and continue.
///
///   User handler (Custom / CustomInfo):
///     - In a real kernel: build a signal frame on the user stack and arrange
///       for a trampoline to call the handler, then IRET to user space.
///     - Here: log the intent, update the signal mask to block the handler's
///       sa_mask during delivery, and clear the pending bit.  The SA_RESETHAND
///       flag resets the disposition to SIG_DFL after clearing.
///     - TODO(signals): implement actual user-space signal frame setup when
///       the IRET/sysret path and TSS ring-3 stack are finalised.
///
/// Returns `true` if the process was killed by a signal (caller should
/// switch to the next runnable process).
pub fn deliver_pending_signals(proc: &mut Process) -> bool {
    // Loop until no more deliverable signals remain.
    loop {
        let sig = match proc.dequeue_signal() {
            Some(s) => s,
            None => return false, // nothing left to deliver
        };

        let handler = proc.get_signal_handler(sig);
        proc.rusage.signals_delivered = proc.rusage.signals_delivered.saturating_add(1);

        crate::serial_println!(
            "[signal] delivering sig={} ({}) to PID {}",
            sig,
            signal::name(sig),
            proc.pid
        );

        match handler.action {
            // ---- SIG_IGN ------------------------------------------------
            SignalAction::Ignore => {
                // Simply discard.
                continue;
            }

            // ---- SIG_DFL ------------------------------------------------
            SignalAction::Default => {
                if signal::is_fatal_default(sig) {
                    // Terminate the process.
                    proc.state = ProcessState::Dead;
                    proc.exit_code = -(sig as i32);
                    crate::serial_println!(
                        "[signal] PID {} killed by {} (SIG_DFL fatal)",
                        proc.pid,
                        signal::name(sig)
                    );
                    // Caller must close FDs and remove from scheduler.
                    return true;
                }

                if signal::is_stop_default(sig) {
                    // Stop the process (job control).
                    proc.stopped = true;
                    proc.state = ProcessState::Stopped;
                    crate::serial_println!(
                        "[signal] PID {} stopped by {} (SIG_DFL stop)",
                        proc.pid,
                        signal::name(sig)
                    );
                    // Not killed — but caller should remove from run queue.
                    return false;
                }

                // SIGCHLD, SIGWINCH, SIGURG, SIGCONT (when continued):
                // default action is "ignore" or "continue" — do nothing.
                continue;
            }

            // ---- User handler (Custom) -----------------------------------
            SignalAction::Custom {
                handler: handler_addr,
            } => {
                crate::serial_println!(
                    "[signal] PID {} custom handler sig={} addr=0x{:016X}",
                    proc.pid,
                    sig,
                    handler_addr
                );
                // Block sa_mask + the signal itself while in the handler.
                let prev_mask = proc.signal_mask;
                let block_during = handler.sa_mask | (1u32 << (sig as u32));
                proc.block_signals(block_during);
                proc.in_signal_handler = true;
                proc.signal_depth = proc.signal_depth.saturating_add(1);

                // --- User-space signal frame setup -------------------------
                // Capture the current user-space register state from the PCB,
                // push a SignalFrame onto the user stack, then redirect the
                // saved RIP to `handler_addr` so the IRET path enters the
                // handler.  When the handler executes `ret`, it jumps to the
                // trampoline which issues `rt_sigreturn`, and
                // `restore_sigframe()` reconstructs the original context.
                let user_rip = proc.context.rip;
                let user_rsp = proc.context.rsp;
                let user_rflags = proc.context.rflags;

                let new_rsp = build_sigframe(user_rsp, sig as u32, user_rip, user_rsp, user_rflags);

                // Redirect context so IRET resumes at the handler.
                proc.context.rip = handler_addr as u64;
                proc.context.rsp = new_rsp;
                // Pass signal number in rdi (first argument, System V AMD64 ABI).
                proc.context.rdi = sig as u64;
                // -----------------------------------------------------------

                // SA_RESETHAND: reset to SIG_DFL after first delivery.
                // Do this BEFORE returning so the disposition is correct if
                // the handler itself sends the same signal.
                if handler.flags.reset_hand {
                    proc.signal_handlers.remove(&sig);
                }

                // Note: prev_mask and signal_depth are restored by
                // rt_sigreturn → restore_signal_context(), not here.
                // Store them in the process so the return path can find them.
                proc.context.saved_signal_mask = prev_mask as u64;
                proc.context.saved_signal_depth = proc.signal_depth;

                continue;
            }

            // ---- User handler (CustomInfo / SA_SIGINFO) ------------------
            SignalAction::CustomInfo {
                handler: handler_addr,
            } => {
                crate::serial_println!(
                    "[signal] PID {} SA_SIGINFO handler sig={} addr=0x{:016X}",
                    proc.pid,
                    sig,
                    handler_addr
                );
                let prev_mask = proc.signal_mask;
                let block_during = handler.sa_mask | (1u32 << (sig as u32));
                proc.block_signals(block_during);
                proc.in_signal_handler = true;
                proc.signal_depth = proc.signal_depth.saturating_add(1);

                // --- siginfo_t construction + signal frame ------------------
                // Build a SigInfo and write it to the user stack *above* the
                // SignalFrame so the handler receives a pointer to it in rsi.
                //
                // Stack layout after this block (addresses grow downward):
                //
                //   [original RSP]
                //       - sizeof(SigInfo)     ← siginfo written here
                //       - sizeof(SignalFrame) ← signal frame written here
                //   ← new RSP / rsp handed to handler
                //
                let user_rip = proc.context.rip;
                let user_rsp = proc.context.rsp;
                let user_rflags = proc.context.rflags;

                // Carve out room for SigInfo above the signal frame.
                let siginfo_size = core::mem::size_of::<SigInfo>() as u64;
                let siginfo_rsp = (user_rsp & !0xF_u64).saturating_sub(siginfo_size);

                let info = build_siginfo(sig as u32, 0, SigInfo::SI_KERNEL);
                // SAFETY: siginfo_rsp is a valid user address (caller's responsibility).
                unsafe {
                    core::ptr::write_volatile(siginfo_rsp as *mut SigInfo, info);
                }

                // Now push the standard signal frame below the SigInfo slot.
                let new_rsp =
                    build_sigframe(siginfo_rsp, sig as u32, user_rip, user_rsp, user_rflags);

                // Set up handler arguments (System V AMD64 ABI):
                //   rdi = signo
                //   rsi = pointer to siginfo_t
                //   rdx = pointer to ucontext_t (stub: 0 until FPU save is wired)
                proc.context.rip = handler_addr as u64;
                proc.context.rsp = new_rsp;
                proc.context.rdi = sig as u64;
                proc.context.rsi = siginfo_rsp;
                proc.context.rdx = 0; // ucontext stub — extend when ucontext_t is defined
                                      // -----------------------------------------------------------

                // Store context for rt_sigreturn.
                proc.context.saved_signal_mask = prev_mask as u64;
                proc.context.saved_signal_depth = proc.signal_depth;

                if handler.flags.reset_hand {
                    proc.signal_handlers.remove(&sig);
                }

                continue;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience: send a signal to a process by PID
// ---------------------------------------------------------------------------

/// Queue a signal to a process identified by PID.
///
/// Uses the global `PROCESS_TABLE` lock.  Returns `Err` if the PID is not
/// found or the signal number is invalid.
pub fn send_signal_to(pid: u32, signo: u8) -> Result<(), &'static str> {
    if signo == 0 || signo > 31 {
        return Err("signal: invalid signal number");
    }

    let mut table = super::pcb::PROCESS_TABLE.lock();
    let proc = table[pid as usize]
        .as_mut()
        .ok_or("signal: no such process")?;

    // SIGKILL and SIGSTOP are always delivered, never masked.
    if signo == signal::SIGKILL {
        proc.state = ProcessState::Dead;
        proc.exit_code = -9;
        crate::serial_println!("[signal] SIGKILL → PID {} (immediate)", pid);
        return Ok(());
    }

    proc.send_signal(signo);
    crate::serial_println!(
        "[signal] queued sig={} ({}) to PID {}",
        signo,
        signal::name(signo),
        pid
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the signal subsystem.
pub fn init() {
    crate::serial_println!("  signal: subsystem ready");
}
