use crate::serial_println;
/// epoll — fd-based I/O event notification for the IPC layer
///
/// This module wraps `fs::epoll` with a process-fd-oriented interface that
/// the syscall layer (and userspace programs) actually see.
///
/// Design decisions:
///   - epoll instances are identified by a plain integer "epoll fd" returned
///     to userspace.  We encode these as (1000 + table_index) to avoid
///     colliding with real file descriptors (0..999).
///   - Readiness probing queries `ipc::pipe` for pipe fds and falls back to a
///     generic "always ready" heuristic for non-pipe fds, so that programs
///     doing simple readiness loops work correctly without a full VFS poll hook.
///   - All state is stored in a fixed-size static table (16 simultaneous epoll
///     instances) so that no heap allocation is required beyond what
///     `fs::epoll` already does internally.
///   - No float arithmetic, no `as f32/f64` casts, no panic paths.
///
/// Inspired by: Linux epoll(7). All code is original.
use crate::sync::Mutex;

// Re-export the event-flag constants and EpollEvent so callers only need to
// import from this module.
pub use crate::fs::epoll::{
    EPOLLERR, EPOLLET, EPOLLHUP, EPOLLIN, EPOLLONESHOT, EPOLLOUT, EPOLLRDHUP,
};

/// EPOLL_CTL operation codes
pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

/// Offset added to a table index to produce an "epoll fd" visible to userspace.
/// High enough to avoid clash with ordinary process fds (0-999).
const EPOLL_FD_BASE: i32 = 1000;

/// Maximum simultaneous epoll instances kernel-wide.
const MAX_EPOLL_FDS: usize = 16;

/// An event returned from `epoll_wait`.
#[derive(Clone, Copy)]
pub struct EpollEvent {
    /// Bitmask of ready events (EPOLLIN | EPOLLOUT | …)
    pub events: u32,
    /// User-supplied opaque value (typically the fd or a pointer)
    pub data: u64,
}

// ---------------------------------------------------------------------------
// Internal per-instance state
// ---------------------------------------------------------------------------

/// One entry in an epoll interest list.
#[derive(Clone, Copy)]
struct Watch {
    /// Watched file descriptor (-1 = slot unused)
    fd: i32,
    /// Requested event mask
    events: u32,
    /// User data echoed back on wake
    data: u64,
    /// Last events delivered (for edge-triggered tracking)
    last_rev: u32,
}

impl Watch {
    const fn empty() -> Self {
        Watch {
            fd: -1,
            events: 0,
            data: 0,
            last_rev: 0,
        }
    }
}

/// One epoll instance — up to 64 watched fds.
struct EpollFd {
    watches: [Watch; 64],
    watch_count: usize,
    /// Mirrors the `fs::epoll` table index so we can forward signals there.
    fs_id: usize,
}

impl EpollFd {
    const fn empty() -> Self {
        EpollFd {
            watches: [const { Watch::empty() }; 64],
            watch_count: 0,
            fs_id: 0,
        }
    }

    /// Find a watch slot by fd. Returns the index or None.
    fn find(&self, fd: i32) -> Option<usize> {
        for i in 0..self.watch_count {
            if self.watches[i].fd == fd {
                return Some(i);
            }
        }
        None
    }

    /// Add fd. Returns 0 on success, -17 (EEXIST) if already watched, -28 if full.
    fn add(&mut self, fd: i32, events: u32, data: u64) -> i32 {
        if self.find(fd).is_some() {
            return -17; // EEXIST
        }
        if self.watch_count >= 64 {
            return -28; // ENOSPC
        }
        let idx = self.watch_count;
        self.watches[idx] = Watch {
            fd,
            events,
            data,
            last_rev: 0,
        };
        self.watch_count = self.watch_count.saturating_add(1);
        0
    }

    /// Modify an existing watch. Returns 0 or -2 (ENOENT).
    fn modify(&mut self, fd: i32, events: u32, data: u64) -> i32 {
        match self.find(fd) {
            Some(i) => {
                self.watches[i].events = events;
                self.watches[i].data = data;
                self.watches[i].last_rev = 0;
                0
            }
            None => -2, // ENOENT
        }
    }

