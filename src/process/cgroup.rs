use crate::serial_println;
use crate::sync::Mutex;
/// cgroup v2 resource controllers for Genesis
///
/// Implements a lightweight, no-heap cgroup v2 subsystem with CPU, memory,
/// and I/O resource accounting and enforcement.  All data lives in fixed-size
/// static arrays; no Vec, Box, String, or alloc::* is used anywhere.
///
/// Design follows the Linux cgroup v2 unified hierarchy:
///   - A root cgroup (id=1, name="root") is created at init time.
///   - Every cgroup carries a CPU weight (proportional scheduling),
///     optional period/quota (bandwidth limiting), a memory high/max limit,
///     and per-device I/O byte/IOPS limits.
///   - Process membership is exclusive: a PID belongs to exactly one cgroup
///     at a time.  Moving a PID atomically removes it from its current
///     cgroup before inserting it into the target.
///
/// No panics.  No floats.  All counters use saturating arithmetic.
/// Sequence numbers (cgroup IDs) use wrapping_add.
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Capacity constants
// ---------------------------------------------------------------------------

/// Maximum number of cgroups that can exist simultaneously (including root).
pub const MAX_CGROUPS: usize = 32;

/// Maximum number of PIDs that can be members of a single cgroup.
pub const MAX_CGROUP_PROCS: usize = 64;

// ---------------------------------------------------------------------------
// CPU controller
// ---------------------------------------------------------------------------

/// CPU bandwidth and weight configuration for a cgroup.
///
/// `weight`     — proportional share in the range 1..=10 000 (default 100).
///                Maps directly to the cgroup v2 `cpu.weight` knob.
/// `period_us`  — scheduler period in microseconds (0 = use kernel default).
/// `quota_us`   — CPU time allowed per period in microseconds;
///                -1 means unlimited (no quota enforcement).
/// `usage_us`   — accumulated CPU time charged to this cgroup since the last
///                `cgroup_reset_cpu_usage()` call.
#[derive(Clone, Copy)]
pub struct CgroupCpuCtrl {
    pub weight: u32,
    pub period_us: u64,
    pub quota_us: i64,
    pub usage_us: u64,
}

