/// POSIX signal handling compatibility
///
/// Part of the AIOS compatibility layer.
///
/// Maps POSIX signal numbers and sigaction semantics to the AIOS native
/// signal infrastructure. Provides signal masks, signal sets, and the
/// sigaction structure for Linux binary compatibility.
///
/// Design:
///   - Standard POSIX signals (SIGHUP..SIGRTMAX) are mapped to AIOS
///     native signal numbers.
///   - Per-process signal dispositions (default, ignore, handler) are
///     tracked in a table.
///   - Signal masks (sigset_t) use a 64-bit bitmask for signals 1..64.
///   - sigaction() installs handlers with SA_* flags.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: POSIX signal.h, Linux kernel signal handling. All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Signal numbers
// ---------------------------------------------------------------------------

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const SIGURG: u32 = 23;
pub const SIGXCPU: u32 = 24;
pub const SIGXFSZ: u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF: u32 = 27;
pub const SIGWINCH: u32 = 28;
pub const SIGIO: u32 = 29;
pub const SIGPWR: u32 = 30;
pub const SIGSYS: u32 = 31;
pub const SIGRTMIN: u32 = 32;
pub const SIGRTMAX: u32 = 64;
pub const MAX_SIGNALS: usize = 65;

// SA flags
pub const SA_NOCLDSTOP: u32 = 0x0001;
pub const SA_NOCLDWAIT: u32 = 0x0002;
pub const SA_SIGINFO: u32 = 0x0004;
pub const SA_RESTART: u32 = 0x10000000;
pub const SA_NODEFER: u32 = 0x40000000;
pub const SA_RESETHAND: u32 = 0x80000000;

// ---------------------------------------------------------------------------
// Signal set (bitmask)
// ---------------------------------------------------------------------------

/// A set of signals represented as a 64-bit bitmask.
#[derive(Clone, Copy)]
pub struct SigSet {
    bits: u64,
}

impl SigSet {
    pub fn empty() -> Self {
        SigSet { bits: 0 }
    }

    pub fn full() -> Self {
        SigSet { bits: u64::MAX }
    }

    pub fn add(&mut self, sig: u32) {
        if sig >= 1 && sig <= 64 {
            self.bits |= 1u64 << (sig - 1);
        }
    }

    pub fn remove(&mut self, sig: u32) {
        if sig >= 1 && sig <= 64 {
            self.bits &= !(1u64 << (sig - 1));
        }
    }

    pub fn contains(&self, sig: u32) -> bool {
        if sig >= 1 && sig <= 64 {
            (self.bits >> (sig - 1)) & 1 != 0
        } else {
            false
        }
    }

    pub fn union(&self, other: &SigSet) -> SigSet {
        SigSet {
            bits: self.bits | other.bits,
        }
    }

    pub fn intersect(&self, other: &SigSet) -> SigSet {
        SigSet {
            bits: self.bits & other.bits,
        }
    }

    pub fn complement(&self) -> SigSet {
        SigSet { bits: !self.bits }
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }
}

// ---------------------------------------------------------------------------
// Signal disposition
// ---------------------------------------------------------------------------

/// What to do when a signal is delivered.
#[derive(Clone, Copy, PartialEq)]
pub enum SigDisposition {
    Default,
    Ignore,
    /// Handler address (in userspace).
    Handler(usize),
}

/// sigaction-style signal configuration.
#[derive(Clone, Copy)]
pub struct SigAction {
    pub disposition: SigDisposition,
    pub mask: SigSet,
    pub flags: u32,
}

impl SigAction {
    pub fn default() -> Self {
        SigAction {
            disposition: SigDisposition::Default,
            mask: SigSet::empty(),
            flags: 0,
        }
    }

