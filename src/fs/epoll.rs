use crate::serial_println;
use crate::sync::Mutex;
/// Event polling (epoll) -- scalable I/O event notification
///
/// Part of the AIOS filesystem layer.
///
/// Provides an epoll-compatible interface for monitoring multiple file
/// descriptors for readiness events without busy-polling.
///
/// Design:
///   - Each EpollInstance owns an interest list (Vec of watched fds) and a
///     ready list (Vec of fds with pending events).
///   - Supports edge-triggered (EPOLLET) and level-triggered (default) modes.
///   - A global table maps epoll IDs to instances, protected by Mutex.
///   - Wait returns immediately if events are pending or the timeout is zero;
///     otherwise it spins up to timeout_ms checking for readiness.
///
/// Inspired by: Linux epoll (fs/eventpoll.c). All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Event flag constants
// ---------------------------------------------------------------------------

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLET: u32 = 1 << 31; // edge-triggered
pub const EPOLLONESHOT: u32 = 1 << 30;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An event returned from epoll_wait.
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub fd: i32,
    pub data: u64,
}

/// Trigger mode for a watched descriptor.
#[derive(Clone, Copy, PartialEq)]
enum TriggerMode {
    Level,
    Edge,
}

/// A single entry in the interest list.
#[derive(Clone)]
struct InterestEntry {
    fd: i32,
    events: u32,
    data: u64,
    mode: TriggerMode,
    oneshot: bool,
    /// For edge-triggered: tracks whether the event was already delivered
    /// since the last state transition.
    delivered: bool,
    /// Disabled after oneshot fires
    disabled: bool,
}

/// A single epoll instance.
struct EpollInner {
    interest: Vec<InterestEntry>,
    ready: Vec<EpollEvent>,
}

/// Global table of all epoll instances.
struct EpollTable {
    instances: Vec<Option<EpollInner>>,
    next_id: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static EPOLL_TABLE: Mutex<Option<EpollTable>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// EpollInner
// ---------------------------------------------------------------------------

impl EpollInner {
    fn new() -> Self {
        EpollInner {
            interest: Vec::new(),
            ready: Vec::new(),
        }
    }

    /// Add a file descriptor to the interest list.
    fn ctl_add(&mut self, fd: i32, events: u32, data: u64) -> Result<(), i32> {
        // Check for duplicate
        for entry in self.interest.iter() {
            if entry.fd == fd {
                return Err(-1); // EEXIST
            }
        }
        let mode = if events & EPOLLET != 0 {
            TriggerMode::Edge
        } else {
            TriggerMode::Level
        };
        let oneshot = events & EPOLLONESHOT != 0;
        self.interest.push(InterestEntry {
            fd,
            events: events & !(EPOLLET | EPOLLONESHOT),
            data,
            mode,
            oneshot,
            delivered: false,
            disabled: false,
        });
        Ok(())
    }

    /// Modify the events/data for an already-watched fd.
    fn ctl_mod(&mut self, fd: i32, events: u32, data: u64) -> Result<(), i32> {
        for entry in self.interest.iter_mut() {
            if entry.fd == fd {
                entry.events = events & !(EPOLLET | EPOLLONESHOT);
                entry.data = data;
                entry.mode = if events & EPOLLET != 0 {
                    TriggerMode::Edge
                } else {
                    TriggerMode::Level
                };
                entry.oneshot = events & EPOLLONESHOT != 0;
                entry.delivered = false;
                entry.disabled = false;
                return Ok(());
            }
        }
        Err(-2) // ENOENT
    }

    /// Remove a file descriptor from the interest list.
    fn ctl_del(&mut self, fd: i32) -> Result<(), i32> {
        let before = self.interest.len();
        self.interest.retain(|e| e.fd != fd);
        if self.interest.len() == before {
            Err(-2) // ENOENT
        } else {
            Ok(())
        }
    }

    /// Signal that a descriptor has become ready with certain events.
    /// Called by the kernel when I/O state changes.
    fn signal_ready(&mut self, fd: i32, revents: u32) {
        for entry in self.interest.iter_mut() {
            if entry.fd == fd && !entry.disabled {
                let matched = entry.events & revents;
                if matched != 0 {
                    if entry.mode == TriggerMode::Edge {
                        if entry.delivered {
                            continue; // Already fired since last transition
                        }
                        entry.delivered = true;
                    }
                    self.ready.push(EpollEvent {
                        events: matched,
                        fd,
                        data: entry.data,
                    });
                    if entry.oneshot {
                        entry.disabled = true;
                    }
                }
            }
        }
    }