    /// Delete a watch. Returns 0 or -2 (ENOENT).
    fn delete(&mut self, fd: i32) -> i32 {
        match self.find(fd) {
            Some(i) => {
                let last = self.watch_count.saturating_sub(1);
                if i < last {
                    self.watches[i] = self.watches[last];
                }
                self.watches[last] = Watch::empty();
                self.watch_count = last;
                0
            }
            None => -2, // ENOENT
        }
    }
}

// ---------------------------------------------------------------------------
// Global table
// ---------------------------------------------------------------------------

struct EpollTable {
    slots: [Option<EpollFd>; MAX_EPOLL_FDS],
}

impl EpollTable {
    const fn new() -> Self {
        EpollTable {
            slots: [const { None }; MAX_EPOLL_FDS],
        }
    }

    fn alloc(&mut self) -> Option<usize> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(EpollFd::empty());
                return Some(i);
            }
        }
        None
    }

    fn get_mut(&mut self, idx: usize) -> Option<&mut EpollFd> {
        if idx < MAX_EPOLL_FDS {
            self.slots[idx].as_mut()
        } else {
            None
        }
    }

    fn free(&mut self, idx: usize) {
        if idx < MAX_EPOLL_FDS {
            self.slots[idx] = None;
        }
    }
}

static EPOLL_TABLE: Mutex<EpollTable> = Mutex::new(EpollTable::new());

// ---------------------------------------------------------------------------
// Fd-readiness helper
// ---------------------------------------------------------------------------

/// Check whether fd `fd` is readable right now.
///
/// Pipe read ends are queried via `ipc::pipe::available()`; everything else
/// is assumed ready (stdin always has potential input, sockets are stubbed
/// ready until a real poll hook exists).
fn fd_can_read(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    // Pipe read ends: fd encoded as pipe read fd 2000+idx*2
    if fd >= 2000 && fd % 2 == 0 {
        let pipe_idx = ((fd - 2000) / 2) as usize;
        return crate::ipc::pipe::can_read_by_idx(pipe_idx);
    }
    // For everything else (files, sockets, stdin) treat as readable.
    true
}

/// Check whether fd `fd` is writable right now.
fn fd_can_write(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    // Pipe write ends: fd encoded as 2001+idx*2
    if fd >= 2001 && fd % 2 == 1 {
        let pipe_idx = ((fd - 2001) / 2) as usize;
        return crate::ipc::pipe::can_write_by_idx(pipe_idx);
    }
    true
}

