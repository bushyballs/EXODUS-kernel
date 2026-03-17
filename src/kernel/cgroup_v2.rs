/// cgroup_v2 — Linux cgroup v2 (unified hierarchy) resource controller
///
/// Provides a unified hierarchy where each cgroup may have controllers:
///   - cpu: weight-based CPU scheduling shares
///   - memory: RSS limit with OOM-kill-first policy
///   - io: iops/bps throttling per block device
///   - pids: maximum number of processes in the cgroup
///
/// Design: flat table of cgroup entries (no tree — parent is tracked by id).
/// All process associations are tracked in a PID→cgroup_id map.
///
/// Inspired by: Linux kernel/cgroup/cgroup.c. All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_CGROUPS: usize = 64;
const MAX_PIDS: usize = 256;
const CGROUP_NAME_LEN: usize = 32;

// Controller flags (bitmask)
pub const CTRL_CPU: u32 = 1 << 0;
pub const CTRL_MEMORY: u32 = 1 << 1;
pub const CTRL_IO: u32 = 1 << 2;
pub const CTRL_PIDS: u32 = 1 << 3;

// ---------------------------------------------------------------------------
// Cgroup entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Cgroup {
    pub id: u32,
    pub parent: u32, // 0 = root cgroup
    pub name: [u8; CGROUP_NAME_LEN],
    pub name_len: u8,
    pub controllers: u32, // enabled controller bitmask

    // cpu controller
    pub cpu_weight: u32,    // 1..10000, default 100
    pub cpu_max_us: u64,    // period quota in µs (0 = unlimited)
    pub cpu_period_us: u64, // period length in µs (default 100_000)

    // memory controller
    pub mem_max_bytes: u64, // RSS limit in bytes (0 = unlimited)
    pub mem_current: u64,   // current RSS bytes
    pub mem_high: u64,      // throttle threshold (0 = unlimited)
    pub mem_oom_kill: bool, // kill on OOM instead of just throttle

    // pids controller
    pub pids_max: u32, // max processes (0 = unlimited)
    pub pids_current: u32,

    // io controller (single device throttle for simplicity)
    pub io_rbps_max: u64,  // read  bytes/s limit (0 = unlimited)
    pub io_wbps_max: u64,  // write bytes/s limit (0 = unlimited)
    pub io_riops_max: u32, // read  IOPS limit (0 = unlimited)
    pub io_wiops_max: u32, // write IOPS limit (0 = unlimited)

    pub active: bool,
}

