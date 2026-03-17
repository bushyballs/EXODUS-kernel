// signalfd — Linux-compatible signal file descriptors
//
// Redirects signals matching a bitmask into a readable file descriptor
// instead of delivering them to the process's signal handler.  This
// allows event-driven programs (using poll/select/epoll) to handle
// signals synchronously via normal I/O rather than async signal
// handlers.
//
// Design:
//   - A per-fd pending queue stores undelivered captured signals.
//   - deliver_to_signalfds() is called from deliver_signals_full() before
//     the normal signal handler path; any signal whose bit is set in
//     tfd.sigset is captured here instead.
//   - signalfd_read() returns a packed siginfo_t-compatible record for
//     each captured signal.
//
// siginfo record layout (64 bytes, matches Linux siginfo_t subset):
//   offset  0  u32  si_signo  — signal number
//   offset  4  u32  si_errno  — errno (always 0 here)
//   offset  8  u32  si_code   — SI_KERNEL (0x80)
//   offset 12  u32  _pad0
//   offset 16  u32  si_pid    — sender PID (always 0 here)
//   offset 20  u32  si_uid    — sender UID (always 0 here)
//   offset 24 [u8; 40] _pad   — reserved, zero-filled
//
// Inspired by: Linux signalfd(2). All code is original.

use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Close on exec flag.
pub const SFD_CLOEXEC: u32 = 0x80000;
/// Non-blocking reads.
pub const SFD_NONBLOCK: u32 = 0x800;

const EAGAIN: i32 = 11;
const EINVAL: i32 = 22;
const EMFILE: i32 = 24;
const EBADF: i32 = 9;
const EFAULT: i32 = 14;

/// Size of one signal record in bytes (matches Linux signalfd_siginfo).
pub const SIGINFO_SIZE: usize = 64;

/// Maximum simultaneously open signalfds.
const MAX_SIGNALFDS: usize = 64;

/// Maximum pending signals queued per signalfd before oldest are dropped.
const MAX_PENDING: usize = 256;

// ---------------------------------------------------------------------------
// SignalFd record
// ---------------------------------------------------------------------------

/// One signalfd file descriptor.
pub struct SignalFd {
    /// File descriptor number (1-based).
    pub fd: u32,
    /// Owner process PID.
    pub owner_pid: u32,
    /// Bit-mask of captured signal numbers (bit N corresponds to signal N).
    pub sigset: u64,
    /// Creation flags (SFD_NONBLOCK, SFD_CLOEXEC).
    pub flags: u32,
    /// Pending captured signals (oldest first).
    pending: VecDeque<u8>,
    /// Whether the fd is still open.
    pub active: bool,
}

impl SignalFd {
    fn new(fd: u32, owner_pid: u32, sigset: u64, flags: u32) -> Self {
        SignalFd {
            fd,
            owner_pid,
            sigset,
            flags,
            pending: VecDeque::new(),
            active: true,
        }
    }

    fn is_nonblock(&self) -> bool {
        self.flags & SFD_NONBLOCK != 0
    }

    /// Returns true if signal `sig` is captured by this fd.
    fn captures(&self, sig: u8) -> bool {
        sig < 64 && (self.sigset >> sig) & 1 == 1
    }

