/// signalfd — receive POSIX signals via file descriptor reads
///
/// Allows processes to receive signals synchronously by reading an fd
/// rather than via asynchronous signal handlers.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

const MAX_SIGNALFDS: usize = 32;
const SFD_FD_OFFSET: u32 = 300;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct SigInfo {
    pub signo: u32,
    pub errno: i32,
    pub code: i32,
    pub pid: u32,
    pub uid: u32,
    pub addr: u64,
    pub active: bool,
}

impl SigInfo {
    pub const fn empty() -> Self {
        SigInfo {
            signo: 0,
            errno: 0,
            code: 0,
            pid: 0,
            uid: 0,
            addr: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct SignalFd {
    pub id: u32,
    pub sigmask: u64, // bitmask of signals to receive
    pub pending: [SigInfo; 32],
    pub npending: u8,
    pub pid: u32, // owning process
    pub flags: u32,
    pub active: bool,
}

impl SignalFd {
    pub const fn empty() -> Self {
        const EMPTY_SIG: SigInfo = SigInfo::empty();
        SignalFd {
            id: 0,
            sigmask: 0,
            pending: [EMPTY_SIG; 32],
            npending: 0,
            pid: 0,
            flags: 0,
            active: false,
        }
    }
}

const EMPTY_SFD: SignalFd = SignalFd::empty();
static SIGNALFDS: Mutex<[SignalFd; MAX_SIGNALFDS]> = Mutex::new([EMPTY_SFD; MAX_SIGNALFDS]);
static SFD_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn signalfd_create(pid: u32, sigmask: u64, flags: u32) -> Option<u32> {
    let id = SFD_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut sfds = SIGNALFDS.lock();
    let mut i = 0usize;
    while i < MAX_SIGNALFDS {
        if !sfds[i].active {
            sfds[i] = SignalFd {
                id,
                sigmask,
                pid,
                flags,
                active: true,
                ..SignalFd::empty()
            };
            return Some(id.saturating_add(SFD_FD_OFFSET));
        }
        i = i.saturating_add(1);
    }
    None
}

/// Deliver a signal to all matching signalfds.
pub fn signalfd_deliver(target_pid: u32, signo: u32, from_pid: u32, code: i32) {
    if signo == 0 || signo > 63 {
        return;
    }
    let bit = 1u64 << (signo & 63);
    let mut sfds = SIGNALFDS.lock();
    let mut i = 0usize;
    while i < MAX_SIGNALFDS {
        if sfds[i].active && sfds[i].pid == target_pid && sfds[i].sigmask & bit != 0 {
            let np = sfds[i].npending as usize;
            if np < 32 {
                sfds[i].pending[np] = SigInfo {
                    signo,
                    errno: 0,
                    code,
                    pid: from_pid,
                    uid: 0,
                    addr: 0,
                    active: true,
                };
                sfds[i].npending = sfds[i].npending.saturating_add(1);
            }
        }
        i = i.saturating_add(1);
    }
}

/// Dequeue up to 8 signals from the fd.
pub fn signalfd_read(fd: u32, out: &mut [SigInfo; 8]) -> usize {
    let id = fd.wrapping_sub(SFD_FD_OFFSET);
    let mut sfds = SIGNALFDS.lock();
    let mut j = 0usize;
    while j < MAX_SIGNALFDS {
        if sfds[j].active && sfds[j].id == id {
            let count = (sfds[j].npending as usize).min(8);
            let mut k = 0usize;
            while k < count {
                out[k] = sfds[j].pending[k];
                k = k.saturating_add(1);
            }
            // Shift remaining signals down
            let remaining = sfds[j].npending as usize - count;
            let mut r = 0usize;
            while r < remaining {
                sfds[j].pending[r] = sfds[j].pending[r + count];
                r = r.saturating_add(1);
            }
            sfds[j].npending = remaining as u8;
            return count;
        }
        j = j.saturating_add(1);
    }
    0
}

pub fn signalfd_setmask(fd: u32, sigmask: u64) -> bool {
    let id = fd.wrapping_sub(SFD_FD_OFFSET);
    let mut sfds = SIGNALFDS.lock();
    let mut i = 0usize;
    while i < MAX_SIGNALFDS {
        if sfds[i].active && sfds[i].id == id {
            sfds[i].sigmask = sigmask;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn signalfd_close(fd: u32) -> bool {
    let id = fd.wrapping_sub(SFD_FD_OFFSET);
    let mut sfds = SIGNALFDS.lock();
    let mut i = 0usize;
    while i < MAX_SIGNALFDS {
        if sfds[i].active && sfds[i].id == id {
            sfds[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    serial_println!("[signalfd] signalfd primitive initialized");
}
