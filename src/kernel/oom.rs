/// oom — Out-of-Memory killer
///
/// When memory allocation fails, selects the "best" process to kill to
/// recover memory. Scoring algorithm:
///
///   base_score  = (rss_pages * 1000) / total_pages
///   adj_score   = base_score + oom_score_adj   (clamped 0..1000)
///   final_score = adj_score * child_factor      (has children → higher score)
///
/// Processes with oom_score_adj == -1000 are immune (never killed).
/// Kernel threads (uid==0, no user-facing) are also skipped.
///
/// Inspired by: Linux mm/oom_kill.c. All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of processes tracked by the OOM killer.
const MAX_PROCS: usize = 256;

/// Minimum oom_score_adj — process is immune from OOM kill.
pub const OOM_SCORE_ADJ_MIN: i16 = -1000;
/// Maximum oom_score_adj — process is most preferred for kill.
pub const OOM_SCORE_ADJ_MAX: i16 = 1000;

// ---------------------------------------------------------------------------
// Per-process OOM data
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct OomEntry {
    pub pid: u32,
    pub uid: u32,
    pub rss_pages: u64,     // current resident set size in pages
    pub oom_score_adj: i16, // user-adjustable bias (-1000..1000)
    pub child_count: u16,   // number of child processes
    pub is_kernel: bool,    // kernel thread — immune
    pub active: bool,
}

impl OomEntry {
    pub const fn empty() -> Self {
        OomEntry {
            pid: 0,
            uid: 0,
            rss_pages: 0,
            oom_score_adj: 0,
            child_count: 0,
            is_kernel: false,
            active: false,
        }
    }
}

const EMPTY_ENTRY: OomEntry = OomEntry::empty();
static OOM_TABLE: Mutex<[OomEntry; MAX_PROCS]> = Mutex::new([EMPTY_ENTRY; MAX_PROCS]);
static TOTAL_PAGES: AtomicU64 = AtomicU64::new(1); // avoid div/0; set from memory init

// ---------------------------------------------------------------------------
// OOM event log
// ---------------------------------------------------------------------------

const OOM_LOG_SIZE: usize = 32;

#[derive(Copy, Clone)]
pub struct OomEvent {
    pub killed_pid: u32,
    pub killed_score: u32,
    pub rss_freed: u64,
    pub seq: u64,
    pub valid: bool,
}

impl OomEvent {
    pub const fn empty() -> Self {
        OomEvent {
            killed_pid: 0,
            killed_score: 0,
            rss_freed: 0,
            seq: 0,
            valid: false,
        }
    }
}

const EMPTY_EVENT: OomEvent = OomEvent::empty();
static OOM_LOG: Mutex<[OomEvent; OOM_LOG_SIZE]> = Mutex::new([EMPTY_EVENT; OOM_LOG_SIZE]);
static OOM_LOG_HEAD: AtomicU64 = AtomicU64::new(0);
static OOM_SEQ: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// Public API: registration
// ---------------------------------------------------------------------------