/// Probe a single watched fd against its requested events.
/// Returns the bitmask of events that are currently pending (may be 0).
fn probe_readiness(fd: i32, requested: u32) -> u32 {
    let mut ready: u32 = 0;
    if requested & EPOLLIN != 0 && fd_can_read(fd) {
        ready |= EPOLLIN;
    }
    if requested & EPOLLOUT != 0 && fd_can_write(fd) {
        ready |= EPOLLOUT;
    }
    ready
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new epoll instance.
///
/// Returns a non-negative "epoll fd" (1000 + index) on success, or a
/// negative error code on failure.
pub fn epoll_create() -> i32 {
    // Also create a backing entry in the fs::epoll layer so signal_ready()
    // can be called from VFS paths.
    let fs_id = match crate::fs::epoll::epoll_create() {
        Ok(id) => id,
        Err(_) => return -12, // ENOMEM
    };

    let mut table = EPOLL_TABLE.lock();
    match table.alloc() {
        Some(idx) => {
            if let Some(inst) = table.get_mut(idx) {
                inst.fs_id = fs_id;
            }
            EPOLL_FD_BASE.saturating_add(idx as i32)
        }
        None => {
            crate::fs::epoll::epoll_close(fs_id);
            -24 // EMFILE
        }
    }
}

/// Control an epoll instance (add / modify / delete a watched fd).
///
/// `epfd`  — the value returned by `epoll_create()`
/// `op`    — EPOLL_CTL_ADD / EPOLL_CTL_DEL / EPOLL_CTL_MOD
/// `fd`    — the file descriptor to watch
/// `event` — the requested events and user data (ignored for CTL_DEL)
///
/// Returns 0 on success or a negative errno.
pub fn epoll_ctl(epfd: i32, op: u32, fd: i32, event: &EpollEvent) -> i32 {
    let idx = (epfd - EPOLL_FD_BASE) as usize;
    if idx >= MAX_EPOLL_FDS {
        return -9; // EBADF
    }

    let mut table = EPOLL_TABLE.lock();
    let inst = match table.get_mut(idx) {
        Some(i) => i,
        None => return -9, // EBADF
    };

    match op {
        EPOLL_CTL_ADD => inst.add(fd, event.events, event.data),
        EPOLL_CTL_MOD => inst.modify(fd, event.events, event.data),
        EPOLL_CTL_DEL => inst.delete(fd),
        _ => -22, // EINVAL
    }
}

/// Wait for events on the epoll instance.
///
/// `epfd`       — epoll fd from `epoll_create()`
/// `events`     — caller-supplied slice to fill with ready events
/// `max_events` — maximum events to return (capped to `events.len()`)
/// `timeout_ms` — milliseconds to wait; -1 = block until any event
///
/// Returns the number of events filled, or a negative errno.
///
/// Implementation: spin-poll the interest list up to `timeout_ms` iterations
/// (each iteration is one pass through all watches).  This is a busy-wait stub
/// suitable for a bare-metal kernel without proper sleep/wake; real blocking
/// would require scheduler integration.
pub fn epoll_wait(epfd: i32, events: &mut [EpollEvent], max_events: usize, timeout_ms: i32) -> i32 {
    let idx = (epfd - EPOLL_FD_BASE) as usize;
    if idx >= MAX_EPOLL_FDS {
        return -9; // EBADF
    }

    let cap = max_events.min(events.len());
    if cap == 0 {
        return -22; // EINVAL
    }

    // How many iterations to spin. Each "iteration" is one full sweep of all
    // watched fds.  We map timeout_ms=-1 to a large but finite count so we
    // do not loop forever in a no_std context.
    let max_iters: u32 = if timeout_ms < 0 {
        1_000_000 // ~1M sweeps ≈ practical "block forever" for a spin loop
    } else if timeout_ms == 0 {
        1
    } else {
        (timeout_ms as u32).saturating_mul(100)
    };

    let mut found: usize = 0;

    'outer: for _iter in 0..max_iters {
        // Lock only long enough to snapshot watches; release before probing.
        let (watch_count, watches_snap, edge_flags) = {
            let table = EPOLL_TABLE.lock();
            let inst = match table.slots[idx].as_ref() {
                Some(i) => i,
                None => return -9,
            };
            let count = inst.watch_count;
            let mut snap = [(0i32, 0u32, 0u64); 64];
            let mut eflags = [false; 64];
            for j in 0..count {
                snap[j] = (
                    inst.watches[j].fd,
                    inst.watches[j].events,
                    inst.watches[j].data,
                );
                eflags[j] = inst.watches[j].events & EPOLLET != 0;
            }
            (count, snap, eflags)
        };

        for j in 0..watch_count {
            let (fd, requested, data) = watches_snap[j];
            let is_et = edge_flags[j];
            let rev = probe_readiness(fd, requested);
            if rev == 0 {
                continue;
            }

            // Edge-triggered: only report if state changed since last delivery.
            if is_et {
                let mut table = EPOLL_TABLE.lock();
                if let Some(inst) = table.get_mut(idx) {
                    if let Some(wi) = inst.find(fd) {
                        if inst.watches[wi].last_rev == rev {
                            continue; // already delivered this edge
                        }
                        inst.watches[wi].last_rev = rev;
                    }
                }
            }

            events[found] = EpollEvent { events: rev, data };
            found = found.saturating_add(1);
            if found >= cap {
                break 'outer;
            }
        }

        if found > 0 {
            break;
        }
    }

    found as i32
}

/// Close an epoll instance (free the table slot).
pub fn epoll_close(epfd: i32) {
    let idx = (epfd - EPOLL_FD_BASE) as usize;
    if idx >= MAX_EPOLL_FDS {
        return;
    }
    let fs_id = {
        let table = EPOLL_TABLE.lock();
        table.slots[idx].as_ref().map(|i| i.fs_id)
    };
    if let Some(id) = fs_id {
        crate::fs::epoll::epoll_close(id);
    }
    EPOLL_TABLE.lock().free(idx);
}

/// Initialize the IPC epoll subsystem.
pub fn init() {
    serial_println!("    [epoll] ipc epoll: ready (16 instances, 64 watches/instance)");
}
