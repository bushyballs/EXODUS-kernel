/// Extended `sigaction` support — per-process signal action tables.
///
/// Provides a static-storage `SigTable` per tracked process (up to
/// `MAX_SIG_TABLES` simultaneously), holding a full `SigAction` for each of
/// the 64 signal numbers (standard 1-31 and RT 34-64).
///
/// The `sigprocmask` / `sigpending` functions operate on a 64-bit mask,
/// giving coverage for both the standard signal range and the lower half of
/// the RT signal range without heap allocation.
///
/// # SA_* flag constants
///
/// | Constant         | Value        | Meaning                                     |
/// |------------------|--------------|---------------------------------------------|
/// | `SA_NOCLDSTOP`   | `0x0000_0001`| Do not send SIGCHLD for stopped children    |
/// | `SA_NOCLDWAIT`   | `0x0000_0002`| Do not create zombies on child exit         |
/// | `SA_SIGINFO`     | `0x0000_0004`| Call `sa_sigaction(sig, info, ctx)`         |
/// | `SA_ONSTACK`     | `0x0800_0000`| Use alternate signal stack                  |
/// | `SA_RESTART`     | `0x1000_0000`| Restart syscall after handler returns       |
/// | `SA_NODEFER`     | `0x4000_0000`| Do not mask signal during its own handler   |
/// | `SA_RESETHAND`   | `0x8000_0000`| Reset disposition to SIG_DFL after delivery |
/// | `SA_RESTORER`    | `0x0400_0000`| `sa_restorer` field is valid                |
///
/// RULES (no violations or the kernel panics):
///   - No heap (`Vec`, `Box`, `String`, `alloc::*`)
///   - No float casts (`as f32`, `as f64`)
///   - No `unwrap()`, `expect()`, `panic!()`
///   - All counters: `saturating_add` / `saturating_sub`
///   - All sequence numbers: `wrapping_add`
///   - Every struct inside a `static Mutex` must be `Copy` with `const fn empty()`
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// SA_* flags
// ---------------------------------------------------------------------------

/// Do not send SIGCHLD when a child is stopped (but not terminated).
pub const SA_NOCLDSTOP: u32 = 0x0000_0001;
/// Do not create zombies when children exit.
pub const SA_NOCLDWAIT: u32 = 0x0000_0002;
/// Invoke the handler as `sigaction(sig, info, ctx)` (`SA_SIGINFO` protocol).
pub const SA_SIGINFO: u32 = 0x0000_0004;
/// Use the alternate signal stack established by `sigaltstack`.
pub const SA_ONSTACK: u32 = 0x0800_0000;
/// Restart interrupted system calls automatically after the handler returns.
pub const SA_RESTART: u32 = 0x1000_0000;
/// Do not block the signal being delivered while the handler is running.
pub const SA_NODEFER: u32 = 0x4000_0000;
/// Reset the signal disposition to `SIG_DFL` after the first delivery.
pub const SA_RESETHAND: u32 = 0x8000_0000;
/// The `sa_restorer` field is valid (required by Linux ABI on x86_64).
pub const SA_RESTORER: u32 = 0x0400_0000;

// ---------------------------------------------------------------------------
// SigAction — per-signal action descriptor
// ---------------------------------------------------------------------------

/// Describes how a signal should be handled.
///
/// Mirrors the kernel-ABI `sigaction` struct used by `rt_sigaction(2)`.
/// All fields are plain integers so the struct is `Copy`.
#[repr(C)]
#[derive(Copy, Clone)]
pub struct SigAction {
    /// Handler / disposition:
    ///   - `0` = SIG_DFL (default action)
    ///   - `1` = SIG_IGN (ignore)
    ///   - otherwise: user-space function pointer
    pub sa_handler: u64,
    /// Flags — combination of `SA_*` constants above.
    pub sa_flags: u32,
    /// Additional signals to block while the handler is executing
    /// (bitmask, same encoding as `sigprocmask`).
    pub sa_mask: u64,
    /// Restorer trampoline address (used when `SA_RESTORER` is set).
    pub sa_restorer: u64,
}

impl SigAction {
    /// Construct a default-action `SigAction` (handler = 0 = SIG_DFL).
    pub const fn default() -> Self {
        SigAction {
            sa_handler: 0,
            sa_flags: 0,
            sa_mask: 0,
            sa_restorer: 0,
        }
    }

    /// Return `true` if this action represents `SIG_DFL`.
    #[inline]
    pub fn is_default(&self) -> bool {
        self.sa_handler == 0
    }

    /// Return `true` if this action represents `SIG_IGN`.
    #[inline]
    pub fn is_ignore(&self) -> bool {
        self.sa_handler == 1
    }

    /// Return `true` if `SA_RESETHAND` is set.
    #[inline]
    pub fn reset_hand(&self) -> bool {
        self.sa_flags & SA_RESETHAND != 0
    }

    /// Return `true` if `SA_SIGINFO` is set.
    #[inline]
    pub fn use_siginfo(&self) -> bool {
        self.sa_flags & SA_SIGINFO != 0
    }

    /// Return `true` if `SA_RESTART` is set.
    #[inline]
    pub fn restart(&self) -> bool {
        self.sa_flags & SA_RESTART != 0
    }

    /// Return `true` if `SA_NODEFER` is set.
    #[inline]
    pub fn nodefer(&self) -> bool {
        self.sa_flags & SA_NODEFER != 0
    }
}

// ---------------------------------------------------------------------------
// SigTable — per-process signal action table
// ---------------------------------------------------------------------------

/// Number of signal slots per table.  Covers signals 0-63.
const SIG_TABLE_ENTRIES: usize = 64;

/// Maximum number of processes whose `SigTable` this module tracks.
pub const MAX_SIG_TABLES: usize = 64;