impl Cgroup {
    pub const fn empty() -> Self {
        Cgroup {
            id: 0,
            parent: 0,
            name: [0u8; CGROUP_NAME_LEN],
            name_len: 0,
            controllers: 0,
            cpu_weight: 100,
            cpu_max_us: 0,
            cpu_period_us: 100_000,
            mem_max_bytes: 0,
            mem_current: 0,
            mem_high: 0,
            mem_oom_kill: true,
            pids_max: 0,
            pids_current: 0,
            io_rbps_max: 0,
            io_wbps_max: 0,
            io_riops_max: 0,
            io_wiops_max: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PID→cgroup association table
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct PidAssoc {
    pid: u32,
    cgroup_id: u32,
    active: bool,
}

impl PidAssoc {
    const fn empty() -> Self {
        PidAssoc {
            pid: 0,
            cgroup_id: 0,
            active: false,
        }
    }
}

const EMPTY_CG: Cgroup = Cgroup::empty();
const EMPTY_PA: PidAssoc = PidAssoc::empty();
static CGROUPS: Mutex<[Cgroup; MAX_CGROUPS]> = Mutex::new([EMPTY_CG; MAX_CGROUPS]);
static PID_ASSOC: Mutex<[PidAssoc; MAX_PIDS]> = Mutex::new([EMPTY_PA; MAX_PIDS]);
static CG_NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn copy_name(dst: &mut [u8; CGROUP_NAME_LEN], src: &[u8]) -> u8 {
    let len = src.len().min(CGROUP_NAME_LEN - 1);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

// ---------------------------------------------------------------------------
// Public API: cgroup lifecycle
// ---------------------------------------------------------------------------

/// Create a new cgroup. Returns id or 0 on failure.
pub fn cgroup_create(name: &[u8], parent: u32) -> u32 {
    let id = CG_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if !cgs[i].active {
            cgs[i] = Cgroup::empty();
            cgs[i].id = id;
            cgs[i].parent = parent;
            cgs[i].name_len = copy_name(&mut cgs[i].name, name);
            cgs[i].active = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

pub fn cgroup_destroy(id: u32) -> bool {
    // Only if no processes attached
    let pa = PID_ASSOC.lock();
    let mut j = 0usize;
    while j < MAX_PIDS {
        if pa[j].active && pa[j].cgroup_id == id {
            return false;
        }
        j = j.saturating_add(1);
    }
    drop(pa);
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Enable a controller on a cgroup.
pub fn cgroup_enable_controller(id: u32, ctrl: u32) -> bool {
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].controllers |= ctrl;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Process attachment
// ---------------------------------------------------------------------------

/// Attach a PID to a cgroup.
pub fn cgroup_attach_pid(cgroup_id: u32, pid: u32) -> bool {
    // Verify cgroup exists
    {
        let cgs = CGROUPS.lock();
        let mut found = false;
        let mut i = 0usize;
        while i < MAX_CGROUPS {
            if cgs[i].active && cgs[i].id == cgroup_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return false;
        }
    }
    // Check pids_max constraint
    {
        let mut cgs = CGROUPS.lock();
        let mut i = 0usize;
        while i < MAX_CGROUPS {
            if cgs[i].active
                && cgs[i].id == cgroup_id
                && (cgs[i].controllers & CTRL_PIDS != 0)
                && cgs[i].pids_max > 0
                && cgs[i].pids_current >= cgs[i].pids_max
            {
                return false; // pids limit reached
            }
            i = i.saturating_add(1);
        }
        // Increment pids_current
        i = 0;
        while i < MAX_CGROUPS {
            if cgs[i].active && cgs[i].id == cgroup_id {
                cgs[i].pids_current = cgs[i].pids_current.saturating_add(1);
                break;
            }
            i = i.saturating_add(1);
        }
    }
    let mut pa = PID_ASSOC.lock();
    // Update existing entry if PID already tracked
    let mut j = 0usize;
    while j < MAX_PIDS {
        if pa[j].active && pa[j].pid == pid {
            pa[j].cgroup_id = cgroup_id;
            return true;
        }
        j = j.saturating_add(1);
    }
    // New entry
    j = 0;
    while j < MAX_PIDS {
        if !pa[j].active {
            pa[j] = PidAssoc {
                pid,
                cgroup_id,
                active: true,
            };
            return true;
        }
        j = j.saturating_add(1);
    }
    false
}

/// Detach a PID from all cgroups (on process exit).
pub fn cgroup_detach_pid(pid: u32) {
    let mut pa = PID_ASSOC.lock();
    let mut j = 0usize;
    while j < MAX_PIDS {
        if pa[j].active && pa[j].pid == pid {
            let cg_id = pa[j].cgroup_id;
            pa[j].active = false;
            drop(pa);
            // Decrement pids_current
            let mut cgs = CGROUPS.lock();
            let mut i = 0usize;
            while i < MAX_CGROUPS {
                if cgs[i].active && cgs[i].id == cg_id {
                    cgs[i].pids_current = cgs[i].pids_current.saturating_sub(1);
                    break;
                }
                i = i.saturating_add(1);
            }
            return;
        }
        j = j.saturating_add(1);
    }
}

/// Get the cgroup ID a PID belongs to.
pub fn cgroup_of_pid(pid: u32) -> u32 {
    let pa = PID_ASSOC.lock();
    let mut j = 0usize;
    while j < MAX_PIDS {
        if pa[j].active && pa[j].pid == pid {
            return pa[j].cgroup_id;
        }
        j = j.saturating_add(1);
    }
    0 // root cgroup
}

// ---------------------------------------------------------------------------
// Controller setters
// ---------------------------------------------------------------------------

pub fn cgroup_set_cpu_weight(id: u32, weight: u32) -> bool {
    let w = weight.max(1).min(10_000);
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].cpu_weight = w;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn cgroup_set_cpu_max(id: u32, quota_us: u64, period_us: u64) -> bool {
    if period_us == 0 {
        return false;
    }
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].cpu_max_us = quota_us;
            cgs[i].cpu_period_us = period_us;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn cgroup_set_mem_max(id: u32, bytes: u64) -> bool {
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].mem_max_bytes = bytes;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn cgroup_update_mem_current(id: u32, bytes: u64) {
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].mem_current = bytes;
            return;
        }
        i = i.saturating_add(1);
    }
}

/// Returns true if the cgroup is over its memory limit (OOM condition).
pub fn cgroup_mem_oom(id: u32) -> bool {
    let cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            return cgs[i].mem_max_bytes > 0
                && cgs[i].mem_current > cgs[i].mem_max_bytes
                && cgs[i].mem_oom_kill;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn cgroup_set_pids_max(id: u32, max: u32) -> bool {
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].pids_max = max;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn cgroup_set_io_max(id: u32, rbps: u64, wbps: u64, riops: u32, wiops: u32) -> bool {
    let mut cgs = CGROUPS.lock();
    let mut i = 0usize;
    while i < MAX_CGROUPS {
        if cgs[i].active && cgs[i].id == id {
            cgs[i].io_rbps_max = rbps;
            cgs[i].io_wbps_max = wbps;
            cgs[i].io_riops_max = riops;
            cgs[i].io_wiops_max = wiops;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    // Create root cgroup (id=1, parent=0)
    let root = cgroup_create(b"system", 0);
    cgroup_enable_controller(root, CTRL_CPU | CTRL_MEMORY | CTRL_PIDS);
    serial_println!(
        "[cgroup_v2] unified cgroup v2 hierarchy initialized (max {} cgroups)",
        MAX_CGROUPS
    );
}
