use super::pcb::PROCESS_TABLE;
use super::scheduler::SCHEDULER;
use super::MAX_PROCESSES;
use crate::serial_println;
/// POSIX Session and Process Group Management
///
/// Implements sessions and process groups for job control.
///
/// POSIX model:
///   - Every process belongs to exactly one process group.
///   - Every process group belongs to exactly one session.
///   - A session has at most one controlling terminal.
///   - A process group is "foreground" if it holds the controlling terminal.
///
/// Capacity limits (no heap):
///   - Up to 32 sessions
///   - Up to 64 process groups
///   - Up to 32 members per process group
///   - Up to 16 process groups per session
///
/// Integration notes:
///   - `init()` must be called from `process::init()` during kernel boot.
///   - On fork (process/fork.rs `do_fork`), the child must inherit the
///     parent's pgid and sid.  The relevant fields are already copied in
///     `Process::fork()` (pcb.rs lines ~962-963).  No additional hook is
///     needed in fork.rs unless you wish to call `add_to_pgroup()` to keep
///     the session/pgroup tables in sync with the PCB table.
///   - Session/pgroup table entries are *advisory* mirrors of the pgid/sid
///     fields stored directly on each PCB.  The canonical source of truth
///     for a process's pgid/sid is its PCB entry in PROCESS_TABLE.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Error codes (negated POSIX errno values as i32)
// ---------------------------------------------------------------------------

const EPERM: i32 = -1;
const ESRCH: i32 = -3;
const EINVAL: i32 = -22;

// ---------------------------------------------------------------------------
// Process Group
// ---------------------------------------------------------------------------

/// A POSIX process group.
#[derive(Clone, Copy)]
pub struct ProcessGroup {
    /// Process group ID (0 = slot empty)
    pub pgid: u32,
    /// Session this group belongs to
    pub session_id: u32,
    /// PIDs that are members of this group
    pub members: [u32; 32],
    /// Number of valid entries in `members`
    pub member_count: u32,
    /// True when this group currently holds the controlling terminal
    pub foreground: bool,
}