/// Per-process signal action table.
///
/// `actions[n]` holds the `SigAction` for signal number `n`.
/// Index 0 is unused (signals are numbered from 1).
#[derive(Copy, Clone)]
pub struct SigTable {
    /// Owning process PID (0 = slot unused).
    pub pid: u32,
    /// Slot is in use.
    pub active: bool,
    /// Per-signal actions (indices 0..63).
    pub actions: [SigAction; SIG_TABLE_ENTRIES],
    /// Current signal mask — bit `n` set means signal `n` is blocked.
    pub mask: u64,
    /// Set of pending signals waiting to be delivered (bitmask).
    pub pending: u64,
}

impl SigTable {
    /// Construct an empty `SigTable` with all signals at `SIG_DFL` and no
    /// blocked / pending signals.
    pub const fn empty() -> Self {
        SigTable {
            pid: 0,
            active: false,
            actions: [const { SigAction::default() }; SIG_TABLE_ENTRIES],
            mask: 0,
            pending: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global table
// ---------------------------------------------------------------------------

static SIG_TABLES: Mutex<[SigTable; MAX_SIG_TABLES]> =
    Mutex::new([const { SigTable::empty() }; MAX_SIG_TABLES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the slot index for `pid`.
fn find_slot(table: &[SigTable; MAX_SIG_TABLES], pid: u32) -> Option<usize> {
    table.iter().position(|t| t.active && t.pid == pid)
}

/// Find or allocate a slot for `pid`.
fn find_or_alloc(table: &mut [SigTable; MAX_SIG_TABLES], pid: u32) -> Option<usize> {
    if let Some(i) = table.iter().position(|t| t.active && t.pid == pid) {
        return Some(i);
    }
    if let Some(i) = table.iter().position(|t| !t.active) {
        table[i] = SigTable::empty();
        table[i].pid = pid;
        table[i].active = true;
        return Some(i);
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the `SigAction` registered for `signo` in `pid`'s table.
///
/// Returns `None` if no table exists for `pid` or `signo >= 64`.
pub fn sigaction_get(pid: u32, signo: u32) -> Option<SigAction> {
    if signo == 0 || signo as usize >= SIG_TABLE_ENTRIES {
        return None;
    }
    let table = SIG_TABLES.lock();
    let slot = find_slot(&table, pid)?;
    Some(table[slot].actions[signo as usize])
}

/// Register `action` for `signo` in `pid`'s table.
///
/// Creates a new table for `pid` if one does not already exist.
///
/// # Returns
/// `true` on success; `false` if the table is full or `signo` is out of range.
pub fn sigaction_set(pid: u32, signo: u32, action: SigAction) -> bool {
    if signo == 0 || signo as usize >= SIG_TABLE_ENTRIES {
        return false;
    }
    // SIGKILL (9) and SIGSTOP (19) cannot be caught or ignored.
    if signo == 9 || signo == 19 {
        return false;
    }
    let mut table = SIG_TABLES.lock();
    match find_or_alloc(&mut table, pid) {
        None => false,
        Some(slot) => {
            table[slot].actions[signo as usize] = action;
            true
        }
    }
}

/// Get the current signal mask for `pid`.
///
/// Returns `0` if no table exists for `pid`.
pub fn sigprocmask_get(pid: u32) -> u64 {
    let table = SIG_TABLES.lock();
    match find_slot(&table, pid) {
        None => 0,
        Some(slot) => table[slot].mask,
    }
}

/// Set the signal mask for `pid`.
///
/// Creates a table entry for `pid` if needed (best-effort; silent on failure).
pub fn sigprocmask_set(pid: u32, mask: u64) {
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_or_alloc(&mut table, pid) {
        table[slot].mask = mask;
    }
}

/// Block additional signals for `pid` (`SIG_BLOCK` semantics).
pub fn sigprocmask_block(pid: u32, add: u64) {
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_or_alloc(&mut table, pid) {
        table[slot].mask |= add;
    }
}

/// Unblock signals for `pid` (`SIG_UNBLOCK` semantics).
pub fn sigprocmask_unblock(pid: u32, remove: u64) {
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_slot(&mut table, pid) {
        table[slot].mask &= !remove;
    }
}

/// Return the set of pending signals for `pid`.
///
/// "Pending" means the signal has been sent but not yet delivered (either
/// because it is blocked or because delivery has not been attempted yet).
///
/// Returns `0` if no table exists for `pid`.
pub fn sigpending(pid: u32) -> u64 {
    let table = SIG_TABLES.lock();
    match find_slot(&table, pid) {
        None => 0,
        Some(slot) => table[slot].pending,
    }
}

/// Mark signal `signo` as pending for `pid`.
///
/// Called by the signal-sending path to record that a signal is waiting.
pub fn sigpending_mark(pid: u32, signo: u32) {
    if signo == 0 || signo >= 64 {
        return;
    }
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_or_alloc(&mut table, pid) {
        table[slot].pending |= 1u64 << signo;
    }
}

/// Clear the pending bit for `signo` once the signal has been delivered.
pub fn sigpending_clear(pid: u32, signo: u32) {
    if signo == 0 || signo >= 64 {
        return;
    }
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_slot(&mut table, pid) {
        table[slot].pending &= !(1u64 << signo);
    }
}

/// Release the `SigTable` slot for `pid` (called on process exit).
pub fn sigtable_free(pid: u32) {
    let mut table = SIG_TABLES.lock();
    if let Some(slot) = find_slot(&table, pid) {
        table[slot] = SigTable::empty();
    }
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialise the sigaction subsystem.
pub fn init() {
    crate::serial_println!(
        "  sigaction: extended signal table ready ({} process slots)",
        MAX_SIG_TABLES
    );
}
