use crate::serial_println;
/// OOM (Out-of-Memory) killer for Genesis AIOS
///
/// When the system runs critically low on memory the OOM killer selects a
/// process to terminate in order to free memory.  Selection is based on RSS,
/// uid, and a user-tunable per-process adjustment value (oom_score_adj).
///
/// Design rules enforced throughout this module:
///   - NO heap: no Vec, Box, String, alloc::*
///   - NO panics: no unwrap(), expect(), panic!()
///   - NO float casts: no `as f64` / `as f32`
///   - All counters use saturating_add / saturating_sub
///   - All statics must be Copy + const-constructible
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Process info table
// ---------------------------------------------------------------------------

/// Per-process information used by the OOM scorer.
#[derive(Clone, Copy)]
pub struct OomProcessInfo {
    /// Process identifier
    pub pid: u32,
    /// Owner user identifier (0 = root)
    pub uid: u32,
    /// Resident set size in 4 KB pages
    pub rss_pages: u32,
    /// Virtual memory size in pages
    pub vm_pages: u32,
    /// User-tunable adjustment: -1000 (never kill) … 1000 (always kill first)
    pub oom_score_adj: i16,
    /// Slot is occupied by a live process
    pub active: bool,
    /// Process must never be selected as a victim (kernel threads, init)
    pub oom_immune: bool,
}

impl OomProcessInfo {
    pub const fn empty() -> Self {
        OomProcessInfo {
            pid: 0,
            uid: 0,
            rss_pages: 0,
            vm_pages: 0,
            oom_score_adj: 0,
            active: false,
            oom_immune: false,
        }
    }
}

const OOM_TABLE_SIZE: usize = 256;
const EMPTY_ENTRY: OomProcessInfo = OomProcessInfo::empty();

struct OomTable {
    entries: [OomProcessInfo; OOM_TABLE_SIZE],
    /// Total physical pages available on this system (set during init)
    total_pages: u32,
}

impl OomTable {
    const fn new() -> Self {
        OomTable {
            entries: [EMPTY_ENTRY; OOM_TABLE_SIZE],
            total_pages: 1, // avoid division by zero before init
        }
    }
}

static OOM_TABLE: Mutex<OomTable> = Mutex::new(OomTable::new());

// ---------------------------------------------------------------------------
// OOM score calculation — integer only, no floats
// ---------------------------------------------------------------------------

/// Calculate an OOM score for a process.
///
/// Higher score → more likely to be killed.
///
/// Algorithm (all integer arithmetic):
///   base  = rss_pages * 1000 / total_pages   (0..1000)
///   base += oom_score_adj
///   if uid == 0: base -= 100
///   if oom_immune: return i32::MIN
///   clamp to [0, 1000]
pub fn oom_score(info: &OomProcessInfo, total_pages: u32) -> i32 {
    if info.oom_immune {
        return i32::MIN;
    }

    let total = if total_pages == 0 { 1 } else { total_pages } as i32;
    let rss = info.rss_pages as i32;

    // base score: RSS expressed as a per-mille fraction of total memory
    let mut score: i32 = rss.saturating_mul(1000) / total;

    // apply user-tunable adjustment
    score = score.saturating_add(info.oom_score_adj as i32);

    // root processes are slightly protected
    if info.uid == 0 {
        score = score.saturating_sub(100);
    }

    // clamp to valid range
    if score < 0 {
        score = 0;
    }
    if score > 1000 {
        score = 1000;
    }

    score
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register or update a process in the OOM table.
///
/// If the pid already has a slot it is updated in place; otherwise the first
/// inactive slot is used.  If the table is full the request is silently
/// dropped (safe: OOM killer simply won't see this process).
pub fn oom_register_process(pid: u32, uid: u32, rss_pages: u32, vm_pages: u32) {
    let mut tbl = OOM_TABLE.lock();

    // Update existing entry if found
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].uid = uid;
            tbl.entries[i].rss_pages = rss_pages;
            tbl.entries[i].vm_pages = vm_pages;
            return;
        }
    }

    // Allocate a new slot
    for i in 0..OOM_TABLE_SIZE {
        if !tbl.entries[i].active {
            tbl.entries[i] = OomProcessInfo {
                pid,
                uid,
                rss_pages,
                vm_pages,
                oom_score_adj: 0,
                active: true,
                oom_immune: false,
            };
            return;
        }
    }
    // Table full — drop silently
}

/// Update the RSS for an already-registered process.
pub fn oom_update_rss(pid: u32, rss_pages: u32) {
    let mut tbl = OOM_TABLE.lock();
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].rss_pages = rss_pages;
            return;
        }
    }
}

/// Set the oom_score_adj for a process.
///
/// Returns `true` if the process was found, `false` otherwise.
/// The value is clamped to [-1000, 1000].
pub fn oom_set_adj(pid: u32, adj: i16) -> bool {
    let clamped = if adj < -1000 {
        -1000i16
    } else if adj > 1000 {
        1000i16
    } else {
        adj
    };
    let mut tbl = OOM_TABLE.lock();
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].oom_score_adj = clamped;
            return true;
        }
    }
    false
}