impl ProcessGroup {
    const fn empty() -> Self {
        ProcessGroup {
            pgid: 0,
            session_id: 0,
            members: [0u32; 32],
            member_count: 0,
            foreground: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A POSIX session.
#[derive(Clone, Copy)]
pub struct Session {
    /// Session ID (0 = slot empty)
    pub sid: u32,
    /// PID of the session leader
    pub leader_pid: u32,
    /// PGIDs that belong to this session
    pub groups: [u32; 16],
    /// Number of valid entries in `groups`
    pub group_count: u32,
    /// Controlling terminal device ID (None = no controlling terminal)
    pub controlling_tty: Option<u32>,
}

impl Session {
    const fn empty() -> Self {
        Session {
            sid: 0,
            leader_pid: 0,
            groups: [0u32; 16],
            group_count: 0,
            controlling_tty: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

struct SessionTable {
    slots: [Session; 32],
    count: u32,
}

impl SessionTable {
    const fn new() -> Self {
        SessionTable {
            slots: [Session::empty(); 32],
            count: 0,
        }
    }

    /// Find a session by SID.  Returns `None` if not found.
    fn find(&self, sid: u32) -> Option<usize> {
        if sid == 0 {
            return None;
        }
        for i in 0..32 {
            if self.slots[i].sid == sid {
                return Some(i);
            }
        }
        None
    }

    /// Allocate a new empty slot.  Returns the index or `None` if full.
    fn alloc(&mut self) -> Option<usize> {
        for i in 0..32 {
            if self.slots[i].sid == 0 {
                return Some(i);
            }
        }
        None
    }
}

struct PgroupTable {
    slots: [ProcessGroup; 64],
    count: u32,
}

impl PgroupTable {
    const fn new() -> Self {
        PgroupTable {
            slots: [ProcessGroup::empty(); 64],
            count: 0,
        }
    }

    /// Find a process group by PGID.  Returns `None` if not found.
    fn find(&self, pgid: u32) -> Option<usize> {
        if pgid == 0 {
            return None;
        }
        for i in 0..64 {
            if self.slots[i].pgid == pgid {
                return Some(i);
            }
        }
        None
    }

    /// Allocate a new empty slot.  Returns the index or `None` if full.
    fn alloc(&mut self) -> Option<usize> {
        for i in 0..64 {
            if self.slots[i].pgid == 0 {
                return Some(i);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Static tables protected by spinlocks
// ---------------------------------------------------------------------------

static SESSIONS: Mutex<SessionTable> = Mutex::new(SessionTable::new());
static PGROUPS: Mutex<PgroupTable> = Mutex::new(PgroupTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the session/process-group subsystem.
///
/// Creates the initial session (SID=1) for the init process.
pub fn init() {
    {
        let mut sessions = SESSIONS.lock();
        if let Some(idx) = sessions.alloc() {
            sessions.slots[idx] = Session {
                sid: 1,
                leader_pid: 1,
                groups: {
                    let mut g = [0u32; 16];
                    g[0] = 1;
                    g
                },
                group_count: 1,
                controlling_tty: None,
            };
            sessions.count = sessions.count.saturating_add(1);
        }
    }
    {
        let mut pgroups = PGROUPS.lock();
        if let Some(idx) = pgroups.alloc() {
            pgroups.slots[idx] = ProcessGroup {
                pgid: 1,
                session_id: 1,
                members: {
                    let mut m = [0u32; 32];
                    m[0] = 1;
                    m
                },
                member_count: 1,
                foreground: true,
            };
            pgroups.count = pgroups.count.saturating_add(1);
        }
    }
    serial_println!("  Session: subsystem ready (max 32 sessions, 64 pgroups)");
}

/// `setsid` — create a new session.
///
/// The calling process becomes the session leader and sole member of a new
/// process group with the same ID as the session.
///
/// Returns the new session ID on success, or a negative errno value:
///   -EPERM  if the calling process is already a process group leader.
pub fn setsid() -> i32 {
    let pid = SCHEDULER.lock().current();

    // POSIX: a process group leader cannot call setsid().
    {
        let table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_ref() {
            if proc.is_group_leader() {
                return EPERM;
            }
        } else {
            return ESRCH;
        }
    }

    let new_sid = pid; // new session ID == caller's PID

    // Update PCB first.
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            proc.sid = new_sid;
            proc.pgid = new_sid;
        }
    }

    // Create advisory session record.
    create_session(pid);

    // Create advisory process group record for the new group.
    add_to_pgroup(pid, new_sid);

    serial_println!("  Session: PID {} created session {}", pid, new_sid);
    new_sid as i32
}

/// `getsid` — get the session ID of a process.
///
/// If `pid` is 0, returns the caller's session ID.
/// Returns the session ID on success, or -ESRCH if the process does not exist.
pub fn getsid(pid: u32) -> i32 {
    let target = if pid == 0 {
        SCHEDULER.lock().current()
    } else {
        pid
    };
    if target as usize >= MAX_PROCESSES {
        return ESRCH;
    }
    let table = PROCESS_TABLE.lock();
    match table[target as usize].as_ref() {
        Some(proc) => proc.sid as i32,
        None => ESRCH,
    }
}

/// `setpgid` — set the process group of a process.
///
/// If `pid` is 0, sets the caller's own process group.
/// If `pgid` is 0, sets the process group to `pid` (makes the process a
/// group leader).
///
/// Returns 0 on success, or a negative errno value.
pub fn setpgid(pid: u32, pgid: u32) -> i32 {
    let caller = SCHEDULER.lock().current();
    let target = if pid == 0 { caller } else { pid };
    let new_pgid = if pgid == 0 { target } else { pgid };

    if target as usize >= MAX_PROCESSES {
        return ESRCH;
    }

    // Permission check: can only set self or a child.
    {
        let table = PROCESS_TABLE.lock();
        let is_self = target == caller;
        let is_child = table[caller as usize]
            .as_ref()
            .map(|p| p.children.contains(&target))
            .unwrap_or(false);
        if !is_self && !is_child {
            return EPERM;
        }
        // Session leaders cannot change their pgroup.
        if let Some(proc) = table[target as usize].as_ref() {
            if proc.is_session_leader() {
                return EPERM;
            }
        } else {
            return ESRCH;
        }

        // The destination pgroup must already exist in the same session,
        // unless the process is creating a new group equal to its own PID.
        if new_pgid != target {
            let target_sid = table[target as usize].as_ref().map(|p| p.sid).unwrap_or(0);
            let group_exists = table.iter().any(|slot| {
                slot.as_ref()
                    .map(|p| p.pgid == new_pgid && p.sid == target_sid)
                    .unwrap_or(false)
            });
            if !group_exists {
                return EINVAL;
            }
        }
    }

    // Remove from old pgroup in advisory table.
    {
        let table = PROCESS_TABLE.lock();
        if let Some(old_pgid) = table[target as usize].as_ref().map(|p| p.pgid) {
            drop(table);
            remove_from_pgroup(target, old_pgid);
        }
    }

    // Update PCB.
    {
        let mut table = PROCESS_TABLE.lock();
        if let Some(proc) = table[target as usize].as_mut() {
            proc.set_pgid(new_pgid);
        }
    }

    // Add to new pgroup in advisory table.
    add_to_pgroup(target, new_pgid);

    0
}

/// `getpgid` — get the process group ID of a process.
///
/// If `pid` is 0, returns the caller's PGID.
/// Returns the PGID on success, or -ESRCH if the process does not exist.
pub fn getpgid(pid: u32) -> i32 {
    let target = if pid == 0 {
        SCHEDULER.lock().current()
    } else {
        pid
    };
    if target as usize >= MAX_PROCESSES {
        return ESRCH;
    }
    let table = PROCESS_TABLE.lock();
    match table[target as usize].as_ref() {
        Some(proc) => proc.pgid as i32,
        None => ESRCH,
    }
}

/// `getpgrp` — return the PGID of the calling process.
pub fn getpgrp() -> u32 {
    let pid = SCHEDULER.lock().current();
    let table = PROCESS_TABLE.lock();
    table[pid as usize].as_ref().map(|p| p.pgid).unwrap_or(0)
}

/// `tcgetpgrp` — return the foreground process group for the terminal
/// associated with file descriptor `fd`.
///
/// Currently there is only one terminal, so `fd` is accepted but ignored;
/// the foreground PGID is the first foreground entry in the pgroup table.
///
/// Returns the PGID on success, or a negative errno value.
pub fn tcgetpgrp(_fd: i32) -> i32 {
    let pgroups = PGROUPS.lock();
    for slot in pgroups.slots.iter() {
        if slot.pgid != 0 && slot.foreground {
            return slot.pgid as i32;
        }
    }
    ESRCH
}

/// `tcsetpgrp` — set the foreground process group for the terminal
/// associated with file descriptor `fd`.
///
/// The `pgid` must belong to the same session as the caller.
/// Returns 0 on success, or a negative errno value.
pub fn tcsetpgrp(_fd: i32, pgid: u32) -> i32 {
    // Verify that the pgid is in the caller's session.
    let caller = SCHEDULER.lock().current();
    let caller_sid = {
        let table = PROCESS_TABLE.lock();
        table[caller as usize].as_ref().map(|p| p.sid).unwrap_or(0)
    };

    let idx = {
        let pgroups = PGROUPS.lock();
        match pgroups.find(pgid) {
            Some(i) => {
                if pgroups.slots[i].session_id != caller_sid {
                    return EPERM;
                }
                i
            }
            None => return ESRCH,
        }
    };

    // Clear current foreground group in this session, then set the new one.
    {
        let mut pgroups = PGROUPS.lock();
        for slot in pgroups.slots.iter_mut() {
            if slot.pgid != 0 && slot.session_id == caller_sid {
                slot.foreground = false;
            }
        }
        pgroups.slots[idx].foreground = true;
    }
    0
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Create an advisory session record for `leader_pid`.
///
/// The new SID equals `leader_pid`.  If a session record already exists for
/// that SID it is returned as-is.  Returns the new session ID.
pub fn create_session(leader_pid: u32) -> u32 {
    let sid = leader_pid;
    let mut sessions = SESSIONS.lock();
    // Idempotent: return existing record if already present.
    if sessions.find(sid).is_some() {
        return sid;
    }
    if let Some(idx) = sessions.alloc() {
        sessions.slots[idx] = Session {
            sid,
            leader_pid,
            groups: {
                let mut g = [0u32; 16];
                g[0] = sid; // leader starts in pgroup == sid
                g
            },
            group_count: 1,
            controlling_tty: None,
        };
        sessions.count = sessions.count.saturating_add(1);
    }
    sid
}

/// Add `pid` to process group `pgid` in the advisory table.
///
/// Creates the process group record if it does not already exist.
/// Returns `true` on success, `false` if the table is full.
pub fn add_to_pgroup(pid: u32, pgid: u32) -> bool {
    if pgid == 0 {
        return false;
    }

    // Derive session ID from the process's PCB.
    let sid = {
        if pid as usize >= MAX_PROCESSES {
            return false;
        }
        let table = PROCESS_TABLE.lock();
        table[pid as usize].as_ref().map(|p| p.sid).unwrap_or(pgid)
    };

    let mut pgroups = PGROUPS.lock();
    if let Some(idx) = pgroups.find(pgid) {
        // Group exists — add the member if not already present.
        let slot = &mut pgroups.slots[idx];
        for i in 0..slot.member_count as usize {
            if slot.members[i] == pid {
                return true; // already a member
            }
        }
        if slot.member_count as usize >= 32 {
            return false; // group is full
        }
        let mc = slot.member_count as usize;
        slot.members[mc] = pid;
        slot.member_count = slot.member_count.saturating_add(1);
        true
    } else {
        // Create a new group.
        match pgroups.alloc() {
            Some(idx) => {
                pgroups.slots[idx] = ProcessGroup {
                    pgid,
                    session_id: sid,
                    members: {
                        let mut m = [0u32; 32];
                        m[0] = pid;
                        m
                    },
                    member_count: 1,
                    foreground: false,
                };
                pgroups.count = pgroups.count.saturating_add(1);

                // Register the group in its session's group list.
                drop(pgroups); // release before taking sessions lock
                let mut sessions = SESSIONS.lock();
                if let Some(sidx) = sessions.find(sid) {
                    let sess = &mut sessions.slots[sidx];
                    if sess.group_count < 16 {
                        let gc = sess.group_count as usize;
                        sess.groups[gc] = pgid;
                        sess.group_count = sess.group_count.saturating_add(1);
                    }
                }
                true
            }
            None => false,
        }
    }
}

/// Remove `pid` from the advisory process group `pgid`.
///
/// If the group becomes empty its slot is freed and removed from the session.
pub fn remove_from_pgroup(pid: u32, pgid: u32) {
    if pgid == 0 {
        return;
    }
    let mut pgroups = PGROUPS.lock();
    let idx = match pgroups.find(pgid) {
        Some(i) => i,
        None => return,
    };

    let slot = &mut pgroups.slots[idx];
    // Remove pid from member list by swapping with the last entry.
    let mut found = false;
    let mut i = 0u32;
    while i < slot.member_count {
        if slot.members[i as usize] == pid {
            let last = (slot.member_count - 1) as usize;
            slot.members[i as usize] = slot.members[last];
            slot.members[last] = 0;
            slot.member_count = slot.member_count.saturating_sub(1);
            found = true;
            break;
        }
        i = i.saturating_add(1);
    }

    if !found {
        return;
    }

    // If the group is now empty, release the slot and remove from session.
    if slot.member_count == 0 {
        let sid = slot.session_id;
        slot.pgid = 0; // free the slot

        drop(pgroups); // release before taking sessions lock
        let mut sessions = SESSIONS.lock();
        if let Some(sidx) = sessions.find(sid) {
            let sess = &mut sessions.slots[sidx];
            let mut j = 0u32;
            while j < sess.group_count {
                if sess.groups[j as usize] == pgid {
                    let last = (sess.group_count - 1) as usize;
                    sess.groups[j as usize] = sess.groups[last];
                    sess.groups[last] = 0;
                    sess.group_count = sess.group_count.saturating_sub(1);
                    break;
                }
                j = j.saturating_add(1);
            }
        }
    }
}

/// Returns `true` if every process in `pgid` has a parent that is
/// **outside** the same session — i.e. the group is orphaned.
///
/// An orphaned process group cannot receive job-control signals from a
/// session leader exiting, so the kernel must send SIGHUP+SIGCONT to it.
pub fn pgroup_is_orphaned(pgid: u32) -> bool {
    // Collect members from the PCB table (canonical source).
    let table = PROCESS_TABLE.lock();

    // Find the session for this group.
    let sid = table.iter().find_map(|slot| {
        slot.as_ref()
            .and_then(|p| if p.pgid == pgid { Some(p.sid) } else { None })
    });
    let sid = match sid {
        Some(s) => s,
        None => return true, // group not found — treat as orphaned
    };

    // A group is orphaned if *no* member has a parent inside the same session.
    for slot in table.iter() {
        if let Some(proc) = slot.as_ref() {
            if proc.pgid != pgid {
                continue;
            }
            let parent_sid = if proc.parent_pid as usize >= MAX_PROCESSES {
                0u32
            } else {
                table[proc.parent_pid as usize]
                    .as_ref()
                    .map(|pp| pp.sid)
                    .unwrap_or(0)
            };
            if parent_sid == sid {
                return false; // at least one member has a parent in the session
            }
        }
    }
    true
}

/// Send `signal` to every process in process group `pgid`.
///
/// Delegates to `process::send_signal` for each member.  Errors on
/// individual processes are silently ignored so that all reachable
/// members receive the signal.
pub fn send_signal_to_pgroup(pgid: u32, signal: u32) {
    if signal >= 32 || pgid == 0 {
        return;
    }
    // Collect PIDs without holding the lock during signal delivery.
    let mut pids = [0u32; 32];
    let mut count = 0usize;
    {
        let table = PROCESS_TABLE.lock();
        for slot in table.iter() {
            if let Some(proc) = slot.as_ref() {
                if proc.pgid == pgid && count < 32 {
                    pids[count] = proc.pid;
                    count = count.saturating_add(1);
                }
            }
        }
    }
    for i in 0..count {
        let _ = super::send_signal(pids[i], signal as u8);
    }
}