    pub fn ignore() -> Self {
        SigAction {
            disposition: SigDisposition::Ignore,
            mask: SigSet::empty(),
            flags: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Default actions
// ---------------------------------------------------------------------------

/// Default action for a signal.
#[derive(Clone, Copy, PartialEq)]
pub enum DefaultAction {
    Terminate,
    CoreDump,
    Stop,
    Continue,
    Ignore,
}

pub fn default_action(sig: u32) -> DefaultAction {
    match sig {
        SIGHUP | SIGINT | SIGPIPE | SIGALRM | SIGTERM | SIGUSR1 | SIGUSR2 | SIGPROF
        | SIGVTALRM | SIGSTKFLT | SIGIO | SIGPWR => DefaultAction::Terminate,
        SIGQUIT | SIGILL | SIGABRT | SIGFPE | SIGSEGV | SIGBUS | SIGSYS | SIGTRAP
        | SIGXCPU | SIGXFSZ => DefaultAction::CoreDump,
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => DefaultAction::Stop,
        SIGCONT => DefaultAction::Continue,
        SIGCHLD | SIGURG | SIGWINCH => DefaultAction::Ignore,
        _ if sig >= SIGRTMIN && sig <= SIGRTMAX => DefaultAction::Terminate,
        _ => DefaultAction::Terminate,
    }
}

// ---------------------------------------------------------------------------
// Per-process signal state
// ---------------------------------------------------------------------------

struct ProcessSignals {
    pid: u32,
    actions: Vec<SigAction>,    // indexed by signal number
    pending: SigSet,
    blocked: SigSet,
}

impl ProcessSignals {
    fn new(pid: u32) -> Self {
        let mut actions = Vec::with_capacity(MAX_SIGNALS);
        for _ in 0..MAX_SIGNALS {
            actions.push(SigAction::default());
        }
        // SIGCHLD, SIGURG, SIGWINCH default to ignore
        actions[SIGCHLD as usize] = SigAction::ignore();
        actions[SIGURG as usize] = SigAction::ignore();
        actions[SIGWINCH as usize] = SigAction::ignore();

        ProcessSignals {
            pid,
            actions,
            pending: SigSet::empty(),
            blocked: SigSet::empty(),
        }
    }
}

/// Inner state.
struct Inner {
    processes: Vec<ProcessSignals>,
}

// ---------------------------------------------------------------------------
// Inner implementation
// ---------------------------------------------------------------------------

impl Inner {
    fn new() -> Self {
        Inner {
            processes: Vec::new(),
        }
    }

    fn get_or_create(&mut self, pid: u32) -> &mut ProcessSignals {
        let pos = self.processes.iter().position(|p| p.pid == pid);
        match pos {
            Some(idx) => &mut self.processes[idx],
            None => {
                self.processes.push(ProcessSignals::new(pid));
                let last = self.processes.len() - 1;
                &mut self.processes[last]
            }
        }
    }

    fn sigaction(&mut self, pid: u32, sig: u32, act: &SigAction) -> Result<SigAction, i32> {
        // SIGKILL and SIGSTOP cannot be caught or ignored
        if sig == SIGKILL || sig == SIGSTOP {
            return Err(-22); // EINVAL
        }
        if sig == 0 || sig as usize >= MAX_SIGNALS {
            return Err(-22);
        }
        let ps = self.get_or_create(pid);
        let old = ps.actions[sig as usize];
        ps.actions[sig as usize] = *act;
        Ok(old)
    }

    fn send_signal(&mut self, pid: u32, sig: u32) -> Result<(), i32> {
        if sig == 0 || sig as usize >= MAX_SIGNALS {
            return Err(-22);
        }
        let ps = self.get_or_create(pid);
        ps.pending.add(sig);
        Ok(())
    }

    fn dequeue_signal(&mut self, pid: u32) -> Option<u32> {
        let ps = self.get_or_create(pid);
        let deliverable = ps.pending.bits & !ps.blocked.bits;
        if deliverable == 0 {
            return None;
        }
        // Find lowest set bit
        let sig = deliverable.trailing_zeros() + 1;
        ps.pending.remove(sig);

        // Check SA_RESETHAND
        if sig < MAX_SIGNALS as u32 {
            if ps.actions[sig as usize].flags & SA_RESETHAND != 0 {
                ps.actions[sig as usize] = SigAction::default();
            }
        }
        Some(sig)
    }

    fn sigmask(&mut self, pid: u32, how: u32, set: &SigSet) -> SigSet {
        let ps = self.get_or_create(pid);
        let old = ps.blocked;
        match how {
            0 => ps.blocked = ps.blocked.union(set),     // SIG_BLOCK
            1 => ps.blocked.bits &= !set.bits,           // SIG_UNBLOCK
            2 => ps.blocked = *set,                       // SIG_SETMASK
            _ => {}
        }
        // SIGKILL and SIGSTOP cannot be blocked
        ps.blocked.remove(SIGKILL);
        ps.blocked.remove(SIGSTOP);
        old
    }

    fn remove_process(&mut self, pid: u32) {
        self.processes.retain(|p| p.pid != pid);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static SIGNALS: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install a signal action (sigaction).
pub fn sigaction(pid: u32, sig: u32, act: &SigAction) -> Result<SigAction, i32> {
    let mut guard = SIGNALS.lock();
    guard.as_mut().ok_or(-1).and_then(|inner| inner.sigaction(pid, sig, act))
}

/// Send a signal to a process.
pub fn kill(pid: u32, sig: u32) -> Result<(), i32> {
    let mut guard = SIGNALS.lock();
    guard.as_mut().ok_or(-1).and_then(|inner| inner.send_signal(pid, sig))
}

/// Dequeue the next deliverable signal.
pub fn dequeue(pid: u32) -> Option<u32> {
    let mut guard = SIGNALS.lock();
    guard.as_mut().and_then(|inner| inner.dequeue_signal(pid))
}

/// Modify the signal mask (sigprocmask). how: 0=BLOCK, 1=UNBLOCK, 2=SETMASK.
pub fn sigmask(pid: u32, how: u32, set: &SigSet) -> SigSet {
    let mut guard = SIGNALS.lock();
    guard
        .as_mut()
        .map_or(SigSet::empty(), |inner| inner.sigmask(pid, how, set))
}

/// Clean up signal state for a terminated process.
pub fn cleanup(pid: u32) {
    let mut guard = SIGNALS.lock();
    if let Some(inner) = guard.as_mut() {
        inner.remove_process(pid);
    }
}

/// Initialize the signal compatibility subsystem.
pub fn init() {
    let mut guard = SIGNALS.lock();
    *guard = Some(Inner::new());
    serial_println!("    signal_compat: initialized (POSIX signals 1-64, sigaction, masks)");
}