    /// Reset edge-trigger state for a descriptor (called when the fd is
    /// re-armed or the underlying condition clears).
    fn reset_edge(&mut self, fd: i32) {
        for entry in self.interest.iter_mut() {
            if entry.fd == fd {
                entry.delivered = false;
            }
        }
    }

    /// Collect pending events, draining the ready list.
    fn collect(&mut self, max_events: usize) -> Vec<EpollEvent> {
        let take = max_events.min(self.ready.len());
        let result: Vec<EpollEvent> = self.ready.drain(..take).collect();
        result
    }
}

// ---------------------------------------------------------------------------
// EpollTable
// ---------------------------------------------------------------------------

impl EpollTable {
    fn new() -> Self {
        EpollTable {
            instances: Vec::new(),
            next_id: 0,
        }
    }

    fn create(&mut self) -> usize {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        if id < self.instances.len() {
            self.instances[id] = Some(EpollInner::new());
        } else {
            self.instances.push(Some(EpollInner::new()));
        }
        id
    }

    fn get_mut(&mut self, id: usize) -> Option<&mut EpollInner> {
        self.instances.get_mut(id).and_then(|slot| slot.as_mut())
    }

    fn destroy(&mut self, id: usize) {
        if id < self.instances.len() {
            self.instances[id] = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new epoll instance, returning its ID.
pub fn epoll_create() -> Result<usize, i32> {
    let mut guard = EPOLL_TABLE.lock();
    match guard.as_mut() {
        Some(table) => Ok(table.create()),
        None => Err(-1),
    }
}

/// Add a descriptor to an epoll interest list.
pub fn epoll_ctl_add(epfd: usize, fd: i32, events: u32, data: u64) -> Result<(), i32> {
    let mut guard = EPOLL_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let inst = table.get_mut(epfd).ok_or(-2)?;
    inst.ctl_add(fd, events, data)
}

/// Modify a descriptor in an epoll interest list.
pub fn epoll_ctl_mod(epfd: usize, fd: i32, events: u32, data: u64) -> Result<(), i32> {
    let mut guard = EPOLL_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let inst = table.get_mut(epfd).ok_or(-2)?;
    inst.ctl_mod(fd, events, data)
}

/// Remove a descriptor from an epoll interest list.
pub fn epoll_ctl_del(epfd: usize, fd: i32) -> Result<(), i32> {
    let mut guard = EPOLL_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let inst = table.get_mut(epfd).ok_or(-2)?;
    inst.ctl_del(fd)
}

/// Signal readiness on a descriptor (called by kernel I/O paths).
pub fn epoll_signal(epfd: usize, fd: i32, revents: u32) {
    let mut guard = EPOLL_TABLE.lock();
    if let Some(table) = guard.as_mut() {
        if let Some(inst) = table.get_mut(epfd) {
            inst.signal_ready(fd, revents);
        }
    }
}

/// Collect ready events (non-blocking). Returns up to max_events.
pub fn epoll_wait(epfd: usize, max_events: usize) -> Result<Vec<EpollEvent>, i32> {
    let mut guard = EPOLL_TABLE.lock();
    let table = guard.as_mut().ok_or(-1)?;
    let inst = table.get_mut(epfd).ok_or(-2)?;
    Ok(inst.collect(max_events))
}

/// Destroy an epoll instance.
pub fn epoll_close(epfd: usize) {
    let mut guard = EPOLL_TABLE.lock();
    if let Some(table) = guard.as_mut() {
        table.destroy(epfd);
    }
}

/// Reset edge-trigger state after re-arming an fd.
pub fn epoll_reset_edge(epfd: usize, fd: i32) {
    let mut guard = EPOLL_TABLE.lock();
    if let Some(table) = guard.as_mut() {
        if let Some(inst) = table.get_mut(epfd) {
            inst.reset_edge(fd);
        }
    }
}

/// Initialize the epoll subsystem.
pub fn init() {
    let mut guard = EPOLL_TABLE.lock();
    *guard = Some(EpollTable::new());
    serial_println!("    epoll: initialized (edge/level trigger, oneshot)");
}