impl CgroupCpuCtrl {
    pub const fn default() -> Self {
        CgroupCpuCtrl {
            weight: 100,
            period_us: 100_000, // 100 ms — Linux default
            quota_us: -1,       // unlimited
            usage_us: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Memory controller
// ---------------------------------------------------------------------------

/// Memory accounting and limits for a cgroup.
///
/// `limit_bytes` — hard ceiling; u64::MAX = unlimited.
/// `usage_bytes` — currently charged byte count.
/// `high_bytes`  — soft high-water mark; triggers reclaim when exceeded
///                 (enforcement is caller's responsibility).
/// `swap_limit`  — maximum swap the cgroup may use; 0 = no swap allowed,
///                 u64::MAX = unlimited.
#[derive(Clone, Copy)]
pub struct CgroupMemCtrl {
    pub limit_bytes: u64,
    pub usage_bytes: u64,
    pub high_bytes: u64,
    pub swap_limit: u64,
}

impl CgroupMemCtrl {
    pub const fn default() -> Self {
        CgroupMemCtrl {
            limit_bytes: u64::MAX, // unlimited
            usage_bytes: 0,
            high_bytes: u64::MAX, // unlimited
            swap_limit: u64::MAX, // unlimited
        }
    }
}

// ---------------------------------------------------------------------------
// I/O controller
// ---------------------------------------------------------------------------

/// I/O bandwidth and IOPS limits plus cumulative byte counters.
///
/// `rbps_limit`  — read  bytes-per-second limit; 0 = unlimited.
/// `wbps_limit`  — write bytes-per-second limit; 0 = unlimited.
/// `riops_limit` — read  IOPS limit; 0 = unlimited.
/// `wiops_limit` — write IOPS limit; 0 = unlimited.
/// `rbytes`      — total bytes read   by processes in this cgroup.
/// `wbytes`      — total bytes written by processes in this cgroup.
#[derive(Clone, Copy)]
pub struct CgroupIoCtrl {
    pub rbps_limit: u64,
    pub wbps_limit: u64,
    pub riops_limit: u32,
    pub wiops_limit: u32,
    pub rbytes: u64,
    pub wbytes: u64,
}

impl CgroupIoCtrl {
    pub const fn default() -> Self {
        CgroupIoCtrl {
            rbps_limit: 0, // 0 = unlimited
            wbps_limit: 0,
            riops_limit: 0,
            wiops_limit: 0,
            rbytes: 0,
            wbytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Cgroup descriptor
// ---------------------------------------------------------------------------

/// A single cgroup node in the hierarchy.
///
/// `id`        — unique identifier assigned at creation time (1 = root).
/// `parent_id` — id of the parent cgroup; 0 = no parent (only root has this).
/// `name`      — UTF-8 name stored as a fixed-size byte array.
/// `name_len`  — valid byte count within `name`.
/// `cpu`       — CPU controller settings.
/// `mem`       — memory controller settings.
/// `io`        — I/O controller settings.
/// `procs`     — PIDs that are currently members of this cgroup.
/// `nprocs`    — number of valid entries in `procs`.
/// `active`    — false means the slot is free and may be reused.
#[derive(Clone, Copy)]
pub struct Cgroup {
    pub id: u32,
    pub parent_id: u32,
    pub name: [u8; 64],
    pub name_len: u8,
    pub cpu: CgroupCpuCtrl,
    pub mem: CgroupMemCtrl,
    pub io: CgroupIoCtrl,
    pub procs: [u32; MAX_CGROUP_PROCS],
    pub nprocs: u8,
    pub active: bool,
}

impl Cgroup {
    pub const fn empty() -> Self {
        Cgroup {
            id: 0,
            parent_id: 0,
            name: [0u8; 64],
            name_len: 0,
            cpu: CgroupCpuCtrl::default(),
            mem: CgroupMemCtrl::default(),
            io: CgroupIoCtrl::default(),
            procs: [0u32; MAX_CGROUP_PROCS],
            nprocs: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// All cgroup descriptors.  Slot 0 is always the root cgroup once init() runs.
static CGROUPS: Mutex<[Cgroup; MAX_CGROUPS]> = {
    const EMPTY: Cgroup = Cgroup::empty();
    Mutex::new([EMPTY; MAX_CGROUPS])
};

/// ID of the root cgroup (always 1 after init).
static ROOT_CGROUP_ID: AtomicU32 = AtomicU32::new(0);

/// Next ID to hand out; wraps on overflow (IDs are never 0).
static NEXT_CGROUP_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Allocate the next cgroup ID using wrapping arithmetic so the counter never
/// panics.  ID 0 is reserved as "no cgroup"; skip it if wrapping lands there.
fn alloc_cgroup_id() -> u32 {
    let id = NEXT_CGROUP_ID.fetch_add(1, Ordering::Relaxed);
    // wrapping_add in the atomic fetch covers the overflow case implicitly;
    // if the result happened to be 0 we hand out 1 instead.
    if id == 0 {
        1
    } else {
        id
    }
}

/// Find the slot index for a cgroup by its id.  Returns None if not found.
fn find_slot(cgroups: &[Cgroup; MAX_CGROUPS], id: u32) -> Option<usize> {
    for i in 0..MAX_CGROUPS {
        if cgroups[i].active && cgroups[i].id == id {
            return Some(i);
        }
    }
    None
}

/// Find the first free (inactive) slot.  Returns None if the table is full.
fn find_free_slot(cgroups: &[Cgroup; MAX_CGROUPS]) -> Option<usize> {
    for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            return Some(i);
        }
    }
    None
}

/// Remove `pid` from whatever cgroup currently contains it.
/// Called while the CGROUPS lock is already held.
fn remove_pid_from_all(cgroups: &mut [Cgroup; MAX_CGROUPS], pid: u32) {
    for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            continue;
        }
        let n = cgroups[i].nprocs as usize;
        let mut j = 0usize;
        while j < n {
            if cgroups[i].procs[j] == pid {
                // Swap-remove: move last element into this slot.
                let last = cgroups[i].nprocs.saturating_sub(1) as usize;
                cgroups[i].procs[j] = cgroups[i].procs[last];
                cgroups[i].procs[last] = 0;
                cgroups[i].nprocs = cgroups[i].nprocs.saturating_sub(1);
                return; // A PID appears in at most one cgroup.
            }
            j = j.saturating_add(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new cgroup with the given name bytes and parent cgroup id.
///
/// Returns the new cgroup's id on success, or None if:
///   - the cgroup table is full (MAX_CGROUPS reached),
///   - `parent_id` does not refer to an active cgroup (and is not 0),
///   - `name` is empty or longer than 63 bytes.
pub fn cgroup_create(name: &[u8], parent_id: u32) -> Option<u32> {
    if name.is_empty() || name.len() > 63 {
        return None;
    }

    let mut cgroups = CGROUPS.lock();

    // Validate parent exists (if not root creation).
    if parent_id != 0 {
        if find_slot(&cgroups, parent_id).is_none() {
            return None;
        }
    }

    let slot = find_free_slot(&cgroups)?;
    let id = alloc_cgroup_id();

    let cg = &mut cgroups[slot];
    *cg = Cgroup::empty();
    cg.id = id;
    cg.parent_id = parent_id;
    cg.active = true;

    // Copy name bytes.
    let copy_len = if name.len() > 63 { 63 } else { name.len() };
    let mut i = 0usize;
    while i < copy_len {
        cg.name[i] = name[i];
        i = i.saturating_add(1);
    }
    cg.name_len = copy_len as u8;

    Some(id)
}

/// Destroy a cgroup.
///
/// Returns false (and does NOT destroy) if the cgroup still has processes
/// attached or if `id` does not refer to an active cgroup.
pub fn cgroup_destroy(id: u32) -> bool {
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    // Refuse to destroy if the cgroup has live processes.
    if cgroups[slot].nprocs > 0 {
        return false;
    }
    cgroups[slot].active = false;
    true
}

/// Add `pid` to `cgroup_id`, atomically removing it from any previous cgroup.
///
/// Returns false if the cgroup does not exist or has reached MAX_CGROUP_PROCS.
pub fn cgroup_add_proc(cgroup_id: u32, pid: u32) -> bool {
    let mut cgroups = CGROUPS.lock();

    // Atomically remove from any current home first.
    remove_pid_from_all(&mut cgroups, pid);

    let slot = match find_slot(&cgroups, cgroup_id) {
        Some(s) => s,
        None => return false,
    };

    let n = cgroups[slot].nprocs as usize;
    if n >= MAX_CGROUP_PROCS {
        return false;
    }

    cgroups[slot].procs[n] = pid;
    cgroups[slot].nprocs = cgroups[slot].nprocs.saturating_add(1);
    true
}

/// Remove `pid` from `cgroup_id`.
///
/// Returns false if the cgroup does not exist or `pid` is not a member.
pub fn cgroup_remove_proc(cgroup_id: u32, pid: u32) -> bool {
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, cgroup_id) {
        Some(s) => s,
        None => return false,
    };

    let n = cgroups[slot].nprocs as usize;
    let mut found = false;
    let mut j = 0usize;
    while j < n {
        if cgroups[slot].procs[j] == pid {
            let last = cgroups[slot].nprocs.saturating_sub(1) as usize;
            cgroups[slot].procs[j] = cgroups[slot].procs[last];
            cgroups[slot].procs[last] = 0;
            cgroups[slot].nprocs = cgroups[slot].nprocs.saturating_sub(1);
            found = true;
            break;
        }
        j = j.saturating_add(1);
    }
    found
}

/// Return the cgroup id that currently contains `pid`, or None.
pub fn cgroup_find_proc(pid: u32) -> Option<u32> {
    let cgroups = CGROUPS.lock();
    for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            continue;
        }
        let n = cgroups[i].nprocs as usize;
        let mut j = 0usize;
        while j < n {
            if cgroups[i].procs[j] == pid {
                return Some(cgroups[i].id);
            }
            j = j.saturating_add(1);
        }
    }
    None
}

/// Set the CPU weight for cgroup `id`.  Clamped to 1..=10 000.
///
/// Returns false if the cgroup does not exist.
pub fn cgroup_set_cpu_weight(id: u32, weight: u32) -> bool {
    let clamped = if weight < 1 {
        1
    } else if weight > 10_000 {
        10_000
    } else {
        weight
    };
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    cgroups[slot].cpu.weight = clamped;
    true
}

/// Set the CPU period and quota for cgroup `id`.
///
/// `quota_us` of -1 disables quota enforcement (unlimited bandwidth).
/// Returns false if the cgroup does not exist.
pub fn cgroup_set_cpu_quota(id: u32, period_us: u64, quota_us: i64) -> bool {
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    cgroups[slot].cpu.period_us = period_us;
    cgroups[slot].cpu.quota_us = quota_us;
    true
}

/// Set the memory hard limit for cgroup `id`.
///
/// `limit_bytes` of u64::MAX means unlimited.
/// Returns false if the cgroup does not exist.
pub fn cgroup_set_mem_limit(id: u32, limit_bytes: u64) -> bool {
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    cgroups[slot].mem.limit_bytes = limit_bytes;
    true
}

/// Set I/O read/write byte-per-second limits for cgroup `id`.
///
/// 0 means unlimited for either field.
/// Returns false if the cgroup does not exist.
pub fn cgroup_set_io_limits(id: u32, rbps: u64, wbps: u64) -> bool {
    let mut cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    cgroups[slot].io.rbps_limit = rbps;
    cgroups[slot].io.wbps_limit = wbps;
    true
}

/// Charge `bytes` of memory to the cgroup that contains `pid`.
///
/// Returns true if the allocation is within the cgroup's memory limit and the
/// usage counter has been incremented; returns false if the allocation would
/// exceed the hard limit (usage is NOT incremented in that case).
///
/// If the pid is not a member of any cgroup, the charge is permitted (true).
pub fn cgroup_charge_mem(pid: u32, bytes: u64) -> bool {
    let mut cgroups = CGROUPS.lock();

    // Locate which cgroup owns this pid.
    let mut owner_slot: Option<usize> = None;
    'outer: for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            continue;
        }
        let n = cgroups[i].nprocs as usize;
        let mut j = 0usize;
        while j < n {
            if cgroups[i].procs[j] == pid {
                owner_slot = Some(i);
                break 'outer;
            }
            j = j.saturating_add(1);
        }
    }

    let slot = match owner_slot {
        Some(s) => s,
        None => return true, // Not in any cgroup — permit freely.
    };

    let new_usage = cgroups[slot].mem.usage_bytes.saturating_add(bytes);
    let limit = cgroups[slot].mem.limit_bytes;

    // u64::MAX is the sentinel for "unlimited".
    if limit != u64::MAX && new_usage > limit {
        return false;
    }

    cgroups[slot].mem.usage_bytes = new_usage;
    true
}

/// Reduce the memory usage counter for the cgroup that contains `pid`.
///
/// Saturates at 0 to prevent underflow.  Silently ignored if the pid is not
/// in any cgroup.
pub fn cgroup_uncharge_mem(pid: u32, bytes: u64) {
    let mut cgroups = CGROUPS.lock();

    let mut owner_slot: Option<usize> = None;
    'outer: for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            continue;
        }
        let n = cgroups[i].nprocs as usize;
        let mut j = 0usize;
        while j < n {
            if cgroups[i].procs[j] == pid {
                owner_slot = Some(i);
                break 'outer;
            }
            j = j.saturating_add(1);
        }
    }

    if let Some(slot) = owner_slot {
        cgroups[slot].mem.usage_bytes = cgroups[slot].mem.usage_bytes.saturating_sub(bytes);
    }
}

/// Accumulate I/O byte counts for the cgroup that contains `pid`.
///
/// `read_bytes`  — bytes transferred in from storage.
/// `write_bytes` — bytes transferred out to storage.
///
/// Both counters use saturating addition.  Silently ignored if the pid is not
/// in any cgroup.
pub fn cgroup_charge_io(pid: u32, read_bytes: u64, write_bytes: u64) {
    let mut cgroups = CGROUPS.lock();

    let mut owner_slot: Option<usize> = None;
    'outer: for i in 0..MAX_CGROUPS {
        if !cgroups[i].active {
            continue;
        }
        let n = cgroups[i].nprocs as usize;
        let mut j = 0usize;
        while j < n {
            if cgroups[i].procs[j] == pid {
                owner_slot = Some(i);
                break 'outer;
            }
            j = j.saturating_add(1);
        }
    }

    if let Some(slot) = owner_slot {
        cgroups[slot].io.rbytes = cgroups[slot].io.rbytes.saturating_add(read_bytes);
        cgroups[slot].io.wbytes = cgroups[slot].io.wbytes.saturating_add(write_bytes);
    }
}

/// Check whether cgroup `id` is within its CPU quota.
///
/// `current_us` — the CPU microseconds used in the current period.
///
/// Returns true if:
///   - the cgroup has no quota (quota_us == -1), OR
///   - `current_us` is less than or equal to `quota_us`.
///
/// Returns false if the quota is exceeded, or if the cgroup does not exist.
pub fn cgroup_check_cpu_quota(id: u32, current_us: u64) -> bool {
    let cgroups = CGROUPS.lock();
    let slot = match find_slot(&cgroups, id) {
        Some(s) => s,
        None => return false,
    };
    let quota = cgroups[slot].cpu.quota_us;
    if quota == -1 {
        return true; // unlimited
    }
    // quota_us is guaranteed non-negative here (> -1).
    let quota_u64 = quota as u64;
    current_us <= quota_u64
}

/// Zero the accumulated CPU usage counter for cgroup `id`.
///
/// Typically called at the start of a new scheduling period.
/// Silently ignored if the cgroup does not exist.
pub fn cgroup_reset_cpu_usage(id: u32) {
    let mut cgroups = CGROUPS.lock();
    if let Some(slot) = find_slot(&cgroups, id) {
        cgroups[slot].cpu.usage_us = 0;
    }
}

/// Retrieve summary statistics for cgroup `id`.
///
/// Returns `Some((cpu_usage_us, mem_usage_bytes, io_rbytes, io_wbytes))`,
/// or `None` if the cgroup does not exist.
pub fn cgroup_get_stats(id: u32) -> Option<(u64, u64, u64, u64)> {
    let cgroups = CGROUPS.lock();
    let slot = find_slot(&cgroups, id)?;
    let cg = &cgroups[slot];
    Some((
        cg.cpu.usage_us,
        cg.mem.usage_bytes,
        cg.io.rbytes,
        cg.io.wbytes,
    ))
}

// ---------------------------------------------------------------------------
// Subsystem initializer
// ---------------------------------------------------------------------------

/// Initialize the cgroup v2 controller subsystem.
///
/// Creates the root cgroup (id=1, name="root") in slot 0 and announces
/// readiness on the serial console.
pub fn init() {
    let mut cgroups = CGROUPS.lock();

    // Consume id=1 from the counter so NEXT_CGROUP_ID starts at 2 afterward.
    // We set ROOT_CGROUP_ID after the lock to avoid ordering issues, but the
    // constant is effectively always 1.
    let root_id = NEXT_CGROUP_ID.fetch_add(1, Ordering::Relaxed);
    // Safety: root_id == 1 on a clean boot; if somehow the counter wrapped
    // past 0 we would get an unexpected id.  Using fetch_add with Relaxed is
    // sufficient because this is single-threaded at init time.

    let root = &mut cgroups[0];
    *root = Cgroup::empty();
    root.id = root_id;
    root.parent_id = 0;
    root.active = true;

    // Write "root\0" into the name array.
    root.name[0] = b'r';
    root.name[1] = b'o';
    root.name[2] = b'o';
    root.name[3] = b't';
    root.name_len = 4;

    drop(cgroups);

    ROOT_CGROUP_ID.store(root_id, Ordering::Relaxed);

    serial_println!("[cgroup] v2 controller initialized");
}