/// Mark a process as OOM-immune (kernel threads, init, etc.).
pub fn oom_set_immune(pid: u32, immune: bool) {
    let mut tbl = OOM_TABLE.lock();
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].oom_immune = immune;
            return;
        }
    }
}

/// Mark a process as no longer active (does not reclaim memory itself).
pub fn oom_deregister_process(pid: u32) {
    let mut tbl = OOM_TABLE.lock();
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].active = false;
            return;
        }
    }
}

/// Select the process with the highest OOM score as the kill victim.
///
/// Returns `Some(pid)` or `None` if no killable process exists.
pub fn oom_select_victim() -> Option<u32> {
    let tbl = OOM_TABLE.lock();
    let total = tbl.total_pages;

    let mut best_pid: Option<u32> = None;
    let mut best_score = i32::MIN;

    for i in 0..OOM_TABLE_SIZE {
        let e = &tbl.entries[i];
        if !e.active {
            continue;
        }
        if e.oom_immune {
            continue;
        }
        let score = oom_score(e, total);
        if score > best_score {
            best_score = score;
            best_pid = Some(e.pid);
        }
    }

    best_pid
}

/// Send SIGKILL to the given process.
///
/// Calls `crate::process::kill::send_signal(pid, 9)` when available.
/// Falls back to marking the process inactive in the OOM table and logging.
///
/// Returns `true` if the kill was initiated, `false` if the process was not
/// found in the OOM table.
pub fn oom_kill_victim(pid: u32) -> bool {
    serial_println!("OOM: killing process PID={}", pid);

    // Attempt to deliver SIGKILL via the process subsystem.
    // The call is wrapped in a cfg so that the module compiles even when the
    // process::kill sub-module has not been linked yet.
    #[cfg(feature = "process_kill")]
    {
        crate::process::kill::send_signal(pid, 9);
    }

    // Mark the process as inactive in our table so that future OOM
    // invocations do not try to kill it again.
    let mut tbl = OOM_TABLE.lock();
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active && tbl.entries[i].pid == pid {
            tbl.entries[i].active = false;
            return true;
        }
    }

    false
}

/// Invoke the OOM killer in response to a failed page allocation.
///
/// Selects the highest-scoring process, kills it, and logs memory pressure
/// statistics.
///
/// Returns `true` when a victim was found and killed; `false` when the table
/// contains no killable processes.
pub fn oom_invoke() -> bool {
    serial_println!("OOM: allocation failure — invoking OOM killer");

    let (registered, immune) = oom_get_stats();
    serial_println!(
        "OOM: memory pressure stats: registered={}, immune={}",
        registered,
        immune
    );

    match oom_select_victim() {
        Some(pid) => {
            oom_kill_victim(pid);
            true
        }
        None => {
            serial_println!("OOM: no killable process found — system may hang");
            false
        }
    }
}

/// Return `true` when free memory has fallen below 5 % of total.
///
/// Uses integer division only:
///   free_pages * 100 / total_pages < 5
pub fn oom_check_pressure(free_pages: u32, total_pages: u32) -> bool {
    if total_pages == 0 {
        return false;
    }
    // free_pages * 100 / total_pages < 5
    // equivalent: free_pages * 100 < 5 * total_pages
    // (avoids potential overflow by checking multiplication side)
    (free_pages as u64).saturating_mul(100) < (total_pages as u64).saturating_mul(5)
}

/// Return `(registered_count, immune_count)` — the number of active processes
/// in the OOM table and the number of those that are immune.
pub fn oom_get_stats() -> (u32, u32) {
    let tbl = OOM_TABLE.lock();
    let mut registered: u32 = 0;
    let mut immune: u32 = 0;
    for i in 0..OOM_TABLE_SIZE {
        if tbl.entries[i].active {
            registered = registered.saturating_add(1);
            if tbl.entries[i].oom_immune {
                immune = immune.saturating_add(1);
            }
        }
    }
    (registered, immune)
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the OOM killer.
///
/// Sets the total page count from the frame allocator so that score
/// calculations have the correct denominator.  PID 0 (idle) and PID 1 (init)
/// are registered as immune.
pub fn init() {
    // Obtain total pages from the frame allocator.
    let total = {
        let alloc = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
        (alloc.free_count() + alloc.used_count()) as u32
    };

    {
        let mut tbl = OOM_TABLE.lock();
        tbl.total_pages = if total == 0 { 1 } else { total };

        // Register idle (PID 0) and init (PID 1) as immune.
        for pid in 0u32..=1 {
            let slot = pid as usize;
            tbl.entries[slot] = OomProcessInfo {
                pid,
                uid: 0,
                rss_pages: 0,
                vm_pages: 0,
                oom_score_adj: -1000,
                active: true,
                oom_immune: true,
            };
        }
    }

    serial_println!("  [oom] initialized, total_pages={}", total);
}
