use crate::sync::Mutex;
/// Signals — asynchronous process notifications
///
/// POSIX-inspired signal delivery with custom Hoags extensions.
/// Signals can be caught, ignored, or left at default behavior.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

static SIGNAL_STATE: Mutex<Option<SignalManager>> = Mutex::new(None);

/// Signal numbers
pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGKILL: u32 = 9; // cannot be caught or ignored
pub const SIGSEGV: u32 = 11;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19; // cannot be caught or ignored
pub const SIGWINCH: u32 = 28;
// Hoags extensions
pub const SIGHEALTH: u32 = 33; // health check request
pub const SIGRELOAD: u32 = 34; // config reload request
pub const SIGUPDATE: u32 = 35; // OTA update notification

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    Default,
    Ignore,
    Catch, // handler is registered in userspace
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    Terminate,
    CoreDump,
    Stop,
    Continue,
    Ignore,
}

pub fn default_action(sig: u32) -> DefaultAction {
    match sig {
        SIGHUP | SIGINT | SIGQUIT | SIGPIPE | SIGALRM | SIGTERM => DefaultAction::Terminate,
        SIGKILL => DefaultAction::Terminate,
        SIGSEGV => DefaultAction::CoreDump,
        SIGSTOP => DefaultAction::Stop,
        SIGCONT => DefaultAction::Continue,
        SIGCHLD | SIGWINCH => DefaultAction::Ignore,
        SIGHEALTH | SIGRELOAD | SIGUPDATE => DefaultAction::Ignore,
        _ => DefaultAction::Terminate,
    }
}

#[derive(Debug, Clone)]
pub struct PendingSignal {
    pub signal: u32,
    pub sender: u32,
}

struct ProcessSignalState {
    actions: BTreeMap<u32, SignalAction>,
    pending: [bool; 64],
    blocked: u64, // bitmask
}

impl ProcessSignalState {
    fn new() -> Self {
        ProcessSignalState {
            actions: BTreeMap::new(),
            pending: [false; 64],
            blocked: 0,
        }
    }
}

pub struct SignalManager {
    states: BTreeMap<u32, ProcessSignalState>,
}

impl SignalManager {
    fn new() -> Self {
        SignalManager {
            states: BTreeMap::new(),
        }
    }

    pub fn register_process(&mut self, pid: u32) {
        self.states.insert(pid, ProcessSignalState::new());
    }

    pub fn set_action(
        &mut self,
        pid: u32,
        signal: u32,
        action: SignalAction,
    ) -> Result<(), &'static str> {
        if signal == SIGKILL || signal == SIGSTOP {
            return Err("cannot change SIGKILL or SIGSTOP");
        }
        let state = self.states.get_mut(&pid).ok_or("process not found")?;
        state.actions.insert(signal, action);
        Ok(())
    }

    pub fn send(&mut self, target: u32, signal: u32, sender: u32) -> Result<(), &'static str> {
        let state = self.states.get_mut(&target).ok_or("process not found")?;
        if signal < 64 {
            state.pending[signal as usize] = true;
        }
        serial_println!("    [signal] {} -> PID {} (sig {})", sender, target, signal);
        Ok(())
    }

    pub fn get_pending(&mut self, pid: u32) -> Option<u32> {
        let state = self.states.get_mut(&pid)?;
        for sig in 1..64u32 {
            if state.pending[sig as usize] && (state.blocked & (1 << sig)) == 0 {
                state.pending[sig as usize] = false;
                return Some(sig);
            }
        }
        None
    }

    pub fn block(&mut self, pid: u32, mask: u64) {
        if let Some(state) = self.states.get_mut(&pid) {
            state.blocked |= mask;
            // Never block SIGKILL or SIGSTOP
            state.blocked &= !((1 << SIGKILL) | (1 << SIGSTOP));
        }
    }
}

pub fn init() {
    let mut mgr = SignalManager::new();
    mgr.register_process(0);
    mgr.register_process(1);
    *SIGNAL_STATE.lock() = Some(mgr);
    serial_println!("    [signal] Signal delivery initialized");
}

pub fn send(target: u32, signal: u32, sender: u32) -> Result<(), &'static str> {
    SIGNAL_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .send(target, signal, sender)
}

/// Register a new process with the signal manager.
/// Must be called when a process is created before any signals can be delivered.
pub fn register_process(pid: u32) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        mgr.register_process(pid);
    }
}

/// Deregister a process when it exits, cleaning up pending signal state.
pub fn deregister_process(pid: u32) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        mgr.states.remove(&pid);
    }
}

/// Set the signal action (Default, Ignore, Catch) for a process.
/// Returns Err if attempting to change SIGKILL or SIGSTOP.
pub fn set_action(pid: u32, signal: u32, action: SignalAction) -> Result<(), &'static str> {
    SIGNAL_STATE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .set_action(pid, signal, action)
}

/// Get the next unblocked pending signal for a process, clearing it.
/// Returns None if no signals are pending.
pub fn get_pending(pid: u32) -> Option<u32> {
    SIGNAL_STATE.lock().as_mut()?.get_pending(pid)
}

/// Block signals in the given bitmask for a process.
/// SIGKILL (bit 9) and SIGSTOP (bit 19) cannot be blocked.
pub fn block(pid: u32, mask: u64) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        mgr.block(pid, mask);
    }
}

/// Unblock signals in the given bitmask for a process.
pub fn unblock(pid: u32, mask: u64) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        if let Some(state) = mgr.states.get_mut(&pid) {
            state.blocked &= !mask;
            // SIGKILL and SIGSTOP must never be blocked
            state.blocked &= !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
        }
    }
}

/// Replace the blocked signal mask entirely for a process.
/// SIGKILL and SIGSTOP bits are silently ignored.
pub fn set_mask(pid: u32, mask: u64) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        if let Some(state) = mgr.states.get_mut(&pid) {
            state.blocked = mask;
            state.blocked &= !((1u64 << SIGKILL) | (1u64 << SIGSTOP));
        }
    }
}

/// Get the current blocked signal mask for a process.
pub fn get_mask(pid: u32) -> u64 {
    SIGNAL_STATE
        .lock()
        .as_ref()
        .and_then(|mgr| mgr.states.get(&pid))
        .map(|s| s.blocked)
        .unwrap_or(0)
}

/// Check whether a process has any unblocked pending signals without consuming them.
pub fn has_pending(pid: u32) -> bool {
    let guard = SIGNAL_STATE.lock();
    if let Some(ref mgr) = *guard {
        if let Some(state) = mgr.states.get(&pid) {
            for sig in 1..64u32 {
                if state.pending[sig as usize] && (state.blocked & (1 << sig)) == 0 {
                    return true;
                }
            }
        }
    }
    false
}

/// Kill a process group: send `signal` to every process registered with the manager.
/// Used for Ctrl-C (SIGINT to foreground process group), etc.
pub fn kill_all(signal: u32, sender: u32) {
    if let Some(ref mut mgr) = *SIGNAL_STATE.lock() {
        let pids: Vec<u32> = mgr.states.keys().copied().collect();
        for pid in pids {
            if signal < 64 {
                if let Some(state) = mgr.states.get_mut(&pid) {
                    state.pending[signal as usize] = true;
                }
            }
            serial_println!(
                "    [signal] broadcast {} -> PID {} (sig {})",
                sender,
                pid,
                signal
            );
        }
    }
}