    /// Enqueue a signal for later reading. Drops oldest if queue is full.
    fn enqueue(&mut self, sig: u8) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.pop_front(); // drop oldest
        }
        self.pending.push_back(sig);
    }

    /// Dequeue and serialise one pending signal into `buf`.
    ///
    /// Writes a 64-byte siginfo record into `buf[0..SIGINFO_SIZE]`.
    /// Returns the number of bytes written, or 0 if no pending signal.
    fn dequeue_into(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < SIGINFO_SIZE {
            return 0;
        }
        let sig = match self.pending.pop_front() {
            Some(s) => s,
            None => return 0,
        };

        // Zero-fill the record first.
        for b in buf[..SIGINFO_SIZE].iter_mut() {
            *b = 0;
        }

        // si_signo (u32 LE at offset 0)
        let signo = sig as u32;
        buf[0] = (signo & 0xFF) as u8;
        buf[1] = ((signo >> 8) & 0xFF) as u8;
        buf[2] = ((signo >> 16) & 0xFF) as u8;
        buf[3] = ((signo >> 24) & 0xFF) as u8;
        // si_errno (u32 LE at offset 4): always 0 — already zeroed.
        // si_code  (u32 LE at offset 8): SI_KERNEL = 0x80
        buf[8] = 0x80;
        // si_pid / si_uid at offset 16/20: already 0.
        SIGINFO_SIZE
    }

    /// Returns true if there is at least one pending signal.
    fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Global signalfd table
// ---------------------------------------------------------------------------

struct SignalFdTable {
    slots: [Option<SignalFd>; MAX_SIGNALFDS],
    next_fd: u32,
}

impl SignalFdTable {
    const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const NONE_SLOT: Option<SignalFd> = None;
        SignalFdTable {
            slots: [NONE_SLOT; MAX_SIGNALFDS],
            next_fd: 1,
        }
    }

    fn alloc(&mut self, pid: u32, sigset: u64, flags: u32) -> Result<u32, i32> {
        for _ in 0..MAX_SIGNALFDS {
            let idx = (self.next_fd as usize).saturating_sub(1) % MAX_SIGNALFDS;
            self.next_fd = (self.next_fd % MAX_SIGNALFDS as u32).saturating_add(1);
            if self.slots[idx].is_none() {
                let fd = (idx as u32).saturating_add(1);
                self.slots[idx] = Some(SignalFd::new(fd, pid, sigset, flags));
                return Ok(fd);
            }
        }
        Err(EMFILE)
    }

    fn slot_mut(&mut self, fd: u32) -> Option<&mut SignalFd> {
        if fd == 0 {
            return None;
        }
        let idx = (fd as usize).saturating_sub(1);
        if idx >= MAX_SIGNALFDS {
            return None;
        }
        self.slots[idx].as_mut().filter(|s| s.fd == fd && s.active)
    }

    fn free(&mut self, fd: u32) {
        if fd == 0 {
            return;
        }
        let idx = (fd as usize).saturating_sub(1);
        if idx < MAX_SIGNALFDS {
            self.slots[idx] = None;
        }
    }

    /// Deliver a signal to any signalfds owned by `pid` that capture it.
    ///
    /// Returns `true` if at least one signalfd captured the signal (so the
    /// caller can skip normal signal handler delivery).
    fn deliver(&mut self, pid: u32, sig: u8) -> bool {
        let mut captured = false;
        for slot in self.slots.iter_mut() {
            if let Some(sfd) = slot.as_mut() {
                if sfd.active && sfd.owner_pid == pid && sfd.captures(sig) {
                    sfd.enqueue(sig);
                    captured = true;
                }
            }
        }
        captured
    }
}

// SAFETY: accessed only through the Mutex guard.
unsafe impl Send for SignalFdTable {}

