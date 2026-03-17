/// eventfd — file-descriptor-based event notification (semaphore counter)
///
/// An eventfd is a counter-based synchronization primitive. Writes add to
/// the counter; reads consume it. In semaphore mode, reads decrement by 1.
///
/// Rules: no_std, no heap (no Vec/Box/alloc), no floats, no panics.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const EFD_SEMAPHORE: u32 = 1;
pub const EFD_NONBLOCK: u32 = 2048;
pub const EFD_CLOEXEC: u32 = 524288;

const MAX_EVENTFDS: usize = 64;
const EFD_FD_OFFSET: u32 = 100;
const EVENTFD_MAX_VALUE: u64 = 0xFFFF_FFFF_FFFF_FFFE;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct EventFd {
    pub id: u32,
    pub counter: u64,
    pub flags: u32,
    pub owner_pid: u32,
    pub total_reads: u64,
    pub total_writes: u64,
    pub active: bool,
}

impl EventFd {
    pub const fn empty() -> Self {
        EventFd {
            id: 0,
            counter: 0,
            flags: 0,
            owner_pid: 0,
            total_reads: 0,
            total_writes: 0,
            active: false,
        }
    }
    pub fn is_semaphore(&self) -> bool {
        self.flags & EFD_SEMAPHORE != 0
    }
}

const EMPTY_EFD: EventFd = EventFd::empty();
static EVENTFDS: Mutex<[EventFd; MAX_EVENTFDS]> = Mutex::new([EMPTY_EFD; MAX_EVENTFDS]);
static EFD_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn create(owner_pid: u32, initval: u64, flags: u32) -> Option<u32> {
    if initval > EVENTFD_MAX_VALUE {
        return None;
    }
    let id = EFD_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if !efds[i].active {
            efds[i] = EventFd {
                id,
                counter: initval,
                flags,
                owner_pid,
                total_reads: 0,
                total_writes: 0,
                active: true,
            };
            return Some(id.saturating_add(EFD_FD_OFFSET));
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn write(fd: u32, value: u64) -> bool {
    if value == 0 {
        return false;
    }
    let id = fd.wrapping_sub(EFD_FD_OFFSET);
    let mut efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].id == id {
            let new_val = efds[i].counter.saturating_add(value).min(EVENTFD_MAX_VALUE);
            efds[i].counter = new_val;
            efds[i].total_writes = efds[i].total_writes.saturating_add(1);
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn read(fd: u32) -> Option<u64> {
    let id = fd.wrapping_sub(EFD_FD_OFFSET);
    let mut efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].id == id {
            if efds[i].counter == 0 {
                return None;
            } // would block
            efds[i].total_reads = efds[i].total_reads.saturating_add(1);
            if efds[i].is_semaphore() {
                efds[i].counter = efds[i].counter.saturating_sub(1);
                return Some(1);
            } else {
                let val = efds[i].counter;
                efds[i].counter = 0;
                return Some(val);
            }
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn poll(fd: u32) -> Option<(bool, bool)> {
    // (readable, writable)
    let id = fd.wrapping_sub(EFD_FD_OFFSET);
    let efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].id == id {
            return Some((efds[i].counter > 0, efds[i].counter < EVENTFD_MAX_VALUE));
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn close(fd: u32) -> bool {
    let id = fd.wrapping_sub(EFD_FD_OFFSET);
    let mut efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].id == id {
            efds[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn close_all(pid: u32) {
    let mut efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].owner_pid == pid {
            efds[i].active = false;
        }
        i = i.saturating_add(1);
    }
}

pub fn counter_value(fd: u32) -> Option<u64> {
    let id = fd.wrapping_sub(EFD_FD_OFFSET);
    let efds = EVENTFDS.lock();
    let mut i = 0usize;
    while i < MAX_EVENTFDS {
        if efds[i].active && efds[i].id == id {
            return Some(efds[i].counter);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn init() {
    serial_println!(
        "    [eventfd] Event file descriptor subsystem ready (max {})",
        MAX_EVENTFDS
    );
}