pub fn oom_register(pid: u32, uid: u32, is_kernel: bool) {
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if !tbl[i].active {
            tbl[i] = OomEntry::empty();
            tbl[i].pid = pid;
            tbl[i].uid = uid;
            tbl[i].is_kernel = is_kernel;
            tbl[i].active = true;
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn oom_unregister(pid: u32) {
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            tbl[i].active = false;
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn oom_update_rss(pid: u32, rss_pages: u64) {
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            tbl[i].rss_pages = rss_pages;
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn oom_set_adj(pid: u32, adj: i16) {
    let clamped = if adj < OOM_SCORE_ADJ_MIN {
        OOM_SCORE_ADJ_MIN
    } else if adj > OOM_SCORE_ADJ_MAX {
        OOM_SCORE_ADJ_MAX
    } else {
        adj
    };
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            tbl[i].oom_score_adj = clamped;
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn oom_child_add(pid: u32) {
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            tbl[i].child_count = tbl[i].child_count.saturating_add(1);
            return;
        }
        i = i.saturating_add(1);
    }
}

pub fn oom_child_remove(pid: u32) {
    let mut tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            tbl[i].child_count = tbl[i].child_count.saturating_sub(1);
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Set total system page count (call from memory subsystem init).
pub fn oom_set_total_pages(pages: u64) {
    TOTAL_PAGES.store(pages.max(1), Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// OOM score calculation
// ---------------------------------------------------------------------------

/// Compute OOM score for a single entry. Higher = more likely to kill.
/// Returns 0 for immune processes.
fn oom_score(entry: &OomEntry, total_pages: u64) -> u32 {
    if entry.is_kernel {
        return 0;
    }
    if entry.oom_score_adj == OOM_SCORE_ADJ_MIN {
        return 0;
    }

    // base: 0..1000 proportional to RSS
    let base = if total_pages == 0 {
        0u32
    } else {
        ((entry.rss_pages.saturating_mul(1000)) / total_pages).min(1000) as u32
    };

    // Apply adj (can go negative — clamp to 0)
    let adj = entry.oom_score_adj as i32;
    let adjusted = (base as i32).saturating_add(adj).max(0).min(1000) as u32;

    // Child factor: +10% per child (saturating)
    let child_boost = (entry.child_count as u32).saturating_mul(10).min(200);
    adjusted.saturating_add(child_boost)
}

/// Select the process with the highest OOM score for termination.
/// Returns (pid, score) or None if no eligible process found.
pub fn oom_select_victim() -> Option<(u32, u32)> {
    let tbl = OOM_TABLE.lock();
    let total = TOTAL_PAGES.load(Ordering::Relaxed);
    let mut best_pid = 0u32;
    let mut best_score = 0u32;
    let mut found = false;

    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active {
            let score = oom_score(&tbl[i], total);
            if score > best_score {
                best_score = score;
                best_pid = tbl[i].pid;
                found = true;
            }
        }
        i = i.saturating_add(1);
    }

    if found {
        Some((best_pid, best_score))
    } else {
        None
    }
}

/// Invoke the OOM killer. Selects the victim and records the kill event.
/// Returns the pid of the killed process (or 0 if none eligible).
/// The actual process termination must be performed by the caller.
pub fn oom_kill() -> u32 {
    let victim = match oom_select_victim() {
        Some(v) => v,
        None => {
            serial_println!("[oom] OOM killer invoked but no eligible victim");
            return 0;
        }
    };
    let (pid, score) = victim;

    // Find RSS to report
    let rss = {
        let tbl = OOM_TABLE.lock();
        let mut i = 0usize;
        let mut r = 0u64;
        while i < MAX_PROCS {
            if tbl[i].active && tbl[i].pid == pid {
                r = tbl[i].rss_pages;
                break;
            }
            i = i.saturating_add(1);
        }
        r
    };

    // Log the event
    let seq = OOM_SEQ.fetch_add(1, Ordering::Relaxed);
    let head = OOM_LOG_HEAD.fetch_add(1, Ordering::Relaxed) as usize % OOM_LOG_SIZE;
    {
        let mut log = OOM_LOG.lock();
        log[head] = OomEvent {
            killed_pid: pid,
            killed_score: score,
            rss_freed: rss,
            seq,
            valid: true,
        };
    }

    serial_println!(
        "[oom] killing pid {} (score {}, rss {} pages)",
        pid,
        score,
        rss
    );
    pid
}

/// Get the score of a specific process (for /proc/PID/oom_score).
pub fn oom_score_of(pid: u32) -> u32 {
    let tbl = OOM_TABLE.lock();
    let total = TOTAL_PAGES.load(Ordering::Relaxed);
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            return oom_score(&tbl[i], total);
        }
        i = i.saturating_add(1);
    }
    0
}

/// Get the oom_score_adj of a process.
pub fn oom_adj_of(pid: u32) -> i16 {
    let tbl = OOM_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_PROCS {
        if tbl[i].active && tbl[i].pid == pid {
            return tbl[i].oom_score_adj;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Drain up to 8 recent OOM kill events.
pub fn oom_recent_events(out: &mut [OomEvent; 8]) -> usize {
    let log = OOM_LOG.lock();
    let head = OOM_LOG_HEAD.load(Ordering::Relaxed) as usize;
    let mut count = 0usize;
    // Scan backwards from head
    let mut k = 0usize;
    while k < OOM_LOG_SIZE.min(8) {
        let idx = (head.wrapping_sub(1).wrapping_sub(k)) % OOM_LOG_SIZE;
        if log[idx].valid {
            out[count] = log[idx];
            count = count.saturating_add(1);
        }
        k = k.saturating_add(1);
    }
    count
}

pub fn init() {
    serial_println!(
        "[oom] OOM killer initialized (tracks up to {} processes)",
        MAX_PROCS
    );
}