static SIGNALFD_TABLE: Mutex<SignalFdTable> = Mutex::new(SignalFdTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a signalfd that captures signals matching `sigset`.
///
/// `sigset` is a bitmask: bit N is set to capture signal N.
/// Signals in SIGKILL (9) and SIGSTOP (19) cannot be captured; those bits
/// are silently cleared.
///
/// `flags` may include SFD_NONBLOCK and SFD_CLOEXEC.
///
/// Returns the new fd on success, or a negative errno.
pub fn signalfd_create(pid: u32, mut sigset: u64, flags: u32) -> Result<u32, i32> {
    // SIGKILL (9) and SIGSTOP (19) cannot be intercepted.
    sigset &= !(1u64 << 9) & !(1u64 << 19);
    if sigset == 0 {
        return Err(EINVAL);
    }
    SIGNALFD_TABLE.lock().alloc(pid, sigset, flags)
}

/// Read one or more pending signal records from a signalfd.
///
/// Reads as many complete 64-byte records as fit into `buf` or are
/// available.  Returns the total bytes written.
///
/// Errors:
///   EBADF  — fd does not name an open signalfd owned by the caller.
///   EAGAIN — no pending signals and the fd is SFD_NONBLOCK.
///   EFAULT — `buf` is too small for even one record.
pub fn signalfd_read(fd: u32, pid: u32, buf: &mut [u8]) -> Result<usize, i32> {
    if buf.len() < SIGINFO_SIZE {
        return Err(EFAULT);
    }

    let mut tbl = SIGNALFD_TABLE.lock();
    let sfd = tbl.slot_mut(fd).ok_or(EBADF)?;
    if sfd.owner_pid != pid {
        return Err(EBADF);
    }

    if !sfd.has_pending() {
        if sfd.is_nonblock() {
            return Err(EAGAIN);
        }
        // Blocking: tell caller to sleep (EAGAIN as wake-on-signal hint).
        return Err(EAGAIN);
    }

    let mut written = 0usize;
    while written.saturating_add(SIGINFO_SIZE) <= buf.len() {
        let n = sfd.dequeue_into(&mut buf[written..]);
        if n == 0 {
            break;
        }
        written = written.saturating_add(n);
    }
    Ok(written)
}

/// Close a signalfd and release its resources.
pub fn signalfd_close(fd: u32, pid: u32) -> Result<(), i32> {
    let mut tbl = SIGNALFD_TABLE.lock();
    match tbl.slot_mut(fd) {
        Some(sfd) if sfd.owner_pid == pid => {
            let fd_to_free = sfd.fd;
            drop(tbl.slot_mut(fd)); // release borrow
            SIGNALFD_TABLE.lock().free(fd_to_free);
            Ok(())
        }
        Some(_) => Err(EBADF), // wrong owner
        None => Err(EBADF),
    }
}

/// Intercept a signal for `pid` into any matching signalfds.
///
/// Called from `deliver_signals_full()` **before** the normal handler path.
/// Returns `true` if the signal was captured by at least one signalfd;
/// the caller should then skip the default handler dispatch for this signal.
pub fn deliver_to_signalfds(pid: u32, sig: u8) -> bool {
    SIGNALFD_TABLE.lock().deliver(pid, sig)
}

/// Close all signalfds owned by `pid`. Called from process exit.
pub fn close_all(pid: u32) {
    let mut tbl = SIGNALFD_TABLE.lock();
    for slot in tbl.slots.iter_mut() {
        if let Some(sfd) = slot.as_mut() {
            if sfd.owner_pid == pid {
                sfd.active = false;
            }
        }
    }
    // Remove deactivated entries.
    for slot in tbl.slots.iter_mut() {
        if let Some(sfd) = slot.as_ref() {
            if !sfd.active {
                *slot = None;
            }
        }
    }
}

/// Update the signal mask for an existing signalfd (Linux SFD_REPLACE).
///
/// Replaces `sigset` on an open signalfd owned by `pid`.  SIGKILL and
/// SIGSTOP bits are silently cleared.
pub fn signalfd_update_mask(fd: u32, pid: u32, mut sigset: u64) -> Result<(), i32> {
    sigset &= !(1u64 << 9) & !(1u64 << 19);
    let mut tbl = SIGNALFD_TABLE.lock();
    let sfd = tbl.slot_mut(fd).ok_or(EBADF)?;
    if sfd.owner_pid != pid {
        return Err(EBADF);
    }
    sfd.sigset = sigset;
    Ok(())
}

/// Initialise the signalfd subsystem.
///
/// Called once from `process::init()`.
pub fn init() {
    crate::serial_println!(
        "    [signalfd] signal file descriptor subsystem ready (max {} fds)",
        MAX_SIGNALFDS
    );
}
